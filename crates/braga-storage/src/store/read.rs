// SPDX-License-Identifier: Apache-2.0

//! Read-side [`Store`] queries.

use rusqlite::{OptionalExtension as _, params};

use super::{FileRow, Store, StoreError};

impl Store {
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

    /// Reads every chunk's id and content, ordered by ascending `rowid`.
    ///
    /// The id is the stored 32-byte `SHA-256` of the chunk bytes. An empty index returns an empty
    /// vector.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the query.
    pub fn all_chunks(&self) -> Result<Vec<(Vec<u8>, String)>, StoreError> {
        let mut statement = self.conn.prepare("SELECT id, content FROM chunks ORDER BY rowid")?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
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
}
