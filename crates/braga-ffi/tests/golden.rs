// SPDX-License-Identifier: Apache-2.0

//! Golden conformance tests for the frozen C ABI.
//!
//! Nothing here imports a Rust item from `braga-ffi`: [`ffi`] re-declares every symbol and type
//! from `include/braga.h`, so any drift between the header and the library is a compile or link
//! error. Every symbol in the header is called somewhere in this suite, which is what makes the
//! whole surface link.
//!
//! The tests are hermetic: [`harness`] serves a loopback `OpenAI`-compatible embedding endpoint, so
//! the `open → update → query → doctor` round-trip needs no external network.

// Link the library for its C symbols only; no Rust API item is imported from it.
use braga_ffi as _;

// The suite is split by responsibility; the files live beside this one in `tests/golden/`.
#[path = "golden/ffi.rs"]
mod ffi;
#[path = "golden/harness.rs"]
mod harness;
#[path = "golden/no_panic.rs"]
mod no_panic;
#[path = "golden/semantics.rs"]
mod semantics;

/// The ABI version is frozen at 1, and the version symbol is callable through the header.
#[test]
fn abi_version_is_frozen() {
    assert_eq!(unsafe { ffi::braga_abi_version() }, 1);
}
