// SPDX-License-Identifier: AGPL-3.0-or-later

//! The extra-mode write path: reconcile the scan directory and embed missing chunks.
//!
//! [`update`] reuses the Apache pipeline unchanged. It opens the store with an encryptor built
//! from the caller's master key, runs [`Scanner::reconcile`], reopens the store, reads every chunk
//! through the seam (decrypting it), and feeds the texts to [`EmbeddingPipeline::run`], which
//! encrypts the new vectors on the way in.
//!
//! Chunk text is sent to the configured embedding provider here. Prefer a local provider such as
//! Ollama when the corpus is sensitive; see the crate docs for the residual remote-provider risk.

use braga::{
    config::Config,
    embedding::{EmbeddingOutcome, EmbeddingPipeline, Metric},
    scanner::{ScanOutcome, Scanner},
};
use braga_chunker::{ContentHash, content_hash};
use braga_embedder::Embedder;
use braga_storage::Key;

use crate::{db, error::Result, key::ExtraKey};

/// What one [`update`] changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOutcome {
    /// Files added, modified, deleted, skipped, or unchanged by the reconcile.
    pub scan: ScanOutcome,
    /// Chunks embedded, skipped as cache hits, and the model's shape.
    pub embedding: EmbeddingOutcome,
}

/// Reconciles the configured directory and embeds every chunk that lacks an embedding.
///
/// The master key is supplied per call. Reconcile and embedding are separate passes over the same
/// store, exactly as in the base CLI, because the scanner owns its store while it reconciles. The
/// embedding cache is content-addressed, so a second run with no new chunks is a full cache hit.
///
/// # Errors
///
/// Returns the scan, storage, seam, and embedding errors wrapped by [`crate::ExtraError`].
pub fn update(
    config: &Config,
    db_key: &Key,
    master: &ExtraKey,
    embedder: &dyn Embedder,
) -> Result<UpdateOutcome> {
    let store = db::extra_store(config, db_key, master)?;
    let scan = Scanner::new(config.scan.dir.clone(), store).reconcile()?;

    let store = db::extra_store(config, db_key, master)?;
    let chunks = store.all_chunks()?;
    let items: Vec<(ContentHash, &str)> = chunks
        .iter()
        .map(|(_, content)| (content_hash(content.as_bytes()), content.as_str()))
        .collect();
    let embedding = EmbeddingPipeline::new(Metric::Cosine).run(&store, embedder, &items)?;

    Ok(UpdateOutcome { scan, embedding })
}
