// SPDX-License-Identifier: Apache-2.0

//! The no-panic contract: every symbol returns a code for NULL, invalid UTF-8, or oversized input.
//!
//! These tests also keep the suite hermetic: they never hand `braga_open` a null configuration path
//! without a key, because that would resolve to the user's real `~/.braga` index.

use std::{ffi::c_char, ptr};

use crate::{
    ffi::{self, BragaResult, BragaStatusCode, Handle},
    harness::Fixture,
};

/// Bytes that are not valid UTF-8.
const INVALID_UTF8: [u8; 2] = [0xFF, 0xFE];

#[test]
fn null_handles_and_out_parameters_are_rejected() {
    assert_eq!(ffi::close(ptr::null_mut()), BragaResult::InvalidArg);
    assert_eq!(ffi::status(ptr::null()), BragaStatusCode::Error);
    assert_eq!(ffi::update(ptr::null_mut()), BragaResult::InvalidArg);

    let text = b"quick brown fox";
    let mut json: *mut c_char = ptr::null_mut();
    let mut len = 0_usize;

    // SAFETY: the reserved key slot is null and both out-parameters are writable.
    let code = unsafe {
        ffi::braga_query(
            ptr::null_mut(),
            text.as_ptr(),
            text.len(),
            1,
            ptr::null(),
            0,
            &raw mut json,
            &raw mut len,
        )
    };
    assert_eq!(code, BragaResult::InvalidArg);
    assert!(json.is_null());
    assert_eq!(len, 0);

    // SAFETY: both out-parameters are writable.
    let code = unsafe { ffi::braga_doctor(ptr::null(), &raw mut json, &raw mut len) };
    assert_eq!(code, BragaResult::InvalidArg);
    assert!(json.is_null());
    assert_eq!(len, 0);

    // A null out-parameter is rejected on either side of the pair.
    // SAFETY: `json` is writable; the null length pointer is the value under test.
    let code = unsafe {
        ffi::braga_query(
            ptr::null_mut(),
            text.as_ptr(),
            text.len(),
            1,
            ptr::null(),
            0,
            &raw mut json,
            ptr::null_mut(),
        )
    };
    assert_eq!(code, BragaResult::InvalidArg);
    // SAFETY: `len` is writable; the null JSON pointer is the value under test.
    let code = unsafe { ffi::braga_doctor(ptr::null(), ptr::null_mut(), &raw mut len) };
    assert_eq!(code, BragaResult::InvalidArg);

    // The error buffer is readable without a handle and is never null.
    let pointer = unsafe { ffi::braga_last_error(ptr::null()) };
    assert!(!pointer.is_null());
    assert!(!ffi::last_error().is_empty(), "the rejecting call must leave a message");
}

#[test]
fn open_and_init_reject_invalid_utf8_and_oversized_lengths() {
    let fixture = Fixture::new();
    let config = fixture.config().as_ptr();
    let config_len = fixture.config().len();
    let key = fixture.key().as_ptr();
    let key_len = fixture.key().len();
    // A non-NULL sentinel: every rejected call must overwrite it with NULL, as the header promises.
    let mut handle = ptr::without_provenance_mut::<Handle>(0x1);

    // SAFETY: every pointer is null or borrowed for the call, and every out-parameter is writable.
    unsafe {
        assert_eq!(
            ffi::braga_open(INVALID_UTF8.as_ptr(), 2, key, key_len, &raw mut handle),
            BragaResult::InvalidArg
        );
        assert!(handle.is_null(), "an invalid config_path must clear *out_handle");

        handle = ptr::without_provenance_mut::<Handle>(0x1);
        assert_eq!(
            ffi::braga_open(config, config_len, INVALID_UTF8.as_ptr(), 2, &raw mut handle),
            BragaResult::InvalidArg
        );
        assert!(handle.is_null(), "an invalid key_path must clear *out_handle");

        // An oversized length is rejected before any read.
        handle = ptr::without_provenance_mut::<Handle>(0x1);
        assert_eq!(
            ffi::braga_open(config, usize::MAX, ptr::null(), 0, &raw mut handle),
            BragaResult::InvalidArg
        );
        assert!(handle.is_null(), "an oversized path must clear *out_handle");
        // A null out-handle is rejected even with otherwise valid paths.
        assert_eq!(
            ffi::braga_open(config, config_len, key, key_len, ptr::null_mut()),
            BragaResult::InvalidArg
        );
        // `braga_init` rejects bad UTF-8 and oversized lengths without touching the filesystem.
        assert_eq!(
            ffi::braga_init(INVALID_UTF8.as_ptr(), 2, ptr::null(), 0),
            BragaResult::InvalidArg
        );
        assert_eq!(
            ffi::braga_init(INVALID_UTF8.as_ptr(), usize::MAX, ptr::null(), 0),
            BragaResult::InvalidArg
        );
    }
}

#[test]
fn query_rejects_invalid_text_and_survives_an_oversized_top_k() {
    let fixture = Fixture::new();
    let (code, handle) = ffi::open(Some(fixture.config()), Some(fixture.key()));
    assert_eq!(code, BragaResult::Ok);
    let mut json: *mut c_char = ptr::null_mut();
    let mut len = 0_usize;

    // SAFETY: the handle is live; each text buffer is either the constant or a null pointer.
    unsafe {
        assert_eq!(
            ffi::braga_query(
                handle,
                INVALID_UTF8.as_ptr(),
                2,
                1,
                ptr::null(),
                0,
                &raw mut json,
                &raw mut len
            ),
            BragaResult::InvalidArg
        );
        assert!(json.is_null());
        assert_eq!(
            ffi::braga_query(
                handle,
                INVALID_UTF8.as_ptr(),
                usize::MAX,
                1,
                ptr::null(),
                0,
                &raw mut json,
                &raw mut len
            ),
            BragaResult::InvalidArg
        );
        assert!(json.is_null());
        // A null text pointer and a zero-length text are both rejected.
        assert_eq!(
            ffi::braga_query(
                handle,
                ptr::null(),
                0,
                1,
                ptr::null(),
                0,
                &raw mut json,
                &raw mut len
            ),
            BragaResult::InvalidArg
        );
        let empty = b"";
        assert_eq!(
            ffi::braga_query(
                handle,
                empty.as_ptr(),
                0,
                1,
                ptr::null(),
                0,
                &raw mut json,
                &raw mut len
            ),
            BragaResult::InvalidArg
        );
    }
    assert!(json.is_null());

    // An oversized `top_k` is clamped by the retrieval layer; it must not abort.
    assert_eq!(ffi::update(handle), BragaResult::Ok);
    let (code, json) = ffi::query(handle, "quick brown fox", 1);
    assert_eq!(code, BragaResult::Ok);
    assert!(json.is_some(), "a normal-sized query must return JSON");

    // A maximal `top_k` may exceed the storage layer's KNN limit; it must still come back as a
    // result code with consistent outputs, never as a panic.
    let (code, json) = ffi::query(handle, "quick brown fox", u32::MAX);
    assert_ne!(code, BragaResult::Panic, "an oversized top_k must not panic");
    assert_eq!(json.is_some(), code == BragaResult::Ok, "outputs must match the result code");
    assert_eq!(ffi::close(handle), BragaResult::Ok);
}

#[test]
fn the_reserved_key_slot_is_ignored() {
    let fixture = Fixture::new();
    let (code, handle) = ffi::open(Some(fixture.config()), Some(fixture.key()));
    assert_eq!(code, BragaResult::Ok);

    // A non-NULL reserved key is accepted and ignored, even when it is not valid UTF-8.
    // SAFETY: `handle` is live and the reserved key is readable for its length.
    let code = unsafe { ffi::braga_update(handle, INVALID_UTF8.as_ptr(), INVALID_UTF8.len()) };
    assert_eq!(code, BragaResult::Ok);
    assert_eq!(ffi::status(handle), BragaStatusCode::UpToDate);

    let text = b"quick brown fox";
    let mut json: *mut c_char = ptr::null_mut();
    let mut len = 0_usize;
    // SAFETY: the handle is live, the text is readable, and both out-parameters are writable.
    let code = unsafe {
        ffi::braga_query(
            handle,
            text.as_ptr(),
            text.len(),
            1,
            INVALID_UTF8.as_ptr(),
            INVALID_UTF8.len(),
            &raw mut json,
            &raw mut len,
        )
    };
    assert_eq!(code, BragaResult::Ok);
    assert!(!json.is_null());
    assert!(len > 0);
    // SAFETY: the buffer came from `braga_query` and is freed exactly once.
    unsafe { ffi::braga_string_free(json) };
    assert_eq!(ffi::close(handle), BragaResult::Ok);
}

#[test]
fn string_free_accepts_null() {
    // SAFETY: a null pointer is a documented no-op.
    unsafe { ffi::braga_string_free(ptr::null_mut()) };
}
