// SPDX-License-Identifier: Apache-2.0

//! HTTP embedding adapters for `bRAGa`.
//!
//! [`Embedder`] is the pipeline's text-in, vectors-out seam: implementations split a batch into
//! provider-sized requests, retry transient failures, and learn the model's vector width on the
//! first successful response. Two adapters cover the configured providers:
//!
//! * [`OpenAiCompatible`] — `POST` with `{"model": …, "input": [texts…]}`, reading
//!   `{"data":[{"index": i, "embedding": [f32…]}, …]}`. Rows are returned in `index` order.
//! * [`Ollama`] — `POST` with the same request body, reading `{"embeddings": [[f32…], …]}` in input
//!   order.
//!
//! Both constructors take the **full endpoint URL**; no path is appended, because compatible
//! vendors diverge (`…/v1/embeddings` for `OpenAI`, `…/api/embed` for `Ollama`).
//!
//! This is a leaf crate: it has no dependency on `braga`, `braga-storage`, or `braga-chunker`, and
//! therefore carries no content hashes or storage types.
//!
//! # Request policy
//!
//! [`embed`](Embedder::embed) accepts a batch of any size and splits it into
//! [`batch_size`](EmbedderOptions::batch_size) groups, one request per group. Transport errors,
//! `429`, and `500`/`502`/`503`/`504` are retried up to [`max_retries`](EmbedderOptions::max_retries)
//! times with `100ms * 2^n` backoff; every other non-`2xx` status fails immediately. Each request is
//! bounded by [`timeout`](EmbedderOptions::timeout).
//!
//! # Errors and redaction
//!
//! [`EmbedError`] keeps only a numeric HTTP status, a short static reason, or a transport message.
//! Response bodies are discarded and input text is never retained, so errors are safe to log.
//! [`dim`](Embedder::dim) is `None` until the first successful response pins the width; later
//! responses with a different width fail with [`EmbedError::DimensionMismatch`].

mod error;
mod http;
mod ollama;
mod openai;

use std::time::Duration;

pub use error::EmbedError;
pub use ollama::Ollama;
pub use openai::OpenAiCompatible;

/// Splits text into fixed-width vectors for a specific model.
///
/// Implementations are `Send + Sync` so the future scan worker can own one. `embed` accepts a batch
/// of any size; the implementation is responsible for splitting it into provider-sized requests.
pub trait Embedder: Send + Sync {
    /// Embeds `inputs` and returns one `f32` vector per input, in input order.
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError`] on transport failure, a non-`2xx` status, a malformed response, or a
    /// response whose vectors do not all share one dimension. No input text or response body is ever
    /// retained in an error.
    fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;

    /// The configured model name (`embedding.model`).
    fn model(&self) -> &str;

    /// The vector width, known only after the first successful [`embed`](Self::embed).
    fn dim(&self) -> Option<usize>;
}

/// Batching, retry, and timeout policy shared by the HTTP adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedderOptions {
    /// Provider-level request size; the adapter splits `embed` input into groups this large.
    pub batch_size: usize,
    /// Bounded retries for transport errors and 429/5xx, with exponential backoff.
    pub max_retries: u32,
    /// Per-attempt read/connect timeout.
    pub timeout: Duration,
}

impl Default for EmbedderOptions {
    fn default() -> Self {
        Self { batch_size: 32, max_retries: 3, timeout: Duration::from_secs(30) }
    }
}
