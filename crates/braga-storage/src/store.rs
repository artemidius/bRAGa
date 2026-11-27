// SPDX-License-Identifier: Apache-2.0

//! The encrypted [`Store`] facade over `SQLCipher`.

use std::{io, path::Path, sync::Once};

use rusqlite::{Connection, ffi};
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
