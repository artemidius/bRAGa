// SPDX-License-Identifier: Apache-2.0

//! The [`Store`] error type.

use std::io;

use thiserror::Error;

use crate::key::KeyFileError;

/// Error returned by [`Store`](super::Store) operations.
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
