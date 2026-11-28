// SPDX-License-Identifier: AGPL-3.0-or-later

//! The AGPL-3.0-or-later extra tier of `bRAGa`: per-record AEAD and decrypt-then-score retrieval.
//!
//! `braga-extra` implements the Apache crates' crypto seam with XChaCha20-Poly1305. It depends on
//! `braga` and `braga-storage`; the dependency never runs the other way, and no AGPL code lives in
//! the Apache crates.
//!
//! # What extra mode provides
//!
//! * Every chunk and embedding is sealed with a fresh XChaCha20-Poly1305 key derived by
//!   `HKDF-SHA256` from the caller's [`ExtraKey`], bound to the seam's AAD, with a random 24-byte
//!   nonce per record. Master and derived keys are zeroized on drop.
//! * [`query()`] decrypts every embedding for the model and scores the vectors in memory with cosine
//!   similarity. There is no approximate-nearest-neighbour search over ciphertext, and the cost is
//!   O(corpus) per query.
//! * [`update()`] reuses `braga::scanner` and `braga::embedding` unchanged over an encryptor-injected
//!   store, so the write pipeline keeps its cache and pinning behavior.
//! * [`rekey()`] re-encrypts every record from one master key to another in one `SQLite` transaction
//!   and rebuilds the opt-in blind index when one exists.
//! * [`build`] and [`search`] implement an opt-in blind index: truncated keyed token hashes for
//!   membership lookups, off until [`build`] is called. Its leakage table is documented in the
//!   [`blind`] module; it is not a way to search ciphertext and makes no such promise.
//! * [`status()`] reads counts and never decrypts a record; it needs only the storage key, not the
//!   AEAD master key.
//!
//! # What it does not promise
//!
//! Extra mode has no approximate-nearest-neighbour search over ciphertext and no general
//! substring or phrase search. A query must decrypt candidate vectors in memory to score them, so
//! the host process
//! and its memory are inside the trust boundary; a memory dump or `ptrace` recovery is out of scope
//! in the same way it is for the base mode. Metadata (file paths, content hashes, chunk offsets,
//! line numbers, and the model registry) is not covered by the AEAD.
//!
//! # Residual risk: remote embedding providers
//!
//! Extra mode encrypts records at rest, not on their way to the embedder. [`update()`] sends chunk
//! text to the configured provider, and [`query()`] sends the query text. A remote endpoint therefore
//! sees plaintext and must be trusted accordingly. Prefer a local provider such as Ollama with a
//! locally hosted model; a hard `allow_remote_embedders = false` policy gate is a follow-up, not
//! part of this stage.
//!
//! # Keys
//!
//! Two independent keys are involved. A [`Key`] opens the `SQLCipher` database
//! file; an [`ExtraKey`] derives the per-record AEAD keys. Every entry point takes them per call,
//! so this crate never stores either one. Losing the master key loses the chunk text and vectors
//! even when the database file is intact.
//!
//! # Licensing
//!
//! This crate is licensed under `AGPL-3.0-or-later`; see the bundled `LICENSE` and `NOTICE`.

#![forbid(unsafe_code)]

pub mod blind;
pub mod crypto;
pub mod key;
pub mod rekey;
pub mod retrieval;
pub mod update;

mod db;
mod error;

pub use blind::{BlindHit, BlindIndexOutcome, TOKEN_LEN, build, search};
pub use braga_embedder::Embedder;
pub use crypto::XChachaEncryptor;
pub use error::{ExtraError, Result};
pub use key::ExtraKey;
pub use rekey::{RekeyOutcome, rekey};
pub use retrieval::{ScoredChunk, query};
pub use update::{UpdateOutcome, update};

use braga::config::Config;
use braga_storage::{Key, Store};

use crate::db::Table;

/// Counts returned by [`status`].
///
/// None of these values requires the AEAD master key: they come from `SQLite` metadata and never
/// decrypt chunk text or vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtraStatus {
    /// Rows in `files`.
    pub files: usize,
    /// Rows in `chunks`; the ciphertext column is never read.
    pub chunks: usize,
    /// Rows in the encrypted embedding side table, or `0` when it does not exist.
    pub embeddings: usize,
    /// Whether the extra-mode embedding table exists yet.
    pub encrypted_embeddings: bool,
}

/// Reports the index's extra-mode shape without decrypting anything.
///
/// `db_key` is the `SQLCipher` key needed to open the database; the AEAD [`ExtraKey`] is not
/// accepted, so this function cannot read chunk text or vectors. It opens the store through
/// [`Store::open`] first so the schema exists on a fresh path, then counts the metadata tables on a
/// separate connection.
///
/// # Errors
///
/// Returns the storage or `SQLite` errors wrapped by [`ExtraError`].
pub fn status(config: &Config, db_key: &Key) -> Result<ExtraStatus> {
    // Base mode is deliberate: opening does not need the master key and must not decrypt.
    let store = Store::open(&config.storage.path, db_key)?;
    drop(store);

    let connection = db::open(&config.storage.path, db_key)?;
    let encrypted_embeddings = db::has_table(&connection, "encrypted_embeddings")?;
    let embeddings =
        if encrypted_embeddings { db::count(&connection, Table::EncryptedEmbeddings)? } else { 0 };
    Ok(ExtraStatus {
        files: db::count(&connection, Table::Files)?,
        chunks: db::count(&connection, Table::Chunks)?,
        embeddings,
        encrypted_embeddings,
    })
}
