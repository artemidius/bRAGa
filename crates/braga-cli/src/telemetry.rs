// SPDX-License-Identifier: Apache-2.0

//! Off-by-default `tracing` telemetry and the embed-call instrumented adapter.

use std::time::Instant;

use braga_embedder::{EmbedError, Embedder};
use tracing::Level;

/// Installs a stderr subscriber when `verbose` is set or `braga_log` has a value.
///
/// `BRAGA_LOG` is a truthy toggle, not a filter directive: any non-empty value enables the
/// subscriber. With neither set, nothing is installed and every telemetry call is a no-op.
pub(crate) fn init_subscriber(verbose: bool, braga_log: Option<&str>) {
    if !verbose && braga_log.is_none_or(str::is_empty) {
        return;
    }
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_max_level(Level::DEBUG).init();
}

/// Whole milliseconds since `started`, saturating instead of overflowing.
pub(crate) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Wraps an [`Embedder`] and emits one `embed.call` event per call.
///
/// The event carries only the model name, the input count, the status, and the elapsed time:
/// never the input text and never a provider response body.
pub(crate) struct InstrumentedEmbedder {
    inner: Box<dyn Embedder>,
}

impl InstrumentedEmbedder {
    /// Wraps `inner`.
    pub(crate) fn new(inner: Box<dyn Embedder>) -> Self {
        Self { inner }
    }
}

impl Embedder for InstrumentedEmbedder {
    fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let started = Instant::now();
        let result = self.inner.embed(inputs);
        let elapsed = elapsed_ms(started);
        let model = self.inner.model();
        let status = if result.is_ok() { "ok" } else { "error" };
        tracing::info!(
            model = %model,
            inputs = inputs.len(),
            status = status,
            elapsed_ms = elapsed,
            "embed.call"
        );
        result
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn dim(&self) -> Option<usize> {
        self.inner.dim()
    }
}
