// SPDX-License-Identifier: Apache-2.0

//! Panic containment and the ABI's sanitized error channel.
//!
//! Every exported entry point runs its body through [`guard`]. A [`Failure`] becomes its result
//! code, a Rust panic becomes [`BragaResult::Panic`], and the message is copied into a thread-local
//! buffer that [`braga_last_error`](crate::braga_last_error) points into.
//!
//! # Sanitization
//!
//! The messages are the existing core error messages, which already redact key material and file
//! content: `StoreError` keeps only the numeric `SQLite` code, `KeyFileError` a path and a fixed
//! reason, and `EmbedError` a numeric status or a static reason. A `TOML` parse error is the one
//! detail that can echo file content, so it is replaced with a fixed message here.

use std::{
    cell::RefCell,
    ffi::{CString, c_char},
    io,
    panic::{self, AssertUnwindSafe},
};

use braga::{
    config::ConfigError, doctor::DoctorError, embedding::EmbeddingError, retrieval::QueryError,
    scanner::ScanError,
};
use braga_storage::{KeyFileError, StoreError};

use crate::abi::BragaResult;

/// The message reported for a caught panic; the payload is never forwarded.
const PANIC_MESSAGE: &str = "a Rust panic was caught inside bRAGa";

/// One ABI-level failure: the result code and the sanitized message to report.
pub(crate) struct Failure {
    /// The code returned across the ABI.
    pub(crate) result: BragaResult,
    /// The sanitized message stored for `braga_last_error`.
    pub(crate) message: String,
}

impl Failure {
    /// Builds a failure with an explicit code and message.
    pub(crate) fn new(result: BragaResult, message: impl Into<String>) -> Self {
        Self { result, message: message.into() }
    }
}

impl From<ConfigError> for Failure {
    fn from(error: ConfigError) -> Self {
        match error {
            // A `TOML` parser message can quote the document, so only a fixed reason crosses.
            ConfigError::Toml(_) => Self::new(BragaResult::Config, "invalid configuration"),
            other => Self::new(BragaResult::Config, other.to_string()),
        }
    }
}

impl From<KeyFileError> for Failure {
    fn from(error: KeyFileError) -> Self {
        Self::new(BragaResult::Key, error.to_string())
    }
}

impl From<StoreError> for Failure {
    fn from(error: StoreError) -> Self {
        Self::new(BragaResult::Store, error.to_string())
    }
}

impl From<EmbeddingError> for Failure {
    fn from(error: EmbeddingError) -> Self {
        match &error {
            EmbeddingError::Store(source) => Self::new(BragaResult::Store, source.to_string()),
            EmbeddingError::Embed(source) => Self::new(BragaResult::Embed, source.to_string()),
            _ => Self::new(BragaResult::Embed, format!("embedder build failed: {error}")),
        }
    }
}

impl From<QueryError> for Failure {
    fn from(error: QueryError) -> Self {
        match &error {
            QueryError::Store(source) => Self::new(BragaResult::Store, source.to_string()),
            QueryError::Embed(source) => Self::new(BragaResult::Embed, source.to_string()),
            QueryError::NothingEmbedded { .. } => {
                Self::new(BragaResult::NotFound, error.to_string())
            }
            QueryError::DimensionMismatch { .. } => {
                Self::new(BragaResult::Embed, error.to_string())
            }
            QueryError::InvalidData { .. } => Self::new(BragaResult::Store, error.to_string()),
        }
    }
}

impl From<ScanError> for Failure {
    fn from(error: ScanError) -> Self {
        match &error {
            ScanError::Io(source) => {
                Self::new(BragaResult::Io, format!("scan I/O error: {source}"))
            }
            ScanError::Store(source) => Self::new(BragaResult::Store, source.to_string()),
            _ => Self::new(BragaResult::Io, error.to_string()),
        }
    }
}

impl From<DoctorError> for Failure {
    fn from(error: DoctorError) -> Self {
        Self::new(BragaResult::Io, error.to_string())
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::new(BragaResult::Io, format!("I/O error: {error}"))
    }
}

impl From<serde_json::Error> for Failure {
    fn from(_error: serde_json::Error) -> Self {
        Self::new(BragaResult::Oom, "failed to serialize the JSON output")
    }
}

/// Runs `body` behind a panic barrier and records a failure in the thread-local buffer.
///
/// The buffer is cleared first: a wrapper must never see a stale message from an earlier call.
pub(crate) fn guard(body: impl FnOnce() -> Result<(), Failure>) -> BragaResult {
    set_message("");
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => BragaResult::Ok,
        Ok(Err(failure)) => {
            set_message(&failure.message);
            failure.result
        }
        Err(_payload) => {
            set_message(PANIC_MESSAGE);
            BragaResult::Panic
        }
    }
}

thread_local! {
    /// The sanitized message of the most recent failure on this thread.
    static LAST_ERROR: RefCell<CString> = RefCell::new(empty());
}

/// Allocates the empty message that seeds the thread-local buffer.
fn empty() -> CString {
    CString::new("").expect("an empty CString is always valid")
}

/// Replaces the thread-local message, replacing interior NUL bytes so the buffer stays a C string.
fn set_message(message: &str) {
    let sanitized: String =
        message.chars().map(|character| if character == '\0' { '?' } else { character }).collect();
    let value = CString::new(sanitized).unwrap_or_else(|_| empty());
    LAST_ERROR.with(|slot| *slot.borrow_mut() = value);
}

/// Points at the thread-local message buffer for `braga_last_error`.
///
/// The pointer is valid until the next guarded call on this thread replaces the buffer.
pub(crate) fn last_error_ptr() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}
