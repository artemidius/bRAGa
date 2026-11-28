// SPDX-License-Identifier: Apache-2.0

//! `braga_update`: reconcile the scan tree, then embed every chunk that is missing a vector.
//!
//! This mirrors the CLI's `scan` command: reconcile and embed are separate passes, and the second
//! pass reopens the database because the scanner owns its store while it reconciles. The operation
//! is explicit and blocking; v1 exposes no background or periodic scan.

use braga::{
    embedding::{EmbeddingPipeline, Metric},
    scanner::Scanner,
};
use braga_chunker::{ContentHash, content_hash};
use braga_storage::Store;

use crate::{
    abi::{BragaResult, BragaStatus},
    guard::{self, Failure},
    handle::BragaHandle,
};

/// Brings the index up to date and embeds uncovered chunks; blocks until the index is queryable.
///
/// The reserved `key`/`key_len` slot is ignored in base mode.
///
/// # Safety
///
/// `handle` must be null or a live handle returned by [`braga_open`](crate::braga_open) that is not
/// used concurrently. `key` must be null when `key_len` is zero and, when non-null, valid for reads
/// of `key_len` bytes; it is never read here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn braga_update(
    handle: *mut BragaHandle,
    _key: *const u8,
    _key_len: usize,
) -> BragaResult {
    guard::guard(|| {
        // SAFETY: `handle` is either null, which `as_ref` maps to `None`, or a live handle.
        let handle = unsafe { handle.as_ref() }
            .ok_or_else(|| Failure::new(BragaResult::InvalidArg, "handle must not be NULL"))?;
        update(handle)
    })
}

/// Runs the update sequence and mirrors its progress into the handle's status.
fn update(handle: &BragaHandle) -> Result<(), Failure> {
    handle.set_status(BragaStatus::Checking);
    match reconcile_and_embed(handle) {
        Ok(()) => {
            handle.set_status(BragaStatus::UpToDate);
            Ok(())
        }
        Err(failure) => {
            handle.set_status(BragaStatus::Error);
            Err(failure)
        }
    }
}

/// Reconciles the configured tree, reopens the store, and embeds the chunks that lack a vector.
fn reconcile_and_embed(handle: &BragaHandle) -> Result<(), Failure> {
    let config = &handle.config;
    {
        let store = Store::open(&config.storage.path, &handle.key)?;
        Scanner::new(config.scan.dir.clone(), store).reconcile()?;
    }
    handle.set_status(BragaStatus::Updating);

    let store = Store::open(&config.storage.path, &handle.key)?;
    let chunks = store.all_chunks()?;
    let pairs: Vec<(ContentHash, &str)> = chunks
        .iter()
        .map(|(_, content)| (content_hash(content.as_bytes()), content.as_str()))
        .collect();
    EmbeddingPipeline::new(Metric::Cosine).run(&store, handle.embedder.as_ref(), &pairs)?;
    Ok(())
}
