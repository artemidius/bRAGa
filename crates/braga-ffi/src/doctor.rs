// SPDX-License-Identifier: Apache-2.0

//! `braga_doctor`: the claims verifier, rendered as pretty-printed JSON.
//!
//! The report is the same [`DoctorReport`](braga::doctor::DoctorReport) the CLI prints, including
//! its `ok` conjunction and the ordered `checks` array. Doctor runs against a scratch directory, so
//! it cannot corrupt the live index; only its `store.open` check touches the configured path.

use std::ffi::c_char;

use braga::doctor;

use crate::{
    abi::BragaResult,
    guard::{self, Failure},
    handle::BragaHandle,
    string,
};

/// Verifies the stack's claims and returns the pretty-printed JSON report.
///
/// # Safety
///
/// `handle` must be null or a live handle returned by [`braga_open`](crate::braga_open) that is not
/// used concurrently. `out_json` and `out_json_len` must be non-null and writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn braga_doctor(
    handle: *const BragaHandle,
    out_json: *mut *mut c_char,
    out_json_len: *mut usize,
) -> BragaResult {
    guard::guard(|| {
        // SAFETY: the caller guarantees both out-parameters are non-null and writable.
        unsafe { string::reset_json_outputs(out_json, out_json_len) }?;
        // SAFETY: `handle` is either null, which `as_ref` maps to `None`, or a live handle.
        let handle = unsafe { handle.as_ref() }
            .ok_or_else(|| Failure::new(BragaResult::InvalidArg, "handle must not be NULL"))?;

        let report = doctor::doctor(&handle.config, &handle.key, handle.embedder.as_ref())?;
        let (buffer, len) = string::into_c(report.to_json()?)?;
        // SAFETY: `reset_json_outputs` validated both pointers; they are still writable.
        unsafe { string::store_json_output(out_json, out_json_len, buffer, len) };
        Ok(())
    })
}
