// SPDX-License-Identifier: Apache-2.0

//! Retrieval-side [`Store`] queries.

use rusqlite::params;

use super::{Store, StoreError, VectorHit};

impl Store {
    /// Runs a top-k KNN search against the `dim` dimension's `vec0` table.
    ///
    /// The query vector is bound as the same little-endian `f32` BLOB that
    /// [`Store::insert_embedding`] writes. `top_k` is bound as `vec0`'s `k`; hits come back ordered
    /// by ascending distance under the table's metric.
    ///
    /// The table is scoped to one model by the stage's one-model-per-collection invariant, so no
    /// `model` filter is applied: `vec0` rejects auxiliary-column constraints in a KNN query.
    ///
    /// `vec0` cannot rank ciphertext, so extra mode has no in-SQL KNN path and fails closed with
    /// [`StoreError::SeamUnavailable`]; `braga-extra` decrypts through
    /// [`Store::list_embeddings`](crate::Store::list_embeddings) and scores in memory instead.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `dim`'s table does not exist, the vector width does not
    /// match it, or `SQLite` rejects the query, and [`StoreError::SeamUnavailable`] when an
    /// encryptor is configured.
    pub fn vector_search(
        &self,
        dim: u32,
        query: &[f32],
        top_k: u32,
    ) -> Result<Vec<VectorHit>, StoreError> {
        if self.encryptor.is_some() {
            return Err(StoreError::SeamUnavailable);
        }
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
    /// Extra mode never populates `chunks_fts`, so keyword search fails closed with
    /// [`StoreError::SeamUnavailable`] instead of returning an empty, misleading result.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if the expression is invalid or `SQLite` rejects the query,
    /// and [`StoreError::SeamUnavailable`] when an encryptor is configured.
    pub fn fts_search(&self, query: &str, top_k: u32) -> Result<Vec<i64>, StoreError> {
        if self.encryptor.is_some() {
            return Err(StoreError::SeamUnavailable);
        }
        let mut statement = self.conn.prepare(
            "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows = statement.query_map(params![query, top_k], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    /// Rebuilds the `chunks_fts` index from the `chunks` content table.
    ///
    /// The per-file methods maintain the index incrementally; this is the consistency-repair
    /// primitive for chunk rows mutated behind the index's back, the future `doctor` invariant, and
    /// the proof that the external-content linkage is wired correctly.
    ///
    /// Extra mode never populates `chunks_fts`, so a rebuild fails closed with
    /// [`StoreError::SeamUnavailable`] rather than indexing ciphertext.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if `SQLite` rejects the rebuild, and
    /// [`StoreError::SeamUnavailable`] when an encryptor is configured.
    pub fn refresh_fts(&self) -> Result<(), StoreError> {
        if self.encryptor.is_some() {
            return Err(StoreError::SeamUnavailable);
        }
        self.conn.execute("INSERT INTO chunks_fts(chunks_fts) VALUES ('rebuild')", [])?;
        Ok(())
    }
}
