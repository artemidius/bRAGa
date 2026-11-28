// SPDX-License-Identifier: Apache-2.0

//! Read-side [`Store`] queries.

use rusqlite::{OptionalExtension as _, params};

use crate::seam::SeamError;

use super::{ChunkRow, FileRow, Store, StoreError, chunk_aad, embedding_aad, open_text};

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
    /// vector. In extra mode the stored content is hex ciphertext and is decrypted before return.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the query, and [`StoreError::Seam`] when
    /// a stored chunk cannot be decrypted.
    pub fn all_chunks(&self) -> Result<Vec<(Vec<u8>, String)>, StoreError> {
        let mut statement = self.conn.prepare("SELECT id, content FROM chunks ORDER BY rowid")?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        match self.encryptor.as_deref() {
            None => Ok(rows),
            Some(encryptor) => rows
                .into_iter()
                .map(|(chunk_id, stored)| {
                    let content = open_text(encryptor, &chunk_aad(&chunk_id), &stored)?;
                    Ok((chunk_id, content))
                })
                .collect(),
        }
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
    /// tokenizer change never invalidates a cached vector. Extra mode checks the side
    /// `encrypted_embeddings` table, which does not exist until the first encrypted insert.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the query or the base-mode table does
    /// not exist.
    pub fn embedding_exists(
        &self,
        model: &str,
        dim: u32,
        chunk_id: &[u8],
    ) -> Result<bool, StoreError> {
        if self.encryptor.is_some() {
            if !self.has_encrypted_embeddings()? {
                return Ok(false);
            }
            return self
                .conn
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM encrypted_embeddings
                         WHERE chunk_id = ?1 AND model = ?2 AND dim = ?3
                     )",
                    params![chunk_id, model, dim],
                    |row| row.get(0),
                )
                .map_err(StoreError::from);
        }
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

    /// Reads and decrypts every embedding for `(model, dim)` in extra mode.
    ///
    /// Vectors are returned as `(chunk_id, vector)` pairs so a caller can score them in memory.
    /// Base mode has no decryptable ciphertext at rest and fails closed with
    /// [`StoreError::SeamUnavailable`].
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::SeamUnavailable`] in base mode, [`StoreError::Backend`] if `SQLite`
    /// rejects the query, and [`StoreError::Seam`] when a stored vector cannot be decrypted or has
    /// the wrong shape.
    #[allow(clippy::type_complexity)] // the frozen seam contract spells the pair type out
    pub fn list_embeddings(
        &self,
        model: &str,
        dim: u32,
    ) -> Result<Vec<(Vec<u8>, Vec<f32>)>, StoreError> {
        let Some(encryptor) = self.encryptor.as_deref() else {
            return Err(StoreError::SeamUnavailable);
        };
        if !self.has_encrypted_embeddings()? {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare(
            "SELECT chunk_id, ciphertext FROM encrypted_embeddings
             WHERE model = ?1 AND dim = ?2 ORDER BY chunk_id",
        )?;
        let rows = statement
            .query_map(params![model, dim], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let expected = usize::try_from(dim)
            .map_err(|_| StoreError::InvalidVec0Parameter { parameter: "dim" })?
            .checked_mul(4)
            .ok_or(StoreError::InvalidVec0Parameter { parameter: "dim" })?;
        let mut embeddings = Vec::with_capacity(rows.len());
        for (chunk_id, ciphertext) in rows {
            let plaintext = encryptor.decrypt(&embedding_aad(model, &chunk_id), &ciphertext)?;
            if plaintext.len() != expected {
                return Err(StoreError::Seam(SeamError::Decrypt));
            }
            let vector = plaintext
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes))
                .collect();
            embeddings.push((chunk_id, vector));
        }
        Ok(embeddings)
    }

    /// Reads the chunks for `rowids`, joined to their file paths.
    ///
    /// Rows come back in ascending `rowid` order, so a caller holding a sorted rowid list can pair
    /// them positionally. Empty input returns an empty vector without querying, and the dynamic
    /// `IN` list is bounded by the caller's `top_k`. In extra mode each row's stored content is
    /// hex ciphertext and is decrypted before return.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the query, and [`StoreError::Seam`] when
    /// a stored chunk cannot be decrypted.
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
        let rows = rows.collect::<Result<Vec<_>, _>>()?;
        match self.encryptor.as_deref() {
            None => Ok(rows),
            Some(encryptor) => rows
                .into_iter()
                .map(|row| {
                    let content = open_text(encryptor, &chunk_aad(&row.chunk_id), &row.content)?;
                    Ok(ChunkRow { content, ..row })
                })
                .collect(),
        }
    }
}
