// SPDX-License-Identifier: Apache-2.0

//! Caller-provided UTF-8 inputs and library-owned NUL-terminated outputs.
//!
//! Every input is a pointer plus an explicit length. The length is authoritative, a null pointer is
//! [`BragaResult::InvalidArg`] unless the parameter is documented as optional, and invalid UTF-8 is
//! always [`BragaResult::InvalidArg`].

use std::{
    ffi::{CString, c_char},
    path::Path,
    ptr, slice, str,
};

use crate::{abi::BragaResult, guard::Failure};

/// Borrows a required, non-empty UTF-8 string from caller memory.
///
/// # Safety
///
/// `pointer` must be null or valid for reads of `len` bytes, and the caller must keep those bytes
/// alive and unaliased for the lifetime the result is used.
pub(crate) unsafe fn required<'a>(
    pointer: *const u8,
    len: usize,
    field: &'static str,
) -> Result<&'a str, Failure> {
    let bytes = unsafe { bytes(pointer, len, field) }?;
    let text = str::from_utf8(bytes).map_err(|_| invalid(field, "is not valid UTF-8"))?;
    if text.is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    Ok(text)
}

/// Borrows an optional UTF-8 path; a null pointer or a zero length means "not supplied".
///
/// # Safety
///
/// `pointer` must be null or valid for reads of `len` bytes, and the caller must keep those bytes
/// alive and unaliased for the lifetime the result is used.
pub(crate) unsafe fn optional_path<'a>(
    pointer: *const u8,
    len: usize,
    field: &'static str,
) -> Result<Option<&'a Path>, Failure> {
    if pointer.is_null() || len == 0 {
        return Ok(None);
    }
    let bytes = unsafe { bytes(pointer, len, field) }?;
    let text = str::from_utf8(bytes).map_err(|_| invalid(field, "is not valid UTF-8"))?;
    Ok(Some(Path::new(text)))
}

/// Borrows `len` bytes from caller memory after checking the pointer and the length.
///
/// # Safety
///
/// `pointer` must be valid for reads of `len` bytes.
unsafe fn bytes<'a>(
    pointer: *const u8,
    len: usize,
    field: &'static str,
) -> Result<&'a [u8], Failure> {
    if pointer.is_null() {
        return Err(invalid(field, "must not be NULL"));
    }
    if len > isize::MAX.unsigned_abs() {
        return Err(invalid(field, "length is out of range"));
    }
    // SAFETY: the caller guarantees `pointer` is valid for `len` reads, and the length bound above
    // keeps the slice inside what `from_raw_parts` can describe.
    Ok(unsafe { slice::from_raw_parts(pointer, len) })
}

/// Builds an invalid-argument failure that names the offending parameter.
fn invalid(field: &'static str, reason: &'static str) -> Failure {
    Failure::new(BragaResult::InvalidArg, format!("{field} {reason}"))
}

/// Moves `text` into a NUL-terminated C buffer, returning its pointer and byte length.
///
/// The length excludes the terminator. The pointer must be released with
/// [`braga_string_free`](crate::braga_string_free), never with `free`.
pub(crate) fn into_c(text: String) -> Result<(*mut c_char, usize), Failure> {
    let len = text.len();
    let buffer = CString::new(text)
        .map_err(|_| Failure::new(BragaResult::Oom, "output contains an interior NUL byte"))?;
    Ok((buffer.into_raw(), len))
}

/// Frees a buffer returned by [`into_c`]; a null pointer is a documented no-op.
///
/// # Safety
///
/// `pointer` must be null or a pointer returned by [`into_c`] that has not been freed yet.
pub(crate) unsafe fn free(pointer: *mut c_char) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: the caller guarantees the pointer came from `CString::into_raw`.
    drop(unsafe { CString::from_raw(pointer) });
}

/// Validates the JSON out-parameters and resets them to `NULL`/`0`.
///
/// # Safety
///
/// Both pointers must be non-null and writable by the caller.
pub(crate) unsafe fn reset_json_outputs(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> Result<(), Failure> {
    if out_json.is_null() || out_len.is_null() {
        return Err(Failure::new(
            BragaResult::InvalidArg,
            "out_json and out_json_len must not be NULL",
        ));
    }
    // SAFETY: both out-parameters are non-null and writable by contract; an error path therefore
    // always leaves the caller with an explicit `NULL`.
    unsafe {
        out_json.write(ptr::null_mut());
        out_len.write(0);
    }
    Ok(())
}

/// Stores a JSON buffer in out-parameters that [`reset_json_outputs`] validated.
///
/// # Safety
///
/// Both pointers must have been validated by [`reset_json_outputs`] and must still be writable.
pub(crate) unsafe fn store_json_output(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
    buffer: *mut c_char,
    len: usize,
) {
    // SAFETY: the caller guarantees writability for the duration of the call.
    unsafe {
        out_json.write(buffer);
        out_len.write(len);
    }
}
