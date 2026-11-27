// SPDX-License-Identifier: Apache-2.0

//! Encrypted storage spike for `bRAGa`.
//!
//! This internal crate de-risks the riskiest part of the storage build before a schema or public
//! storage API is committed: `SQLCipher` through `rusqlite`'s `bundled-sqlcipher` feature, the
//! `sqlite-vec` static auto-extension, and FTS5.
//!
//! The only API is the narrow [`store::Store`] facade. `rusqlite` and `sqlite-vec` types never
//! cross this crate boundary. Key management, schema v1, and migrations land in Stage 4.

pub mod key;
pub mod store;
