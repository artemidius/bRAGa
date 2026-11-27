// SPDX-License-Identifier: Apache-2.0

//! The crate's error type, with sanitized messages.

use std::fmt;

/// An embedding failure that never retains input text or a response body.
#[derive(Debug)]
pub enum EmbedError {
    /// A non-`2xx` HTTP status. Only the numeric code is kept, never the response body.
    Status {
        /// The numeric HTTP status code.
        code: u16,
    },
    /// A transport failure (DNS, connect, timeout, connection reset).
    Transport {
        /// The sanitized transport description; never a request or response body.
        message: String,
    },
    /// A `2xx` body that could not be parsed or was missing a required field.
    InvalidResponse {
        /// A short static description of what was wrong; never the body itself.
        reason: &'static str,
    },
    /// One or more vectors did not match the model's established dimension.
    DimensionMismatch {
        /// The width already established for the model.
        expected: usize,
        /// The width of the offending vector.
        actual: usize,
    },
}

impl fmt::Display for EmbedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status { code } => {
                write!(formatter, "embedding request returned HTTP status {code}")
            }
            Self::Transport { message } => {
                write!(formatter, "embedding transport error: {message}")
            }
            Self::InvalidResponse { reason } => {
                write!(formatter, "invalid embedding response: {reason}")
            }
            Self::DimensionMismatch { expected, actual } => {
                write!(formatter, "embedding dimension mismatch: expected {expected}, got {actual}")
            }
        }
    }
}

impl std::error::Error for EmbedError {}

/// Maps a `ureq` failure, discarding any response body.
pub(crate) fn map_ureq(error: ureq::Error) -> EmbedError {
    match error {
        ureq::Error::StatusCode(code) => EmbedError::Status { code },
        other => EmbedError::Transport { message: other.to_string() },
    }
}
