// SPDX-License-Identifier: Apache-2.0

//! `braga_open`, `braga_close`, `braga_status`, and `braga_last_error`.

use std::{
    ffi::c_char,
    panic::{self, AssertUnwindSafe},
    ptr,
};

use braga::{config::Config, embedding::build_embedder};
use braga_storage::{Key, Store};

use crate::{
    abi::{BragaResult, BragaStatus},
    guard::{self, Failure},
    handle::BragaHandle,
    string,
};

/// A static empty message used if the thread-local buffer cannot be read.
static EMPTY: &[u8] = b"\0";

/// Opens the configured index and returns an opaque handle.
///
/// The configuration is resolved once and captured on the handle; a null `config_path` defers to
/// `BRAGA_TOML`, `BRAGA_CONFIG`, `BRAGA_*__*`, and the built-in defaults. A null `key_path` defers
/// to `BRAGA_KEY_FILE` and then `~/.braga/key`. Both paths must be valid UTF-8.
///
/// The store is opened and closed once before returning, so a wrong key fails here rather than on
/// the first operation.
///
/// # Safety
///
/// Each non-null pointer must be valid for reads of its length and remain so for the call.
/// `out_handle` must be null or point to a writable `braga_handle *`. On success the caller owns
/// the returned handle and must release it exactly once with [`braga_close`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn braga_open(
    config_path: *const u8,
    config_path_len: usize,
    key_path: *const u8,
    key_path_len: usize,
    out_handle: *mut *mut BragaHandle,
) -> BragaResult {
    guard::guard(|| {
        // SAFETY: the caller guarantees each non-null pointer is readable for its length.
        let config_path =
            unsafe { string::optional_path(config_path, config_path_len, "config_path") }?;
        // SAFETY: as above.
        let key_path = unsafe { string::optional_path(key_path, key_path_len, "key_path") }?;
        if out_handle.is_null() {
            return Err(Failure::new(BragaResult::InvalidArg, "out_handle must not be NULL"));
        }
        // SAFETY: `out_handle` is non-null and writable by contract, so an error path leaves it
        // explicitly null instead of dangling.
        unsafe { out_handle.write(ptr::null_mut()) };

        let config = Config::resolve(config_path)?;
        let key = Key::load(key_path)?;
        let embedder = build_embedder(&config.embedding)?;
        // Fail fast on a wrong key instead of on the first operation.
        Store::open(&config.storage.path, &key)?.close()?;

        let handle = Box::into_raw(Box::new(BragaHandle::new(config, key, embedder)));
        // SAFETY: as above; the handle stays valid until `braga_close`.
        unsafe { out_handle.write(handle) };
        Ok(())
    })
}

/// Frees a handle created by [`braga_open`].
///
/// # Safety
///
/// `handle` must be null or a live handle returned by [`braga_open`]. Freeing the same handle twice,
/// or using it after this call, is undefined behaviour.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn braga_close(handle: *mut BragaHandle) -> BragaResult {
    guard::guard(|| {
        if handle.is_null() {
            return Err(Failure::new(BragaResult::InvalidArg, "handle must not be NULL"));
        }
        // SAFETY: the caller guarantees `handle` came from `Box::into_raw` exactly once.
        drop(unsafe { Box::from_raw(handle) });
        Ok(())
    })
}

/// Reports the status of the last [`braga_open`] or [`braga_update`](crate::braga_update).
///
/// A null handle reports [`BragaStatus::Error`]; this function never fails otherwise.
///
/// # Safety
///
/// `handle` must be null or a live handle returned by [`braga_open`] and not yet closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn braga_status(handle: *const BragaHandle) -> BragaStatus {
    // SAFETY: `handle` is either null, which `as_ref` maps to `None`, or a live handle.
    let handle = panic::catch_unwind(AssertUnwindSafe(|| unsafe { handle.as_ref() }));
    match handle {
        Ok(Some(handle)) => handle.status(),
        Ok(None) | Err(_) => BragaStatus::Error,
    }
}

/// Returns the sanitized message of the most recent failure on the calling thread.
///
/// The pointer is library-owned: valid until the next fallible FFI call on this thread, never freed
/// by the caller, and empty when the most recent fallible call succeeded. `handle` is accepted for
/// interface uniformity and is never dereferenced.
#[unsafe(no_mangle)]
pub extern "C" fn braga_last_error(_handle: *const BragaHandle) -> *const c_char {
    panic::catch_unwind(AssertUnwindSafe(guard::last_error_ptr))
        .unwrap_or_else(|_| EMPTY.as_ptr().cast::<c_char>())
}
