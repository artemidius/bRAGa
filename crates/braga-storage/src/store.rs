// SPDX-License-Identifier: Apache-2.0

//! The encrypted [`Store`] facade over `SQLCipher`.

use std::{io, path::Path, sync::Once};

use rusqlite::{Connection, OptionalExtension as _, ffi, params};
use thiserror::Error;

use crate::{
    key::{Key, KeyFileError},
    migrations::{MIGRATIONS, apply_migrations},
};

/// Entry-point type expected by `sqlite3_auto_extension`.
type AutoExtension = unsafe extern "C" fn(
    *mut ffi::sqlite3,
    *mut *mut std::ffi::c_char,
    *const ffi::sqlite3_api_routines,
) -> std::ffi::c_int;

static REGISTER_VEC: Once = Once::new();

/// Registers `sqlite-vec` as a process-wide auto-extension.
///
/// This must run before the first connection is opened. `sqlite-vec` declares
/// `sqlite3_vec_init` with a `()` signature and transmutes it; we repeat that cast with an
/// explicit target type.
fn register_vector_extension() {
    REGISTER_VEC.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the C entry point exported by the statically linked
        // `sqlite-vec` library. SQLite calls it as an extension initializer, which is exactly the
        // signature declared by `AutoExtension`.
        let init: AutoExtension =
            unsafe { std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ()) };
        // SAFETY: `init` is a valid SQLite extension entry point and registration is idempotent.
        unsafe {
            ffi::sqlite3_auto_extension(Some(init));
        }
    });
}

/// Error returned by [`Store`] operations.
///
/// `rusqlite` errors are converted internally and never cross the crate boundary: only their
/// numeric code survives, because a `rusqlite` message may embed SQL text.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    /// A filesystem operation failed (key file, directory, or `VACUUM INTO` target).
    #[error("storage I/O error: {0}")]
    Io(#[from] io::Error),
    /// The key file could not be loaded.
    #[error("key file: {0}")]
    KeyFile(#[from] KeyFileError),
    /// The supplied key does not decrypt the database; no key material is retained.
    #[error("wrong database key")]
    WrongKey,
    /// The database was created by a newer schema than this build supports.
    #[error("database schema version {found} is newer than supported {supported}")]
    NewerDatabase {
        /// Version found in the database.
        found: u32,
        /// Highest version this build supports.
        supported: u32,
    },
    /// A numbered migration failed; its transaction was rolled back.
    #[error("migration failed at version {version}: {name}")]
    Migration {
        /// Version of the failing migration.
        version: u32,
        /// Name of the failing migration.
        name: &'static str,
    },
    /// `SQLite` rejected an operation; only its numeric code is retained.
    ///
    /// `code` is `-1` when `rusqlite` reports an error that carries no `SQLite` result code.
    #[error("sqlite error code {code}")]
    Backend {
        /// `SQLite` extended result code, or `-1` for a non-`SQLite` `rusqlite` error.
        code: i32,
    },
    /// A `model_registry` row already exists with a different `dim` or `metric` (pin violated).
    #[error("model `{model}` is pinned to a different dim or metric")]
    ModelConflict {
        /// The model whose registered shape differs from the requested one.
        model: String,
    },
    /// `dim` is zero/out of range or `metric` is not one of `cosine` | `l2` | `ip`.
    #[error("invalid vec0 parameter: {parameter}")]
    InvalidVec0Parameter {
        /// The rejected parameter name (`dim` or `metric`).
        parameter: &'static str,
    },
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        match error {
            rusqlite::Error::SqliteFailure(failure, _) => {
                if failure.code == rusqlite::ErrorCode::NotADatabase {
                    Self::WrongKey
                } else {
                    Self::Backend { code: failure.extended_code }
                }
            }
            _ => Self::Backend { code: -1 },
        }
    }
}

/// One `files` row as read for scanning.
///
/// `content_hash` stays an opaque byte vector so this crate never depends on the chunker; its
/// rustdoc records the invariant the writer must honor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    /// `files.id`, the integer primary key that `chunks.file_id` references.
    pub id: i64,
    /// `files.path`, the file's UTF-8 path, exactly as the writer stored it.
    pub path: String,
    /// `files.content_hash`, the 32-byte `SHA-256` digest of the file's bytes.
    pub content_hash: Vec<u8>,
}

/// A chunk to write for a file, mirroring the `chunks` columns.
///
/// `file_id` is absent because [`Store::sync_file`] supplies the id of the row it upserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewChunk<'a> {
    /// `chunks.id`: the 32-byte `SHA-256` of [`content`](Self::content).
    pub id: &'a [u8],
    /// `chunks.byte_offset`: byte offset of `content` in the source file.
    pub byte_offset: i64,
    /// `chunks.line_start`: 1-based, inclusive first line of `content`.
    pub line_start: i64,
    /// `chunks.line_end`: 1-based, inclusive last line of `content`.
    pub line_end: i64,
    /// `chunks.content`: the exact chunk text.
    pub content: &'a str,
}

/// One ranked KNN hit: `chunks.rowid` and its `vec0` distance (ascending).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorHit {
    /// `chunks.rowid` of the hit, the join key back to `chunks`.
    pub rowid: i64,
    /// The `vec0` distance under the table's metric; smaller is closer.
    pub distance: f32,
}

/// One `chunks` row joined to its `files.path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRow {
    /// `chunks.id`, the 32-byte `SHA-256` digest of the chunk bytes.
    pub chunk_id: Vec<u8>,
    /// `files.path` of the file the chunk belongs to.
    pub path: String,
    /// `chunks.line_start`: 1-based, inclusive first line of the chunk.
    pub line_start: i64,
    /// `chunks.line_end`: 1-based, inclusive last line of the chunk.
    pub line_end: i64,
    /// `chunks.content`: the exact chunk text.
    pub content: String,
}

/// An open, encrypted `SQLite` database.
///
/// `Store` owns its connection; dropping it closes the database. Reopening is closing (or
/// dropping) it followed by [`Store::open`]. Neither `rusqlite` nor `sqlite-vec` types appear in
/// its public API.
///
/// Key rotation is reserved for a later stage: the intended primitive is `Store::rekey(&self,
/// new_key: &Key)`, backed by `sqlite3_rekey_v2` (reachable as `Connection::key(_, new_key, true)`).
/// It is not implemented here; `braga-extra` defines the rotation contract first.
pub struct Store {
    conn: Connection,
}

/// Validates the `vec0` dimension and distance metric accepted by
/// [`Store::ensure_embedding_table`].
///
/// `1..=16384` is the documented width bound; the metric names are the frozen public set, which
/// this crate validates even though the pinned `sqlite-vec` implements only `l2` and `cosine`.
fn validate_vec0_parameter(dim: u32, metric: &str) -> Result<(), StoreError> {
    if !(1..=16384).contains(&dim) {
        return Err(StoreError::InvalidVec0Parameter { parameter: "dim" });
    }
    match metric {
        "cosine" | "l2" | "ip" => Ok(()),
        _ => Err(StoreError::InvalidVec0Parameter { parameter: "metric" }),
    }
}

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

    /// Reads every `files` row, ordered by `path`.
    ///
    /// The scanner uses this one query for both jobs it needs: detecting deletions (a stored path
    /// absent from the walk) and comparing content hashes without a per-file query.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the query.
    pub fn list_files(&self) -> Result<Vec<FileRow>, StoreError> {
        let mut statement =
            self.conn.prepare("SELECT id, path, content_hash FROM files ORDER BY path")?;
        let rows = statement.query_map([], |row| {
            Ok(FileRow { id: row.get(0)?, path: row.get(1)?, content_hash: row.get(2)? })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
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

    /// Reads the registered `(dim, metric)` for `model`, if any.
    ///
    /// The registry row is the authoritative vector shape once a model has been used, so a fresh
    /// process can recover the dimension that its adapter has not learned yet.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the query.
    pub fn registered_model(&self, model: &str) -> Result<Option<(u32, String)>, StoreError> {
        self.conn
            .query_row("SELECT dim, metric FROM model_registry WHERE model = ?1", [model], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(StoreError::from)
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

    /// Cache lookup: does `chunk_id` already have an embedding for `(model, dim)`?
    ///
    /// The key is `(chunk_id, model, dim)`: `chunk_id` is the `SHA-256` of the chunk bytes, so a
    /// tokenizer change never invalidates a cached vector.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the query or the table does not exist.
    pub fn embedding_exists(
        &self,
        model: &str,
        dim: u32,
        chunk_id: &[u8],
    ) -> Result<bool, StoreError> {
        self.conn
            .query_row(
                &format!(
                    "SELECT EXISTS(
                         SELECT 1 FROM \"embeddings_d{dim}\"
                         WHERE chunk_id = ?1 AND model = ?2 AND dim = ?3
                     )"
                ),
                params![chunk_id, model, dim],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
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

    /// Runs a top-k KNN search against the `dim` dimension's `vec0` table.
    ///
    /// The query vector is bound as the same little-endian `f32` BLOB that
    /// [`Store::insert_embedding`] writes. `top_k` is bound as `vec0`'s `k`; hits come back ordered
    /// by ascending distance under the table's metric.
    ///
    /// The table is scoped to one model by the stage's one-model-per-collection invariant, so no
    /// `model` filter is applied: `vec0` rejects auxiliary-column constraints in a KNN query.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `dim`'s table does not exist, the vector width does not
    /// match it, or `SQLite` rejects the query.
    pub fn vector_search(
        &self,
        dim: u32,
        query: &[f32],
        top_k: u32,
    ) -> Result<Vec<VectorHit>, StoreError> {
        let bytes: Vec<u8> = query.iter().flat_map(|value| value.to_le_bytes()).collect();
        let mut statement = self.conn.prepare(&format!(
            "SELECT rowid, distance FROM \"embeddings_d{dim}\"
             WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance"
        ))?;
        let rows = statement.query_map(params![bytes, top_k], |row| {
            Ok(VectorHit { rowid: row.get(0)?, distance: row.get(1)? })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    /// Runs a top-k FTS5 keyword search and returns `chunks.rowid`s, most relevant first.
    ///
    /// `query` is FTS5 query syntax, passed through unchanged; a malformed expression fails with
    /// [`StoreError::Backend`]. The result order follows FTS5's `rank` (`bm25`, ascending), which is
    /// all the fusion needs.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if the expression is invalid or `SQLite` rejects the query.
    pub fn fts_search(&self, query: &str, top_k: u32) -> Result<Vec<i64>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows = statement.query_map(params![query, top_k], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    /// Reads the chunks for `rowids`, joined to their file paths.
    ///
    /// Rows come back in ascending `rowid` order, so a caller holding a sorted rowid list can pair
    /// them positionally. Empty input returns an empty vector without querying, and the dynamic
    /// `IN` list is bounded by the caller's `top_k`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the query.
    pub fn chunk_metadata(&self, rowids: &[i64]) -> Result<Vec<ChunkRow>, StoreError> {
        if rowids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (1..=rowids.len()).map(|index| format!("?{index}")).collect::<Vec<_>>();
        let mut statement = self.conn.prepare(&format!(
            "SELECT c.id, f.path, c.line_start, c.line_end, c.content
             FROM chunks c JOIN files f ON f.id = c.file_id
             WHERE c.rowid IN ({}) ORDER BY c.rowid",
            placeholders.join(", ")
        ))?;
        let rows = statement.query_map(rusqlite::params_from_iter(rowids), |row| {
            Ok(ChunkRow {
                chunk_id: row.get(0)?,
                path: row.get(1)?,
                line_start: row.get(2)?,
                line_end: row.get(3)?,
                content: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
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
