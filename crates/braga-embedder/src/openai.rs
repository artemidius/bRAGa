// SPDX-License-Identifier: Apache-2.0

//! The `OpenAI`-compatible embeddings adapter.

use std::sync::OnceLock;

use serde_json::{Value, json};
use ureq::Agent;

use crate::{
    EmbedError, Embedder, EmbedderOptions,
    http::{build_agent, embed_in_batches, parse_vector, post_json},
};

/// Adapter for `OpenAI`'s `/v1/embeddings` and compatible endpoints.
pub struct OpenAiCompatible {
    agent: Agent,
    url: String,
    model: String,
    options: EmbedderOptions,
    dim: OnceLock<usize>,
}

impl OpenAiCompatible {
    /// Builds an adapter with default [`EmbedderOptions`].
    ///
    /// `url` is the full endpoint, for example `https://api.openai.com/v1/embeddings`; no path is
    /// appended.
    #[must_use]
    pub fn new(url: String, model: String) -> Self {
        Self::with_options(url, model, EmbedderOptions::default())
    }

    /// Builds an adapter with an explicit batching, retry, and timeout policy.
    ///
    /// `url` is the full endpoint, for example `https://api.openai.com/v1/embeddings`; no path is
    /// appended.
    #[must_use]
    pub fn with_options(url: String, model: String, options: EmbedderOptions) -> Self {
        let agent = build_agent(&options);
        Self { agent, url, model, options, dim: OnceLock::new() }
    }

    fn request(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let body = json!({ "model": self.model, "input": inputs });
        let response = post_json(&self.agent, &self.url, &body, &self.options)?;
        parse_response(&response, inputs.len())
    }
}

impl Embedder for OpenAiCompatible {
    fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        embed_in_batches(inputs, self.options.batch_size, &self.dim, |batch| self.request(batch))
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> Option<usize> {
        self.dim.get().copied()
    }
}

/// Parses `{"data":[{"index": i, "embedding": [...]}, …]}` into input order.
fn parse_response(response: &Value, expected: usize) -> Result<Vec<Vec<f32>>, EmbedError> {
    let data = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or(EmbedError::InvalidResponse { reason: "missing `data` array" })?;
    if data.len() != expected {
        return Err(EmbedError::InvalidResponse {
            reason: "`data` length does not match input count",
        });
    }
    let mut rows = Vec::with_capacity(expected);
    for entry in data {
        let index = entry
            .get("index")
            .and_then(Value::as_u64)
            .ok_or(EmbedError::InvalidResponse { reason: "missing `index`" })?;
        let index = usize::try_from(index)
            .map_err(|_| EmbedError::InvalidResponse { reason: "`index` is out of range" })?;
        let embedding = entry
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or(EmbedError::InvalidResponse { reason: "missing `embedding` array" })?;
        rows.push((index, parse_vector(embedding)?));
    }
    rows.sort_by_key(|(index, _)| *index);
    for (position, (index, _)) in rows.iter().enumerate() {
        if *index != position {
            return Err(EmbedError::InvalidResponse {
                reason: "`data` indices are not contiguous from zero",
            });
        }
    }
    Ok(rows.into_iter().map(|(_, vector)| vector).collect())
}
