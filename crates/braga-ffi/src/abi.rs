// SPDX-License-Identifier: Apache-2.0

//! The frozen ABI version and the result and status enumerations.
//!
//! The discriminants are part of the contract: they are the values in `include/braga.h`, and a
//! wrapper maps them directly.

use std::panic;

/// The ABI version implemented by this library.
///
/// A wrapper compares this with the `BRAGA_ABI_VERSION` it was compiled against and refuses to load
/// on a mismatch.
pub const ABI_VERSION: u32 = 1;

/// The result code returned by every fallible entry point.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BragaResult {
    /// The call succeeded.
    Ok = 0,
    /// A null pointer, bad UTF-8, bad length, or null out-parameter.
    InvalidArg = 1,
    /// Configuration resolution or validation failed.
    Config = 2,
    /// The key file could not be loaded.
    Key = 3,
    /// A storage error: a wrong key, an I/O failure, or a numeric `SQLite` code.
    Store = 4,
    /// The embedder could not be built, or the provider failed.
    Embed = 5,
    /// Nothing is embedded for the configured model, so a query cannot run.
    NotFound = 6,
    /// An I/O error outside the store.
    Io = 7,
    /// A Rust panic was caught inside the library.
    Panic = 8,
    /// An allocation or JSON serialization failed.
    Oom = 9,
}

/// The reconciler state reported by [`braga_status`](crate::braga_status).
///
/// The header calls this C type `braga_status_code`: C and C++ share one ordinary namespace between
/// typedefs and functions, and `braga_status` is the function's name. The `BRAGA_STATUS_*`
/// constants and the discriminants below are unchanged.
///
/// Like the core [`DataStatus`](braga::status::DataStatus), the intermediate states are best-effort
/// observability hints: a fast reconcile may complete before a caller observes them.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BragaStatus {
    /// The index matches the scan directory and no update is in flight.
    UpToDate = 0,
    /// An update is in flight and has not written to the index yet.
    Checking = 1,
    /// The in-flight update has written to the index and is embedding or finishing.
    Updating = 2,
    /// The most recent open or update failed.
    Error = 3,
}

impl BragaStatus {
    /// Decodes the value stored in the handle's atomic; out-of-range values fail closed.
    pub(crate) const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::UpToDate,
            1 => Self::Checking,
            2 => Self::Updating,
            _ => Self::Error,
        }
    }

    /// The value stored in the handle's atomic.
    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Returns [`ABI_VERSION`].
#[unsafe(no_mangle)]
pub extern "C" fn braga_abi_version() -> u32 {
    panic::catch_unwind(|| ABI_VERSION).unwrap_or(0)
}
