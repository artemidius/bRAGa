// SPDX-License-Identifier: Apache-2.0

//! Encrypted storage foundation for `bRAGa`.
//!
//! This crate owns three things:
//!
//! * **Key management** — [`Key`] generates a 256-bit key from the operating system `CSPRNG`,
//!   loads it from a 32-byte keyfile, and persists it without clobbering an existing file. Every
//!   error type in the crate formats a path and a fixed reason only, never key material.
//! * **Schema v1 and migrations** — a versioned schema created by an append-only, transactional
//!   migration runner. The `migrations` table records which numbered steps ran; `meta` holds
//!   `schema_version`. `PRAGMA user_version` is deliberately unused.
//! * **The [`Store`] abstraction** — a narrow concrete type that hides `SQLCipher` and
//!   `sqlite-vec`. Backend errors are mapped to an internal [`StoreError`] that keeps only the
//!   numeric `SQLite` code.
//!
//! Encryption lives at the codec level, so checkpoints and `VACUUM INTO` copies inherit it. The
//! integration suite proves this with a per-run plaintext canary.
//!
//! ## Schema shape and the `vec0` width constraint
//!
//! v1 creates `meta`, `migrations`, `files`, `chunks`, the external-content FTS5 table
//! `chunks_fts`, and `model_registry`. It deliberately creates **no** `embeddings` table:
//! `sqlite-vec`'s `vec0` fixes the vector width in the virtual-table DDL and cannot vary it per
//! row. Embeddings are therefore a family of per-dimension tables, created by Stage 6 from
//! `model_registry`:
//!
//! ```sql
//! CREATE VIRTUAL TABLE embeddings_d<dim> USING vec0(
//!     embedding FLOAT[<dim>],
//!     +chunk_id BLOB,
//!     +model TEXT,
//!     +dim INTEGER
//! );
//! ```
//!
//! The `vec0` implicit integer `rowid` equals `chunks.rowid`, and `+chunk_id` duplicates the
//! content hash used for cache and dedup joins. A new dimension is a data migration, not a schema
//! change.

mod key;
mod migrations;
mod schema;
mod store;

pub mod seam;

pub use key::{Key, KeyFileError};
pub use store::{ChunkRow, FileRow, NewChunk, Store, StoreBuilder, StoreError, VectorHit};
