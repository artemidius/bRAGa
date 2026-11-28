// SPDX-License-Identifier: Apache-2.0

//! `StoreBuilder`: opens a [`Store`] with an optional crypto seam encryptor.

use std::{path::Path, sync::Arc};

use rusqlite::Connection;

use crate::{
    key::Key,
    migrations::{MIGRATIONS, apply_migrations},
    seam::ChunkEncryptor,
};

use super::{Store, StoreError, register_vector_extension};

/// Configures and opens a [`Store`].
///
/// Created by [`Store::builder`]. Without [`with_encryptor`](Self::with_encryptor) it opens in
/// base mode, with storage behavior identical to [`Store::open`].
pub struct StoreBuilder {
    encryptor: Option<Arc<dyn ChunkEncryptor>>,
}

impl StoreBuilder {
    /// Installs the per-record encryptor, switching the store to extra mode.
    #[must_use]
    pub fn with_encryptor(mut self, encryptor: Arc<dyn ChunkEncryptor>) -> Self {
        self.encryptor = Some(encryptor);
        self
    }

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
    pub fn open(self, path: &Path, key: &Key) -> Result<Store, StoreError> {
        register_vector_extension();
        let conn = Connection::open(path)?;
        conn.key(None::<&str>, key.as_bytes(), false)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        apply_migrations(&conn, MIGRATIONS)?;
        Ok(Store { conn, encryptor: self.encryptor })
    }
}

impl Store {
    /// Returns a builder for an encrypted store; [`Store::open`] is the base-mode shortcut.
    #[must_use]
    pub fn builder() -> StoreBuilder {
        StoreBuilder { encryptor: None }
    }
}
