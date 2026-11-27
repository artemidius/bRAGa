// SPDX-License-Identifier: Apache-2.0

//! The `Ollama` embeddings adapter.

use std::sync::OnceLock;

use serde_json::{Value, json};
use ureq::Agent;

use crate::{
    EmbedError, Embedder, EmbedderOptions,
    http::{build_agent, embed_in_batches, parse_vector, post_json},
};

/// Adapter for `Ollama`'s `/api/embed` endpoint.
pub struct Ollama {
    agent: Agent,
    url: String,
    model: String,
    options: EmbedderOptions,
    dim: OnceLock<usize>,
}

impl Ollama {
    /// Builds an adapter with default [`EmbedderOptions`].
    ///
    /// `url` is the full endpoint, for example `http://localhost:11434/api/embed`; no path is
    /// appended.
    #[must_use]
    pub fn new(url: String, model: String) -> Self {
        Self::with_options(url, model, EmbedderOptions::default())
    }

    /// Builds an adapter with an explicit batching, retry, and timeout policy.
    ///
    /// `url` is the full endpoint, for example `http://localhost:11434/api/embed`; no path is
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

impl Embedder for Ollama {
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

/// Parses `{"embeddings": [[...], …]}` in input order.
fn parse_response(response: &Value, expected: usize) -> Result<Vec<Vec<f32>>, EmbedError> {
    let embeddings = response
        .get("embeddings")
        .and_then(Value::as_array)
        .ok_or(EmbedError::InvalidResponse { reason: "missing `embeddings` array" })?;
    if embeddings.len() != expected {
        return Err(EmbedError::InvalidResponse {
            reason: "`embeddings` length does not match input count",
        });
    }
    embeddings
        .iter()
        .map(|embedding| {
            embedding
                .as_array()
                .ok_or(EmbedError::InvalidResponse { reason: "`embeddings` entry is not an array" })
                .and_then(|values| parse_vector(values))
        })
        .collect()
}
