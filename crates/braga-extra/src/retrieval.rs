// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decrypt-then-score retrieval for extra mode.
//!
//! `vec0` cannot rank ciphertext, so there is no in-SQL KNN in extra mode. [`query`] instead reads
//! every embedding for the model, decrypts it through the seam, scores all vectors in memory with
//! cosine similarity, and only then hydrates the winning chunk rows. The cost is O(corpus) per
//! query; this is deliberately not an approximate-nearest-neighbour search over ciphertext.
//!
//! The query text and the chunk text are still sent to the configured embedding provider. See the
//! crate docs for the residual risk that implies for remote providers.

use std::collections::HashMap;

use braga::config::Config;
use braga_embedder::Embedder;
use braga_storage::{ChunkRow, Key, Store};

use crate::{
    db,
    error::{ExtraError, Result},
    key::ExtraKey,
};

/// One chunk returned by [`query`], ordered by descending cosine similarity.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredChunk {
    /// `chunks.id`, the 32-byte `SHA-256` digest of the chunk bytes.
    pub chunk_id: Vec<u8>,
    /// `files.path` of the file the chunk belongs to.
    pub path: String,
    /// 1-based, inclusive first line of the chunk.
    pub line_start: u32,
    /// 1-based, inclusive last line of the chunk.
    pub line_end: u32,
    /// The exact decrypted chunk text.
    pub content: String,
    /// Cosine similarity of the query vector and the chunk vector, in `[-1, 1]`.
    pub score: f64,
}

/// Queries the extra-mode index for the chunks most similar to `text`.
///
/// The master key is supplied per call: it builds the encryptor that opens the store, so the
/// crate never holds a long-lived key. The flow is: embed the query, read and decrypt every
/// embedding for `(model, dim)`, score them with cosine similarity in memory, then hydrate the
/// top `top_k` chunk ids through [`Store::chunk_metadata`]. A `top_k` of `0` returns an empty
/// vector without touching the store or the embedder.
///
/// # Errors
///
/// Returns [`ExtraError::NothingEmbedded`] when the model has no registry row,
/// [`ExtraError::DimensionMismatch`] when the query vector's width differs from the registered
/// dimension, [`ExtraError::InvalidData`] when a stored line range does not fit a `u32` or the
/// provider returns no vector, and the storage, seam, provider, or `SQLite` errors wrapped by
/// [`ExtraError`].
pub fn query(
    config: &Config,
    db_key: &Key,
    master: &ExtraKey,
    embedder: &dyn Embedder,
    text: &str,
    top_k: usize,
) -> Result<Vec<ScoredChunk>> {
    if top_k == 0 {
        return Ok(Vec::new());
    }

    let store = db::extra_store(config, db_key, master)?;
    let model = embedder.model();
    let Some((dim, _metric)) = store.registered_model(model)? else {
        return Err(ExtraError::NothingEmbedded { model: model.to_owned() });
    };

    let vector = embedder
        .embed(&[text])?
        .into_iter()
        .next()
        .ok_or(ExtraError::InvalidData { reason: "embedder returned no query vector" })?;
    if vector.len() != usize::try_from(dim).unwrap_or(usize::MAX) {
        return Err(ExtraError::DimensionMismatch { expected: dim, actual: vector.len() });
    }

    let embeddings = store.list_embeddings(model, dim)?;
    let mut scored: Vec<(Vec<u8>, f64)> = embeddings
        .into_iter()
        .filter_map(|(chunk_id, candidate)| {
            cosine(&vector, &candidate).map(|score| (chunk_id, f64::from(score)))
        })
        .collect();
    scored.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    scored.truncate(top_k);

    hydrate(&store, config, db_key, &scored)
}

/// Hydrates the scored chunk ids through the store's decrypting metadata read.
fn hydrate(
    store: &Store,
    config: &Config,
    db_key: &Key,
    scored: &[(Vec<u8>, f64)],
) -> Result<Vec<ScoredChunk>> {
    if scored.is_empty() {
        return Ok(Vec::new());
    }
    let connection = db::open(&config.storage.path, db_key)?;
    let mut remaining: HashMap<Vec<u8>, f64> = HashMap::with_capacity(scored.len());
    let mut rowids = Vec::with_capacity(scored.len());
    for (chunk_id, score) in scored {
        // A concurrent delete can remove a chunk after scoring; leave it out instead of failing.
        if let Some(rowid) = db::rowid_for(&connection, chunk_id)? {
            rowids.push(rowid);
            remaining.insert(chunk_id.clone(), *score);
        }
    }
    let mut results = Vec::with_capacity(rowids.len());
    for row in store.chunk_metadata(&rowids)? {
        if let Some(score) = remaining.remove(&row.chunk_id) {
            results.push(scored_chunk(row, score)?);
        }
    }
    results.sort_by(|left, right| {
        right.score.total_cmp(&left.score).then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    Ok(results)
}

/// Converts a stored row into the public result, validating the line range.
fn scored_chunk(row: ChunkRow, score: f64) -> Result<ScoredChunk> {
    let line_start = u32::try_from(row.line_start)
        .map_err(|_| ExtraError::InvalidData { reason: "chunk line_start does not fit u32" })?;
    let line_end = u32::try_from(row.line_end)
        .map_err(|_| ExtraError::InvalidData { reason: "chunk line_end does not fit u32" })?;
    Ok(ScoredChunk {
        chunk_id: row.chunk_id,
        path: row.path,
        line_start,
        line_end,
        content: row.content,
        score,
    })
}

/// Cosine similarity, or `None` when the widths differ or either vector has zero norm.
fn cosine(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;
    for (left_value, right_value) in left.iter().zip(right) {
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if !denominator.is_finite() || denominator <= 0.0 {
        return None;
    }
    Some(dot / denominator)
}

#[cfg(test)]
mod tests {
    use super::cosine;

    #[test]
    fn cosine_handles_identical_orthogonal_and_zero_vectors() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]).expect("identical") - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).expect("orthogonal").abs() < 1e-6);
        assert!(cosine(&[0.0, 0.0], &[1.0, 0.0]).is_none());
        assert!(cosine(&[1.0], &[1.0, 0.0]).is_none());
    }
}
