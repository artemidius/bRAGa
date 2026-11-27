// SPDX-License-Identifier: Apache-2.0

//! The encrypted [`Store`] facade over `SQLCipher`.

use std::{io, path::Path, sync::Once};

use rusqlite::{Connection, ffi, params};
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
