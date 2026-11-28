// SPDX-License-Identifier: Apache-2.0

//! The opaque handle behind `braga_handle` and the state it owns.
//!
//! A handle captures what `braga_open` resolved once: the configuration, the database key, the
//! built embedder, and the last reported status. Every operation reopens its own
//! [`Store`](braga_storage::Store), exactly like the CLI, because a `Store` wraps a
//! `rusqlite::Connection` that is `Send` but not `Sync`.

use std::sync::atomic::{AtomicU8, Ordering};

use braga::config::Config;
use braga_embedder::Embedder;
use braga_storage::Key;

use crate::abi::BragaStatus;

/// The state behind a `braga_handle` pointer.
///
/// The type is opaque to C: the header only forward-declares it. It is created by
/// [`braga_open`](crate::braga_open), freed by [`braga_close`](crate::braga_close), and is **not**
/// thread-safe; a caller must use one handle from one thread at a time.
pub struct BragaHandle {
    /// The configuration resolved once by `braga_open`.
    pub(crate) config: Config,
    /// The database key loaded once by `braga_open`.
    pub(crate) key: Key,
    /// The embedding adapter built once by `braga_open`.
    pub(crate) embedder: Box<dyn Embedder>,
    /// The last reported status, stored as a [`BragaStatus`] discriminant.
    status: AtomicU8,
}

impl BragaHandle {
    /// Builds a handle whose status starts as [`BragaStatus::UpToDate`].
    pub(crate) fn new(config: Config, key: Key, embedder: Box<dyn Embedder>) -> Self {
        Self { config, key, embedder, status: AtomicU8::new(BragaStatus::UpToDate.as_u8()) }
    }

    /// The last reported status; a lock-free atomic load that never blocks.
    pub(crate) fn status(&self) -> BragaStatus {
        BragaStatus::from_u8(self.status.load(Ordering::Relaxed))
    }

    /// Publishes a new status for `braga_status` to read.
    pub(crate) fn set_status(&self, status: BragaStatus) {
        self.status.store(status.as_u8(), Ordering::Relaxed);
    }
}
