// SPDX-License-Identifier: Apache-2.0

//! The encrypted [`Store`] facade over `SQLCipher`.

use std::{path::Path, sync::Once};

use rusqlite::{Connection, ffi, params};
use thiserror::Error;

use crate::key::Key;

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
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    /// SQLite rejected the operation, the key is wrong, or the file could not be opened.
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// An open, encrypted SQLite database.
///
/// `Store` owns its connection; dropping it closes the database. Reopening is
/// [`drop`] followed by [`Store::open`]. Neither `rusqlite` nor `sqlite-vec` types appear in its
/// public API.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (or creates) the encrypted database at `path` with `key`.
    ///
    /// The key is the first statement issued on the connection, as `SQLCipher` 4.7 and later fail on
    /// any statement that precedes it. WAL journalling and foreign keys are enabled afterwards.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Sqlite`] when the file cannot be opened or the key is rejected.
    pub fn open(path: &Path, key: &Key) -> Result<Self, StoreError> {
        register_vector_extension();
        let conn = Connection::open(path)?;
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex(key.as_bytes())))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Ok(Self { conn })
    }

    /// Creates the spike's scratch table plus its FTS5 and `vec0` virtual tables.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Sqlite`] if any table cannot be created.
    pub fn create_scratch(&self) -> Result<(), StoreError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scratch (marker TEXT NOT NULL);
             CREATE VIRTUAL TABLE IF NOT EXISTS scratch_fts USING fts5(marker);
             CREATE VIRTUAL TABLE IF NOT EXISTS scratch_vec USING vec0(embedding float[4]);",
        )?;
        Ok(())
    }

    /// Inserts `marker` into the scratch table.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Sqlite`] if the insert fails.
    pub fn put(&self, marker: &str) -> Result<(), StoreError> {
        self.conn.execute("INSERT INTO scratch (marker) VALUES (?1)", params![marker])?;
        Ok(())
    }

    /// Returns the most recently inserted marker.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Sqlite`] if no marker exists or the database cannot be read.
    pub fn get(&self) -> Result<String, StoreError> {
        let marker = self.conn.query_row(
            "SELECT marker FROM scratch ORDER BY rowid DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        Ok(marker)
    }

    /// Checkpoints the write-ahead log into the encrypted main database file.
    ///
    /// `TRUNCATE` also empties the `-wal` file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Sqlite`] if the checkpoint fails.
    pub fn checkpoint(&self) -> Result<(), StoreError> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(64);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}
