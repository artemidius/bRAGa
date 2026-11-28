// SPDX-License-Identifier: Apache-2.0

//! The crypto seam: per-record encryption hooks the [`Store`](crate::Store) applies transparently.
//!
//! Base mode (no encryptor) is the default and is unchanged. Extra mode installs a
//! [`ChunkEncryptor`] through [`StoreBuilder`](crate::StoreBuilder); the store then encrypts chunk
//! content and embedding vectors on write and decrypts them on read. Base-only features
//! ([`Store::vector_search`](crate::Store::vector_search), `fts_search`, and `refresh_fts`) fail
//! closed with [`StoreError::SeamUnavailable`](crate::StoreError::SeamUnavailable) in extra mode.
//!
//! The traits live here rather than in `braga`, so `braga-storage` can name them without inverting
//! the crate graph; `braga` re-exports this module as `braga::seam`.
//!
//! ## AAD serialization (normative for implementors)
//!
//! `collection || '\0' || model-or-empty || '\0' || chunk_id || '\0' || kind`, where `kind` is the
//! single byte `0x01` for [`RecordKind::Chunk`] and `0x02` for [`RecordKind::Embedding`]. A wrong
//! key or a mixed-mode database fails the AEAD tag and surfaces as [`SeamError::Decrypt`]; the
//! database carries no in-band mode marker.

use thiserror::Error;

/// Which kind of record an [`Aad`] binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// A chunk's text, stored in `chunks.content`.
    Chunk,
    /// An embedding vector, stored in `encrypted_embeddings`.
    Embedding,
}

/// The authenticated-additional-data binding for one record.
///
/// Binding the index to the ciphertext prevents a ciphertext from being moved to another chunk,
/// model, or record kind without failing authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aad {
    /// The collection name; the single-collection v1 store always uses `"default"`.
    pub collection: String,
    /// The embedding model, or `None` for chunk content (chunks are written before their model is
    /// known).
    pub model: Option<String>,
    /// The record's `chunk_id`: `chunks.id`, the `SHA-256` content hash.
    pub chunk_id: Vec<u8>,
    /// Whether this record is chunk text or an embedding vector.
    pub kind: RecordKind,
}

/// Failure of a [`ChunkEncryptor`] operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SeamError {
    /// The encryptor could not encrypt a record.
    #[error("encryptor failed to encrypt a record")]
    Encrypt,
    /// The encryptor could not decrypt a record (wrong key or tampered ciphertext).
    #[error("encryptor failed to decrypt a record (wrong key or tampered ciphertext)")]
    Decrypt,
    /// The encryptor rejected the AAD binding.
    #[error("encryptor rejected the AAD binding")]
    InvalidAad,
}

/// Per-record AEAD primitive implemented outside the Apache crates.
///
/// The store holds an optional `Arc<dyn ChunkEncryptor>` and never sees key material.
pub trait ChunkEncryptor: Send + Sync {
    /// Encrypts `plaintext` bound to `aad`, returning self-contained ciphertext.
    ///
    /// # Errors
    ///
    /// Returns [`SeamError::Encrypt`] or [`SeamError::InvalidAad`] when encryption fails.
    fn encrypt(&self, aad: &Aad, plaintext: &[u8]) -> Result<Vec<u8>, SeamError>;

    /// Decrypts `ciphertext` bound to `aad`.
    ///
    /// # Errors
    ///
    /// Returns [`SeamError::Decrypt`] when the key is wrong, the ciphertext was tampered with, or
    /// `aad` does not match the binding used at encryption time.
    fn decrypt(&self, aad: &Aad, ciphertext: &[u8]) -> Result<Vec<u8>, SeamError>;
}

/// Master-key source for seam implementations.
///
/// The store does not hold a `KeyProvider`: in extra mode the master key is caller-supplied per
/// operation. This trait types the concrete implementations and the reserved ABI key slot.
pub trait KeyProvider: Send + Sync {
    /// Returns the 32-byte master key.
    ///
    /// # Errors
    ///
    /// Returns a [`SeamError`] when the key cannot be read or is rejected.
    fn master_key(&self) -> Result<[u8; 32], SeamError>;
}
