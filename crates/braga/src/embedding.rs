// SPDX-License-Identifier: Apache-2.0

//! The embedding pipeline: cache-filter, embed, and store each missing chunk.
//!
//! [`EmbeddingPipeline::run`] is idempotent. It reads the model's pinned shape from
//! `model_registry`, drops chunks that already have an embedding for `(chunk id, model, dim)`,
//! embeds the rest through a [`braga_embedder::Embedder`], and stores each vector with
//! [`Store::insert_embedding`]. A partial failure leaves the rows that committed; the next run
//! converges because the cache is content-addressed.
//!
//! The [`Metric`] is a pipeline parameter, not a configuration leaf: no CLI or status wiring
//! consumes it in this stage.

use braga_chunker::ContentHash;
use braga_embedder::{EmbedError, Embedder, Ollama, OpenAiCompatible};
use braga_storage::{Store, StoreError};
use thiserror::Error;

use crate::config::Embedding;

/// The single distance metric for a collection.
///
/// It is mirrored into `model_registry.metric` and the `vec0` table's `distance_metric`, so a later
/// change fails closed instead of mixing shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Cosine distance, the usual normalized-text metric.
    Cosine,
    /// Euclidean (L2) distance.
    L2,
    /// Inner product. Accepted by the frozen API; the pinned `sqlite-vec` implements only `cosine`
    /// and `l2`, so creating an inner-product table fails closed at the storage layer.
    InnerProduct,
}

impl Metric {
    /// The string stored in `model_registry.metric` and used as `vec0`'s `distance_metric`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cosine => "cosine",
            Self::L2 => "l2",
            Self::InnerProduct => "ip",
        }
    }
}

/// What one [`EmbeddingPipeline::run`] changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingOutcome {
    /// Chunks skipped because an embedding already existed for `(model, dim)`.
    pub cache_hits: usize,
    /// Chunks embedded and stored this run.
    pub embedded: usize,
    /// Model used.
    pub model: String,
    /// Dimension used.
    ///
    /// `0` is only possible for a run with no chunks and no previously registered model, where no
    /// dimension could be observed.
    pub dim: u32,
}

/// Embeds missing chunks into an encrypted [`Store`], with cache dedup and `model_registry`
/// pinning.
pub struct EmbeddingPipeline {
    metric: Metric,
}

impl EmbeddingPipeline {
    /// Builds a pipeline that pins every embedding to `metric`.
    #[must_use]
    pub const fn new(metric: Metric) -> Self {
        Self { metric }
    }

    /// Embeds the chunks that are not cached yet.
    ///
    /// The flow is: read the registered shape, reject a metric change, ensure the dimension's
    /// table, filter out cache hits, embed the remainder once, then store each vector. The adapter
    /// performs provider-level batching; this pipeline issues one [`Embedder::embed`] call per run.
    ///
    /// # Errors
    ///
    /// Returns [`EmbeddingError::Store`] when the model is pinned to a different metric or a
    /// storage operation fails, and [`EmbeddingError::Embed`] when the provider returns an error or
    /// a response that does not match the input count.
    pub fn run(
        &self,
        store: &Store,
        embedder: &dyn Embedder,
        chunks: &[(ContentHash, &str)],
    ) -> Result<EmbeddingOutcome, EmbeddingError> {
        let model = embedder.model();
        let metric = self.metric.as_str();
        let dim = match store.registered_model(model)? {
            Some((registered_dim, registered_metric)) => {
                if registered_metric != metric {
                    return Err(StoreError::ModelConflict { model: model.to_owned() }.into());
                }
                Some(registered_dim)
            }
            // A fresh adapter often has not learned its width yet; the first response pins it.
            None => embedder.dim().and_then(|dim| u32::try_from(dim).ok()),
        };

        if let Some(dim) = dim {
            store.ensure_embedding_table(dim, metric)?;
            store.register_model(model, dim, metric)?;
        }

        let mut missing = Vec::with_capacity(chunks.len());
        let mut cache_hits = 0;
        for &(chunk_id, text) in chunks {
            let cached = match dim {
                Some(dim) => store.embedding_exists(model, dim, chunk_id.as_bytes())?,
                // No table exists yet, so every chunk is missing by construction.
                None => false,
            };
            if cached {
                cache_hits += 1;
            } else {
                missing.push((chunk_id, text));
            }
        }

        if missing.is_empty() {
            return Ok(EmbeddingOutcome {
                cache_hits,
                embedded: 0,
                model: model.to_owned(),
                dim: dim.unwrap_or(0),
            });
        }

        let texts: Vec<&str> = missing.iter().map(|(_, text)| *text).collect();
        let vectors = embedder.embed(&texts)?;
        if vectors.len() != missing.len() {
            return Err(EmbedError::InvalidResponse {
                reason: "vector count does not match input count",
            }
            .into());
        }

        let dim = if let Some(dim) = dim {
            dim
        } else {
            let learned = u32::try_from(vectors[0].len())
                .map_err(|_| StoreError::InvalidVec0Parameter { parameter: "dim" })?;
            store.ensure_embedding_table(learned, metric)?;
            store.register_model(model, learned, metric)?;
            learned
        };

        let mut stored = 0;
        for ((chunk_id, _), vector) in missing.iter().zip(&vectors) {
            store.insert_embedding(chunk_id.as_bytes(), model, dim, vector)?;
            stored += 1;
        }

        Ok(EmbeddingOutcome { cache_hits, embedded: stored, model: model.to_owned(), dim })
    }
}

/// Builds an adapter from the `[embedding]` configuration section.
///
/// `embedding.url` is the **full endpoint**, exactly as the adapters document: for example
/// `…/v1/embeddings` for `openai`/`openai-compatible` and `…/api/embed` for `ollama`. No path is
/// appended. Empty `url`/`model` and unknown providers fail closed.
///
/// # Errors
///
/// Returns [`EmbeddingError::UnknownProvider`] for an empty or unrecognized provider and
/// [`EmbeddingError::MissingConfig`] when `url` or `model` is empty.
pub fn build_embedder(embedding: &Embedding) -> Result<Box<dyn Embedder>, EmbeddingError> {
    let provider = embedding.provider.as_str();
    match provider {
        "ollama" | "openai" | "openai-compatible" => {}
        _ => return Err(EmbeddingError::UnknownProvider { provider: provider.to_owned() }),
    }
    if embedding.url.is_empty() {
        return Err(EmbeddingError::MissingConfig { field: "url" });
    }
    if embedding.model.is_empty() {
        return Err(EmbeddingError::MissingConfig { field: "model" });
    }

    Ok(match provider {
        "ollama" => Box::new(Ollama::new(embedding.url.clone(), embedding.model.clone())),
        _ => Box::new(OpenAiCompatible::new(embedding.url.clone(), embedding.model.clone())),
    })
}

/// Error returned while building or running the embedding pipeline.
#[derive(Debug, Error)]
pub enum EmbeddingError {
    /// The configured provider is empty or not one of the supported names.
    #[error("unknown embedding provider `{provider}`")]
    UnknownProvider {
        /// The provider name as configured.
        provider: String,
    },
    /// A required `[embedding]` field is empty.
    #[error("embedding configuration is missing `{field}`")]
    MissingConfig {
        /// The missing field's name.
        field: &'static str,
    },
    /// A storage operation failed, including a pin violation.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The embedding provider failed.
    #[error(transparent)]
    Embed(#[from] EmbedError),
}

#[cfg(test)]
mod tests {
    use super::{EmbeddingError, Metric, build_embedder};
    use crate::config::Embedding;

    fn config(provider: &str, url: &str, model: &str) -> Embedding {
        Embedding { provider: provider.to_owned(), url: url.to_owned(), model: model.to_owned() }
    }

    #[test]
    fn metric_names_match_the_registry_values() {
        assert_eq!(Metric::Cosine.as_str(), "cosine");
        assert_eq!(Metric::L2.as_str(), "l2");
        assert_eq!(Metric::InnerProduct.as_str(), "ip");
    }

    #[test]
    fn build_embedder_validates_the_provider_and_required_fields() {
        let cases = [
            ("empty provider", config("", "http://localhost/embed", "m"), "provider"),
            ("unknown provider", config("cohere", "http://localhost/embed", "m"), "provider"),
            ("missing url", config("ollama", "", "m"), "url"),
            ("missing model", config("ollama", "http://localhost/embed", ""), "model"),
        ];

        for (name, embedding, field) in cases {
            let Err(error) = build_embedder(&embedding) else {
                panic!("{name}: expected an error");
            };
            match error {
                EmbeddingError::UnknownProvider { .. } => assert_eq!(field, "provider", "{name}"),
                EmbeddingError::MissingConfig { field: missing } => {
                    assert_eq!(missing, field, "{name}");
                }
                other => panic!("{name}: unexpected error: {other}"),
            }
        }
    }
}
