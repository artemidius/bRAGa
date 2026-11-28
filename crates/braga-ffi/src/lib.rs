// SPDX-License-Identifier: Apache-2.0

//! The frozen, versioned C ABI over `bRAGa`.
//!
//! This crate is a thin marshalling layer: it exports the C symbols declared in
//! [`include/braga.h`](https://github.com/artemidius/bRAGa/blob/main/crates/braga-ffi/include/braga.h)
//! and delegates to the existing `braga`, `braga-embedder`, `braga-chunker`, and `braga-storage`
//! APIs. It changes no core behaviour.
//!
//! # The freeze
//!
//! The header is hand-written and is the contract. [`ABI_VERSION`] is `1`; a wrapper must compare
//! [`braga_abi_version`] with the `BRAGA_ABI_VERSION` it was compiled against and refuse to load on
//! a mismatch. After this stage, a breaking change is a new version and a new symbol surface, never
//! a silent edit.
//!
//! * [`BragaHandle`] is opaque: no Rust type appears in the header. It is created by
//!   [`braga_open`], destroyed by [`braga_close`], and is **not** thread-safe (one thread per
//!   handle).
//! * Every string crosses as `const uint8_t *` plus an explicit `size_t` length. Lengths are
//!   authoritative, so chunk content may contain NUL bytes; paths and text must be valid UTF-8 and
//!   are rejected with [`BragaResult::InvalidArg`] otherwise.
//! * [`BragaResult`] is the result code. [`braga_last_error`] points into a thread-local,
//!   library-owned buffer of sanitized messages: no key material, no file content, no query text.
//! * Every exported function contains a Rust panic with `catch_unwind`; a panic is reported as
//!   [`BragaResult::Panic`] and never unwinds across the ABI.
//!
//! # Memory ownership
//!
//! Inputs are borrowed for the duration of the call. JSON output buffers returned by
//! [`braga_query`]/[`braga_doctor`] are library-allocated and released with
//! [`braga_string_free`]. The message returned by [`braga_last_error`] is library-owned and valid
//! until the next FFI call on that thread; it must never be freed.
//!
//! # Scope in v1
//!
//! The ABI deliberately exposes **no** background or periodic scan. `braga_update` is explicit and
//! blocking, mirroring the CLI's `scan`; a wrapper that wants periodic scanning drives it on its own
//! timer. `braga_status` reports the outcome of the last `braga_open`/`braga_update`. Reserving a
//! background surface is a future ABI bump, not a v1 feature.
//!
//! # Reserved key slot
//!
//! `braga_update` and `braga_query` take `key`/`key_len` parameters that are `NULL`/`0` in base
//! mode. A non-`NULL` value is ignored here; the slot exists so `braga-extra` can export the same
//! header and make it live without an ABI break.

mod abi;
mod doctor;
mod guard;
mod handle;
mod init;
mod open;
mod query;
mod string;
mod update;

pub use abi::{ABI_VERSION, BragaResult, BragaStatus};
pub use handle::BragaHandle;
pub use open::{braga_close, braga_last_error, braga_open, braga_status};

pub use abi::braga_abi_version;
pub use doctor::braga_doctor;
pub use init::braga_init;
pub use query::{braga_query, braga_string_free};
pub use update::braga_update;
