// SPDX-License-Identifier: Apache-2.0

//! Hand-written mirror of `include/braga.h`, plus thin helpers for the test bodies.
//!
//! The declarations below are typed from the header, not from the crate's Rust API: this is the
//! consumer side of the contract. `braga_handle` stays opaque as `c_void`.

use std::{
    ffi::{CStr, c_char, c_void},
    ptr, slice, str,
};

/// `braga_handle` is opaque; the tests only pass pointers to it around.
pub(crate) type Handle = c_void;

/// Mirror of the header's `braga_result`.
// The whole discriminant set is part of the contract, even where a test does not name it.
#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BragaResult {
    /// The call succeeded.
    Ok = 0,
    /// A null pointer, bad UTF-8, bad length, or null out-parameter.
    InvalidArg = 1,
    /// Configuration resolution or validation failed.
    Config = 2,
    /// The key file could not be loaded.
    Key = 3,
    /// A storage error.
    Store = 4,
    /// An embedder build or provider failure.
    Embed = 5,
    /// Nothing is embedded for the configured model.
    NotFound = 6,
    /// An I/O error outside the store.
    Io = 7,
    /// A caught Rust panic.
    Panic = 8,
    /// An allocation or serialization failure.
    Oom = 9,
}

/// Mirror of the header's `braga_status_code`.
// The whole discriminant set is part of the contract, even where a test does not name it.
#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BragaStatusCode {
    /// The index matches the scan directory.
    UpToDate = 0,
    /// An update is in flight and has not written yet.
    Checking = 1,
    /// An update is writing or embedding.
    Updating = 2,
    /// The last open or update failed.
    Error = 3,
}

unsafe extern "C" {
    pub(crate) fn braga_abi_version() -> u32;
    pub(crate) fn braga_open(
        config_path: *const u8,
        config_path_len: usize,
        key_path: *const u8,
        key_path_len: usize,
        out_handle: *mut *mut Handle,
    ) -> BragaResult;
    pub(crate) fn braga_close(handle: *mut Handle) -> BragaResult;
    pub(crate) fn braga_status(handle: *const Handle) -> BragaStatusCode;
    pub(crate) fn braga_last_error(handle: *const Handle) -> *const c_char;
    pub(crate) fn braga_update(handle: *mut Handle, key: *const u8, key_len: usize) -> BragaResult;
    pub(crate) fn braga_query(
        handle: *mut Handle,
        text: *const u8,
        text_len: usize,
        top_k: u32,
        key: *const u8,
        key_len: usize,
        out_json: *mut *mut c_char,
        out_json_len: *mut usize,
    ) -> BragaResult;
    pub(crate) fn braga_doctor(
        handle: *const Handle,
        out_json: *mut *mut c_char,
        out_json_len: *mut usize,
    ) -> BragaResult;
    pub(crate) fn braga_init(
        config_path: *const u8,
        config_path_len: usize,
        key_path: *const u8,
        key_path_len: usize,
    ) -> BragaResult;
    pub(crate) fn braga_string_free(pointer: *mut c_char);
}

/// Opens a handle from optional UTF-8 paths.
pub(crate) fn open(config: Option<&str>, key: Option<&str>) -> (BragaResult, *mut Handle) {
    let (config_ptr, config_len): (*const u8, usize) =
        config.map_or((ptr::null(), 0), |path| (path.as_ptr(), path.len()));
    let (key_ptr, key_len): (*const u8, usize) =
        key.map_or((ptr::null(), 0), |path| (path.as_ptr(), path.len()));
    let mut handle = ptr::null_mut();
    // SAFETY: each pointer either is null or borrows a live Rust string, and `handle` is writable.
    let code = unsafe { braga_open(config_ptr, config_len, key_ptr, key_len, &raw mut handle) };
    (code, handle)
}

/// Closes a handle.
pub(crate) fn close(handle: *mut Handle) -> BragaResult {
    // SAFETY: the caller passes a live handle or NULL, both handled by the library.
    unsafe { braga_close(handle) }
}

/// Reads the status of a handle.
pub(crate) fn status(handle: *const Handle) -> BragaStatusCode {
    // SAFETY: the caller passes a live handle or NULL, both handled by the library.
    unsafe { braga_status(handle) }
}

/// Updates the index with no reserved key.
pub(crate) fn update(handle: *mut Handle) -> BragaResult {
    // SAFETY: the caller passes a live handle or NULL, both handled by the library.
    unsafe { braga_update(handle, ptr::null(), 0) }
}

/// Queries the index and returns the JSON output, if the library produced one.
pub(crate) fn query(handle: *mut Handle, text: &str, top_k: u32) -> (BragaResult, Option<String>) {
    let mut json: *mut c_char = ptr::null_mut();
    let mut len = 0_usize;
    // SAFETY: `text` is a live Rust string and both out-parameters are writable.
    let code = unsafe {
        braga_query(
            handle,
            text.as_ptr(),
            text.len(),
            top_k,
            ptr::null(),
            0,
            &raw mut json,
            &raw mut len,
        )
    };
    (code, take_json(json, len))
}

/// Runs the doctor and returns the JSON output, if the library produced one.
pub(crate) fn doctor(handle: *const Handle) -> (BragaResult, Option<String>) {
    let mut json: *mut c_char = ptr::null_mut();
    let mut len = 0_usize;
    // SAFETY: both out-parameters are writable.
    let code = unsafe { braga_doctor(handle, &raw mut json, &raw mut len) };
    (code, take_json(json, len))
}

/// Copies the thread-local error message produced by the most recent call.
pub(crate) fn last_error() -> String {
    // SAFETY: the returned pointer is NUL-terminated and library-owned; it is not freed here.
    let pointer = unsafe { braga_last_error(ptr::null()) };
    if pointer.is_null() {
        return String::new();
    }
    // SAFETY: `braga_last_error` always returns a valid NUL-terminated C string.
    unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned()
}

/// Copies a library JSON buffer and frees it; `None` for the `NULL` an error path leaves behind.
fn take_json(pointer: *mut c_char, len: usize) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the library returned a live buffer of exactly `len` bytes plus its terminator.
    let text = unsafe { str::from_utf8(slice::from_raw_parts(pointer.cast::<u8>(), len)) }
        .expect("JSON output is valid UTF-8")
        .to_owned();
    // SAFETY: the buffer came from the library and is freed exactly once.
    unsafe { braga_string_free(pointer) };
    Some(text)
}
