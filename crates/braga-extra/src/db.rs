// SPDX-License-Identifier: AGPL-3.0-or-later

//! Private `SQLCipher` access for metadata the frozen seam API does not expose.
//!
//! The seam is intentionally closed: chunk text and vectors are only read through [`Store`], and
//! every such read decrypts under the caller's master key. Metadata, which is not an AEAD secret,
//! is handled here with a second connection opened under the same `SQLCipher` key:
//!
//! * mapping a scored `chunk_id` to its integer `rowid` so the result can be hydrated through
//!   [`Store::chunk_metadata`];
//! * rewriting every ciphertext record in one transaction during [`crate::rekey`];
//! * maintaining and reading the opt-in `blind_index` postings in [`crate::blind`];
//! * counting rows for the keyless [`crate::status`] report.
//!
//! Nothing in this module decrypts an AEAD record on its own.

use std::path::Path;

use braga::config::Config;
use braga_storage::{Key, Store};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::{crypto::XChachaEncryptor, error::ExtraError, key::ExtraKey};

/// Opens a `SQLCipher` connection with the storage key applied before any statement.
pub(crate) fn open(path: &Path, key: &Key) -> Result<Connection, ExtraError> {
    let connection = Connection::open(path)?;
    connection.key(None::<&str>, key.as_bytes(), false)?;
    connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}

/// Opens the configured store with an encryptor built from `master`.
pub(crate) fn extra_store(
    config: &Config,
    key: &Key,
    master: &ExtraKey,
) -> Result<Store, ExtraError> {
    let encryptor = XChachaEncryptor::new(master.clone());
    Store::builder()
        .with_encryptor(std::sync::Arc::new(encryptor))
        .open(&config.storage.path, key)
        .map_err(ExtraError::from)
}

/// Reads the integer `rowid` of one chunk, or `None` when the chunk no longer exists.
pub(crate) fn rowid_for(
    connection: &Connection,
    chunk_id: &[u8],
) -> Result<Option<i64>, ExtraError> {
    connection
        .query_row("SELECT rowid FROM chunks WHERE id = ?1", params![chunk_id], |row| row.get(0))
        .optional()
        .map_err(ExtraError::from)
}

/// Whether a table exists in the open database.
pub(crate) fn has_table(connection: &Connection, name: &str) -> Result<bool, ExtraError> {
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
             )",
            params![name],
            |row| row.get(0),
        )
        .map_err(ExtraError::from)
}

/// Counts the rows of one of the crate's known tables.
///
/// `table` is not user input; the match keeps the dynamic SQL bounded to known names.
pub(crate) fn count(connection: &Connection, table: Table) -> Result<usize, ExtraError> {
    let sql = match table {
        Table::Files => "SELECT count(*) FROM files",
        Table::Chunks => "SELECT count(*) FROM chunks",
        Table::EncryptedEmbeddings => "SELECT count(*) FROM encrypted_embeddings",
    };
    let count: i64 = connection.query_row(sql, [], |row| row.get(0))?;
    usize::try_from(count).map_err(|_| ExtraError::InvalidData { reason: "row count overflow" })
}

/// A table this crate may count without decrypting anything.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Table {
    /// `files`, which holds paths and content hashes.
    Files,
    /// `chunks`, counted without reading its ciphertext column.
    Chunks,
    /// `encrypted_embeddings`, counted without reading its ciphertext column.
    EncryptedEmbeddings,
}
