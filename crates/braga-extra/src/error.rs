// SPDX-License-Identifier: AGPL-3.0-or-later

//! The crate-wide error type.

use braga::{embedding::EmbeddingError, scanner::ScanError, seam::SeamError};
use braga_embedder::EmbedError;
use braga_storage::StoreError;
use thiserror::Error;

/// Error returned by every `braga-extra` entry point.
///
/// Messages never include chunk text, embedding vectors, query text, or key material.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExtraError {
    /// `model_registry` has no row for the embedder's model, so nothing can be scored.
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
    /// Stored data violates an invariant the caller can only fix by re-indexing.
    #[error("invalid stored data: {reason}")]
    InvalidData {
        /// A short static description of the violated invariant.
        reason: &'static str,
    },
    /// A storage operation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The reconcile pipeline failed.
    #[error(transparent)]
    Scan(#[from] ScanError),
    /// The embedding pipeline failed.
    #[error(transparent)]
    Embedding(#[from] EmbeddingError),
    /// The embedding provider failed while embedding the query.
    #[error(transparent)]
    Embed(#[from] EmbedError),
    /// The crypto seam rejected a record.
    #[error("crypto seam: {0}")]
    Seam(#[from] SeamError),
    /// A direct `SQLCipher` metadata query failed.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Convenience alias for the crate's result type.
pub type Result<T> = std::result::Result<T, ExtraError>;
