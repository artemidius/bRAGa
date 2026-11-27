// SPDX-License-Identifier: Apache-2.0

//! The query half of the retrieval pipeline: vector KNN, FTS5 keyword search, and RRF fusion.
//!
//! [`query`] reads the collection's shape from `model_registry`, embeds the query with the same
//! [`Embedder`] that indexed the chunks, and runs both ranked lists over the Stage 7a-ii artifacts:
//! the per-dimension `vec0` table for semantic neighbors and `chunks_fts` for keyword matches.
//! Reciprocal-rank fusion combines the two orders, and each surviving `chunks.rowid` is hydrated
//! through its `files` row into a [`RetrievedChunk`].
//!
//! Only the order of each list feeds the fusion, so the two searches need not produce comparable
//! scores. A malformed FTS5 query is passed through to `SQLite` and fails closed.

use std::collections::HashMap;

use braga_embedder::{EmbedError, Embedder};
use braga_storage::{ChunkRow, Store, StoreError};
use thiserror::Error;

/// Reciprocal-rank-fusion constant; the usual value from the original RRF paper.
const RRF_K: f64 = 60.0;

/// One ranked chunk returned by [`query`].
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievedChunk {
    /// `chunks.id`, the 32-byte `SHA-256` digest of the chunk bytes.
    pub chunk_id: Vec<u8>,
    /// `files.path` of the file the chunk belongs to.
    pub path: String,
    /// 1-based, inclusive first line of the chunk.
    pub line_start: u32,
    /// 1-based, inclusive last line of the chunk.
    pub line_end: u32,
    /// The exact chunk text.
    pub content: String,
    /// Fused RRF score; results are sorted by descending score.
    pub score: f64,
}

/// Error returned by [`query`].
#[derive(Debug, Error)]
pub enum QueryError {
    /// `model_registry` has no row for the embedder's model, so nothing can be searched.
    #[error("no embeddings indexed for model `{model}`")]
    NothingEmbedded {
        /// The embedder's model name.
        model: String,
    },
    /// The query vector's width differs from the registered dimension.
    #[error("query vector dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch {
        /// The dimension pinned in `model_registry`.
        expected: u32,
        /// The width the embedder returned for the query.
        actual: usize,
    },
    /// A stored chunk row cannot be represented in the public result type.
    #[error("invalid stored chunk data: {reason}")]
    InvalidData {
        /// A short static description of the violated invariant.
        reason: &'static str,
    },
    /// A storage operation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The embedding provider failed.
    #[error(transparent)]
    Embed(#[from] EmbedError),
}

/// Queries the encrypted index and returns the top `top_k` fused chunks.
///
/// The flow is: resolve the collection from `model_registry` (failing closed when the model has
/// never been embedded), embed `text`, run a `vec0` KNN and an FTS5 search at `top_k`, fuse the two
/// rankings with RRF, then hydrate the winning rows from `chunks` joined to `files`. The final order
/// is score descending, then `chunk_id` ascending as a deterministic tie-break.
///
/// A `top_k` of `0` returns an empty vector without touching the store or the embedder.
///
/// # Errors
///
/// Returns [`QueryError::NothingEmbedded`] when the model has no registry row,
/// [`QueryError::DimensionMismatch`] when the query vector's width differs from the registered
/// dimension, [`QueryError::InvalidData`] when a stored line range does not fit a `u32` or a fused
/// chunk row is missing, [`QueryError::Store`] when a search fails (including an invalid FTS5
/// expression), and [`QueryError::Embed`] when the provider fails.
pub fn query(
    store: &Store,
    embedder: &dyn Embedder,
    text: &str,
    top_k: usize,
) -> Result<Vec<RetrievedChunk>, QueryError> {
    if top_k == 0 {
        return Ok(Vec::new());
    }

    let model = embedder.model();
    let Some((dim, _metric)) = store.registered_model(model)? else {
        return Err(QueryError::NothingEmbedded { model: model.to_owned() });
    };

    let vector = embedder
        .embed(&[text])?
        .into_iter()
        .next()
        .ok_or(EmbedError::InvalidResponse { reason: "embedder returned no query vector" })?;
    if vector.len() != dim as usize {
        return Err(QueryError::DimensionMismatch { expected: dim, actual: vector.len() });
    }

    // A `top_k` wider than `u32` is clamped: no list can be longer than that anyway.
    let limit = u32::try_from(top_k).unwrap_or(u32::MAX);
    let vector_hits = store.vector_search(dim, &vector, limit)?;
    let fts_hits = store.fts_search(text, limit)?;

    let mut fused: HashMap<i64, f64> = HashMap::new();
    for (index, hit) in vector_hits.iter().enumerate() {
        *fused.entry(hit.rowid).or_default() += rrf_score(index + 1);
    }
    for (index, rowid) in fts_hits.iter().enumerate() {
        *fused.entry(*rowid).or_default() += rrf_score(index + 1);
    }

    // Sorting by rowid lets the hydration rows pair positionally with them.
    let mut hits: Vec<(i64, f64)> = fused.into_iter().collect();
    hits.sort_unstable_by_key(|(rowid, _)| *rowid);
    let rowids: Vec<i64> = hits.iter().map(|(rowid, _)| *rowid).collect();
    let metadata = store.chunk_metadata(&rowids)?;
    if metadata.len() != hits.len() {
        return Err(QueryError::InvalidData {
            reason: "a fused chunk row is missing from storage",
        });
    }

    let mut chunks = Vec::with_capacity(hits.len());
    for ((_, score), row) in hits.into_iter().zip(metadata) {
        chunks.push(retrieved_chunk(row, score)?);
    }
    chunks.sort_by(|left, right| {
        right.score.total_cmp(&left.score).then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    chunks.truncate(top_k);
    Ok(chunks)
}

/// The reciprocal-rank-fusion contribution of a 1-based `rank`.
fn rrf_score(rank: usize) -> f64 {
    let rank = u32::try_from(rank).unwrap_or(u32::MAX);
    1.0 / (RRF_K + f64::from(rank))
}

/// Converts a stored row into a public result, validating the line range.
fn retrieved_chunk(row: ChunkRow, score: f64) -> Result<RetrievedChunk, QueryError> {
    let line_start = u32::try_from(row.line_start)
        .map_err(|_| QueryError::InvalidData { reason: "chunk line_start does not fit u32" })?;
    let line_end = u32::try_from(row.line_end)
        .map_err(|_| QueryError::InvalidData { reason: "chunk line_end does not fit u32" })?;
    Ok(RetrievedChunk {
        chunk_id: row.chunk_id,
        path: row.path,
        line_start,
        line_end,
        content: row.content,
        score,
    })
}

#[cfg(test)]
mod tests {
    use super::rrf_score;

    #[test]
    fn rrf_scores_fall_with_rank() {
        assert_eq!(rrf_score(1), 1.0 / 61.0);
        assert!(rrf_score(1) > rrf_score(2));
        assert!(rrf_score(2) > rrf_score(10));
    }
}
