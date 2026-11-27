// SPDX-License-Identifier: Apache-2.0

//! Shared HTTP transport: agent construction, bounded retries, and response shaping.

use std::{sync::OnceLock, thread, time::Duration};

use serde_json::Value;
use ureq::Agent;

use crate::{EmbedError, EmbedderOptions, error::map_ureq};

/// Builds an agent whose requests share the configured timeout.
pub(crate) fn build_agent(options: &EmbedderOptions) -> Agent {
    Agent::config_builder().timeout_global(Some(options.timeout)).build().into()
}

/// Posts `body` as JSON and parses a JSON response, retrying transient failures.
pub(crate) fn post_json(
    agent: &Agent,
    url: &str,
    body: &Value,
    options: &EmbedderOptions,
) -> Result<Value, EmbedError> {
    let mut retries = 0_u32;
    loop {
        match agent.post(url).send_json(body) {
            Ok(mut response) => {
                return response.body_mut().read_json::<Value>().map_err(|_| {
                    EmbedError::InvalidResponse { reason: "response body is not valid JSON" }
                });
            }
            Err(error) => {
                let failure = map_ureq(error);
                if retries >= options.max_retries || !is_retryable(&failure) {
                    return Err(failure);
                }
                thread::sleep(backoff(retries));
                retries += 1;
            }
        }
    }
}

/// Embeds `inputs` in `batch_size` groups, pinning the dimension from the first response.
pub(crate) fn embed_in_batches<F>(
    inputs: &[&str],
    batch_size: usize,
    dim: &OnceLock<usize>,
    mut request: F,
) -> Result<Vec<Vec<f32>>, EmbedError>
where
    F: FnMut(&[&str]) -> Result<Vec<Vec<f32>>, EmbedError>,
{
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let mut vectors = Vec::with_capacity(inputs.len());
    for batch in inputs.chunks(batch_size.max(1)) {
        let mut batch_vectors = request(batch)?;
        pin_dimension(dim, &batch_vectors)?;
        vectors.append(&mut batch_vectors);
    }
    Ok(vectors)
}

/// Parses a JSON array of numbers into an `f32` vector.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // embeddings are `f32`
pub(crate) fn parse_vector(values: &[Value]) -> Result<Vec<f32>, EmbedError> {
    values
        .iter()
        .map(|value| value.as_f64().map(|number| number as f32))
        .collect::<Option<Vec<f32>>>()
        .ok_or(EmbedError::InvalidResponse { reason: "embedding contains a non-numeric value" })
}

/// Verifies a response's widths against the learned dimension, learning it on first use.
fn pin_dimension(dim: &OnceLock<usize>, vectors: &[Vec<f32>]) -> Result<(), EmbedError> {
    let Some(first) = vectors.first() else {
        return Ok(());
    };
    let width = first.len();
    if let Some(expected) = dim.get()
        && *expected != width
    {
        return Err(EmbedError::DimensionMismatch { expected: *expected, actual: width });
    }
    for vector in vectors {
        if vector.len() != width {
            return Err(EmbedError::DimensionMismatch { expected: width, actual: vector.len() });
        }
    }
    let _ = dim.set(width);
    match dim.get() {
        Some(expected) if *expected != width => {
            Err(EmbedError::DimensionMismatch { expected: *expected, actual: width })
        }
        _ => Ok(()),
    }
}

/// Whether a failure is transient enough to retry.
fn is_retryable(error: &EmbedError) -> bool {
    match error {
        EmbedError::Transport { .. } => true,
        EmbedError::Status { code } => matches!(code, 429 | 500 | 502 | 503 | 504),
        EmbedError::InvalidResponse { .. } | EmbedError::DimensionMismatch { .. } => false,
    }
}

/// `100ms * 2^retries`, saturating instead of overflowing.
fn backoff(retries: u32) -> Duration {
    Duration::from_millis(100).saturating_mul(2_u32.saturating_pow(retries))
}
