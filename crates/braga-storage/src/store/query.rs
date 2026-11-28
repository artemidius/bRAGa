// SPDX-License-Identifier: Apache-2.0

//! Retrieval-side [`Store`] queries.

use rusqlite::params;

use super::{ChunkRow, Store, StoreError, VectorHit};

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
}
