// SPDX-License-Identifier: Apache-2.0

//! `braga_query` and `braga_string_free`: the retrieval half of the ABI.
//!
//! The hits are serialized as a JSON array of objects, one per [`RetrievedChunk`]. The `chunk_id`
//! is the lowercase hex of the 32-byte `SHA-256` digest, because JSON numbers are a poor carrier for
//! opaque bytes.

use std::{
    ffi::c_char,
    fmt::Write as _,
    panic::{self, AssertUnwindSafe},
};

use braga::retrieval::{self, RetrievedChunk};
use braga_storage::Store;
use serde::Serialize;

use crate::{
    abi::BragaResult,
    guard::{self, Failure},
    handle::BragaHandle,
    string,
};

/// The JSON shape of one hit; a local mirror so the core type stays free of serialization.
#[derive(Serialize)]
struct HitJson {
    /// Lowercase hex of the chunk's 32-byte `SHA-256` digest.
    chunk_id: String,
    /// Path of the file the chunk belongs to.
    path: String,
    /// 1-based, inclusive first line of the chunk.
    line_start: u32,
    /// 1-based, inclusive last line of the chunk.
    line_end: u32,
    /// The exact chunk text.
    content: String,
    /// Fused reciprocal-rank-fusion score.
    score: f64,
}

impl HitJson {
    /// Converts one core hit into its wire shape.
    fn from_hit(hit: &RetrievedChunk) -> Self {
        Self {
            chunk_id: hex(&hit.chunk_id),
            path: hit.path.clone(),
            line_start: hit.line_start,
            line_end: hit.line_end,
            content: hit.content.clone(),
            score: hit.score,
        }
    }
}

/// Renders `bytes` as lowercase hexadecimal.
fn hex(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// Queries the encrypted index and returns the top `top_k` hits as JSON.
///
/// `top_k` is clamped by the retrieval layer; `0` returns an empty array. The reserved
/// `key`/`key_len` slot is ignored in base mode.
///
/// # Safety
///
/// `handle` must be null or a live handle returned by [`braga_open`](crate::braga_open) that is not
/// used concurrently. `text` must be valid for reads of `text_len` bytes. `out_json` and
/// `out_json_len` must be non-null and writable, and `key` must be null or readable for `key_len`
/// bytes; it is never read here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn braga_query(
    handle: *mut BragaHandle,
    text: *const u8,
    text_len: usize,
    top_k: u32,
    _key: *const u8,
    _key_len: usize,
    out_json: *mut *mut c_char,
    out_json_len: *mut usize,
) -> BragaResult {
    guard::guard(|| {
        // SAFETY: the caller guarantees both out-parameters are non-null and writable.
        unsafe { string::reset_json_outputs(out_json, out_json_len) }?;
        // SAFETY: `handle` is either null, which `as_ref` maps to `None`, or a live handle.
        let handle = unsafe { handle.as_ref() }
            .ok_or_else(|| Failure::new(BragaResult::InvalidArg, "handle must not be NULL"))?;
        // SAFETY: the caller guarantees `text` is readable for `text_len` bytes.
        let text = unsafe { string::required(text, text_len, "text") }?;

        let hits = query(handle, text, top_k)?;
        let (buffer, len) = string::into_c(hits)?;
        // SAFETY: `reset_json_outputs` validated both pointers; they are still writable.
        unsafe { string::store_json_output(out_json, out_json_len, buffer, len) };
        Ok(())
    })
}

/// Opens a fresh store, runs the retrieval, and serializes the hits.
fn query(handle: &BragaHandle, text: &str, top_k: u32) -> Result<String, Failure> {
    let store = Store::open(&handle.config.storage.path, &handle.key)?;
    let limit = usize::try_from(top_k).unwrap_or(usize::MAX);
    let chunks = retrieval::query(&store, handle.embedder.as_ref(), text, limit)?;
    let hits: Vec<HitJson> = chunks.iter().map(HitJson::from_hit).collect();
    serde_json::to_string(&hits).map_err(Failure::from)
}

/// Frees a buffer returned by [`braga_query`] or [`braga_doctor`](crate::braga_doctor).
///
/// A null pointer is a no-op, so a wrapper may call this unconditionally on its output variable.
///
/// # Safety
///
/// `pointer` must be null or a buffer returned by [`braga_query`]/[`braga_doctor`](crate::braga_doctor) that has not
/// already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn braga_string_free(pointer: *mut c_char) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| unsafe { string::free(pointer) }));
}
