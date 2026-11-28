// SPDX-License-Identifier: Apache-2.0

//! Write-side [`Store`] operations.

use std::{io, path::Path};

use rusqlite::{Connection, params};

use crate::{
    key::Key,
    migrations::{MIGRATIONS, apply_migrations},
};

use super::{NewChunk, Store, StoreError, register_vector_extension, validate_vec0_parameter};

impl Store {
    /// Opens (or creates) the encrypted database at `path` with `key`.
    ///
    /// The key is applied with `Connection::key`, before any other statement, because `SQLCipher`
    /// 4.7 and later hard-fails on any statement that precedes it. This keeps the key out of every
    /// SQL string. `WAL` journalling and foreign keys are enabled afterwards, then the schema
    /// migrations run inside the encrypted connection.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::WrongKey`] when `key` does not decrypt an existing database,
    /// [`StoreError::NewerDatabase`] when the database is newer than this build,
    /// [`StoreError::Migration`] when a migration fails, and [`StoreError::Backend`] when `SQLite`
    /// rejects an operation.
    pub fn open(path: &Path, key: &Key) -> Result<Self, StoreError> {
        register_vector_extension();
        let conn = Connection::open(path)?;
        conn.key(None::<&str>, key.as_bytes(), false)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        apply_migrations(&conn, MIGRATIONS)?;
        Ok(Self { conn })
    }

    /// Upserts `path` and replaces its chunks, in one transaction.
    ///
    /// The file row is upserted first, which bumps `version` and `updated_at`. Its previous chunk
    /// rows' FTS entries are then removed **before** the chunk rows themselves: an external-content
    /// FTS5 table reads the old text from the content table to remove it from the index, so
    /// deleting the rows first would orphan the entries. Each new chunk is inserted and linked to
    /// its FTS entry with `last_insert_rowid()`, the integer `rowid` `chunks_fts` expects for its
    /// `content_rowid='rowid'` link.
    ///
    /// Because the upsert and the chunk replace share one transaction, a bumped content hash can
    /// never be observed without its chunks. `unchecked_transaction` is used because the method
    /// takes `&self`; the `Store` owns its only connection, so no transaction is open across calls.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects a statement or the transaction.
    pub fn sync_file(
        &self,
        path: &str,
        content_hash: &[u8],
        chunks: &[NewChunk<'_>],
    ) -> Result<(), StoreError> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO files (path, content_hash, version, updated_at)
             VALUES (?1, ?2, 1, unixepoch())
             ON CONFLICT(path) DO UPDATE SET
                 content_hash = excluded.content_hash,
                 version      = files.version + 1,
                 updated_at   = unixepoch()",
            params![path, content_hash],
        )?;
        let file_id: i64 =
            transaction
                .query_row("SELECT id FROM files WHERE path = ?1", [path], |row| row.get(0))?;
        transaction.execute(
            "DELETE FROM chunks_fts WHERE rowid IN (SELECT rowid FROM chunks WHERE file_id = ?1)",
            [file_id],
        )?;
        transaction.execute("DELETE FROM chunks WHERE file_id = ?1", [file_id])?;
        for chunk in chunks {
            transaction.execute(
                "INSERT INTO chunks (id, file_id, byte_offset, line_start, line_end, content)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    chunk.id,
                    file_id,
                    chunk.byte_offset,
                    chunk.line_start,
                    chunk.line_end,
                    chunk.content
                ],
            )?;
            transaction.execute(
                "INSERT INTO chunks_fts (rowid, content) VALUES (?1, ?2)",
                params![transaction.last_insert_rowid(), chunk.content],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Deletes the file `id` together with its chunks and their FTS entries, in one transaction.
    ///
    /// The FTS entries go first for the same reason as in [`Store::sync_file`]; the `chunks` rows
    /// then follow through `ON DELETE CASCADE`. An unknown `id` is a no-op returning `Ok(())`,
    /// which keeps the scanner's garbage-collection loop idempotent across `TOCTOU` races.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects a statement or the transaction.
    pub fn delete_file(&self, id: i64) -> Result<(), StoreError> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM chunks_fts WHERE rowid IN (SELECT rowid FROM chunks WHERE file_id = ?1)",
            [id],
        )?;
        transaction.execute("DELETE FROM files WHERE id = ?1", [id])?;
        transaction.commit()?;
        Ok(())
    }

    /// Registers `(model, dim, metric)` on first use, failing closed if the model is already
    /// pinned to a different shape.
    ///
    /// The insert is `ON CONFLICT(model) DO NOTHING`, followed by a re-read: the identical row is
    /// idempotent, while any `dim` or `metric` mismatch returns [`StoreError::ModelConflict`]. This
    /// is the per-collection shape guard: a later configuration change fails instead of silently
    /// mixing vector widths in one dimension's table.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ModelConflict`] when the model is pinned to a different `dim` or
    /// `metric`, and [`StoreError::Backend`] if `SQLite` rejects the statement.
    pub fn register_model(&self, model: &str, dim: u32, metric: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO model_registry (model, dim, metric) VALUES (?1, ?2, ?3)
             ON CONFLICT(model) DO NOTHING",
            params![model, dim, metric],
        )?;
        match self.registered_model(model)? {
            Some((registered_dim, registered_metric))
                if registered_dim == dim && registered_metric == metric =>
            {
                Ok(())
            }
            _ => Err(StoreError::ModelConflict { model: model.to_owned() }),
        }
    }

    /// Lazily creates the per-dimension `vec0` virtual table, if absent.
    ///
    /// `vec0` fixes the vector width and distance metric in the `CREATE`, so the table is a
    /// runtime artifact of [`model_registry`](Self::registered_model) rather than a migration.
    /// The call is idempotent because of `IF NOT EXISTS`.
    ///
    /// `ip` is accepted as a valid metric parameter for the frozen public API, but the pinned
    /// `sqlite-vec` 0.1.9 implements only `l2` and `cosine`; creating an `ip` table therefore fails
    /// closed with [`StoreError::Backend`].
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidVec0Parameter`] when `dim` is outside `1..=16384` or `metric`
    /// is not `cosine`, `l2`, or `ip`, and [`StoreError::Backend`] if `SQLite` rejects the DDL.
    pub fn ensure_embedding_table(&self, dim: u32, metric: &str) -> Result<(), StoreError> {
        validate_vec0_parameter(dim, metric)?;
        self.conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS \"embeddings_d{dim}\" USING vec0(
                 embedding FLOAT[{dim}] distance_metric={metric},
                 +chunk_id BLOB,
                 +model TEXT,
                 +dim INTEGER
             )"
        ))?;
        Ok(())
    }

    /// Inserts one embedding, resolving `chunks.rowid` from the content hash internally.
    ///
    /// The vector is bound as little-endian `f32` bytes through `vec_f32`, so no JSON crosses the
    /// `braga-storage` boundary. The `vec0` rowid equals `chunks.rowid`, the documented join key.
    ///
    /// An unknown `chunk_id` fails closed with [`StoreError::Backend`] *before* the insert:
    /// `sqlite-vec` 0.1.9 auto-assigns a rowid for a `NULL` one, which would otherwise create an
    /// embedding that cannot join back to its chunk.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] when the chunk is unknown, the vector width does not match
    /// the table, or `SQLite` rejects the insert.
    pub fn insert_embedding(
        &self,
        chunk_id: &[u8],
        model: &str,
        dim: u32,
        vector: &[f32],
    ) -> Result<(), StoreError> {
        let rowid: i64 =
            self.conn.query_row("SELECT rowid FROM chunks WHERE id = ?1", [chunk_id], |row| {
                row.get(0)
            })?;
        let bytes: Vec<u8> = vector.iter().flat_map(|value| value.to_le_bytes()).collect();
        self.conn.execute(
            &format!(
                "INSERT INTO \"embeddings_d{dim}\" (rowid, embedding, chunk_id, model, dim)
                 VALUES (?1, vec_f32(?2), ?3, ?4, ?5)"
            ),
            params![rowid, bytes, chunk_id, model, dim],
        )?;
        Ok(())
    }

    /// Rebuilds the `chunks_fts` index from the `chunks` content table.
    ///
    /// The per-file methods maintain the index incrementally; this is the consistency-repair
    /// primitive for chunk rows mutated behind the index's back, the future `doctor` invariant, and
    /// the proof that the external-content linkage is wired correctly.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the rebuild.
    pub fn refresh_fts(&self) -> Result<(), StoreError> {
        self.conn.execute("INSERT INTO chunks_fts(chunks_fts) VALUES ('rebuild')", [])?;
        Ok(())
    }

    /// Checkpoints the write-ahead log into the encrypted main database file.
    ///
    /// `TRUNCATE` also empties the `-wal` file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if the checkpoint is rejected.
    pub fn checkpoint(&self) -> Result<(), StoreError> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Writes a consistent copy of the database to `path` with `VACUUM INTO`.
    ///
    /// The copy is produced on the encrypted connection, so it inherits the key and stays
    /// encrypted. `path` must not exist yet.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] when `path` is not valid UTF-8 and [`StoreError::Backend`] when
    /// `SQLite` rejects the vacuum.
    pub fn vacuum_into(&self, path: &Path) -> Result<(), StoreError> {
        let target = path.to_str().ok_or_else(|| {
            StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "VACUUM INTO target path is not valid UTF-8",
            ))
        })?;
        self.conn.execute("VACUUM INTO ?1", [target])?;
        Ok(())
    }

    /// Closes the connection explicitly, so callers can rename or back up the file deterministically.
    ///
    /// Dropping the [`Store`] closes the connection too; this method makes the close observable.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` cannot close the connection cleanly.
    pub fn close(self) -> Result<(), StoreError> {
        self.conn.close().map_err(|(_, error)| StoreError::from(error))
    }
}
