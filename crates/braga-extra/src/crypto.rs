// SPDX-License-Identifier: AGPL-3.0-or-later

//! XChaCha20-Poly1305 per-record encryption.
//!
//! [`XChachaEncryptor`] implements the Apache crate's [`ChunkEncryptor`] seam. Every record gets a
//! fresh key derived with `HKDF-SHA256` from the master key and a domain-separated `info` string,
//! plus a random 24-byte nonce. The ciphertext layout is `nonce || ciphertext || tag`; the AAD is
//! serialized exactly as the seam documents (`collection || 0 || model || 0 || chunk_id || 0 ||
//! kind`), so a ciphertext moved to another chunk, model, or record kind fails authentication.
//!
//! The master key and every derived key are zeroized on drop. The stored database file never
//! contains the master key; a copy of the file therefore yields no chunk text or vector without it.

use braga::seam::{Aad, ChunkEncryptor, KeyProvider as _, RecordKind, SeamError};
use chacha20poly1305::{
    Key as AeadKey, XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::key::ExtraKey;

/// Length of the random XChaCha20-Poly1305 nonce.
pub const NONCE_LEN: usize = 24;

/// Length of the Poly1305 authentication tag.
pub const TAG_LEN: usize = 16;

/// The single collection name carried in AAD by the v1 store.
pub(crate) const COLLECTION: &str = "default";

/// Per-record XChaCha20-Poly1305 encryptor.
///
/// The encryptor holds the master key for its lifetime; callers normally build one per operation
/// and drop it afterwards. Cloning is deliberately not implemented.
pub struct XChachaEncryptor {
    key: ExtraKey,
}

impl XChachaEncryptor {
    /// Builds an encryptor from `key`.
    #[must_use]
    pub const fn new(key: ExtraKey) -> Self {
        Self { key }
    }

    /// Encrypts with a caller-supplied nonce; used by the pinned test vector and [`Self::encrypt`].
    fn encrypt_with_nonce(
        &self,
        aad: &Aad,
        plaintext: &[u8],
        nonce: [u8; NONCE_LEN],
    ) -> Result<Vec<u8>, SeamError> {
        let additional = aad_bytes(aad)?;
        let derived = derive_key(&self.key, aad)?;
        let cipher = XChaCha20Poly1305::new(AeadKey::from_slice(derived.as_ref()));
        let sealed = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &additional })
            .map_err(|_| SeamError::Encrypt)?;
        let mut ciphertext = Vec::with_capacity(NONCE_LEN + sealed.len());
        ciphertext.extend_from_slice(&nonce);
        ciphertext.extend_from_slice(&sealed);
        Ok(ciphertext)
    }
}

impl ChunkEncryptor for XChachaEncryptor {
    /// Encrypts `plaintext` with a fresh random nonce and the AAD-derived key.
    ///
    /// # Errors
    ///
    /// Returns [`SeamError::InvalidAad`] when the binding is malformed and [`SeamError::Encrypt`]
    /// when the system `CSPRNG` or the AEAD fails.
    fn encrypt(&self, aad: &Aad, plaintext: &[u8]) -> Result<Vec<u8>, SeamError> {
        let mut nonce = [0_u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| SeamError::Encrypt)?;
        self.encrypt_with_nonce(aad, plaintext, nonce)
    }

    /// Decrypts `ciphertext` after rebuilding the same AAD-derived key.
    ///
    /// # Errors
    ///
    /// Returns [`SeamError::Decrypt`] when the key is wrong, the ciphertext is shorter than
    /// `nonce + tag`, or the AEAD tag does not verify; [`SeamError::InvalidAad`] when the binding
    /// is malformed.
    fn decrypt(&self, aad: &Aad, ciphertext: &[u8]) -> Result<Vec<u8>, SeamError> {
        let additional = aad_bytes(aad)?;
        if ciphertext.len() < NONCE_LEN + TAG_LEN {
            return Err(SeamError::Decrypt);
        }
        let (nonce, sealed) = ciphertext.split_at(NONCE_LEN);
        let derived = derive_key(&self.key, aad)?;
        let cipher = XChaCha20Poly1305::new(AeadKey::from_slice(derived.as_ref()));
        cipher
            .decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: &additional })
            .map_err(|_| SeamError::Decrypt)
    }
}

/// Derives the per-record AEAD key with `HKDF-SHA256` and a zero salt.
fn derive_key(key: &ExtraKey, aad: &Aad) -> Result<Zeroizing<[u8; 32]>, SeamError> {
    let master = Zeroizing::new(key.master_key()?);
    let hkdf = Hkdf::<Sha256>::new(Some(&[0_u8; 32]), &*master);
    let mut derived = Zeroizing::new([0_u8; 32]);
    hkdf.expand(&key_info(aad), &mut *derived).map_err(|_| SeamError::Encrypt)?;
    Ok(derived)
}

/// The domain-separated HKDF `info` string for `aad`.
fn key_info(aad: &Aad) -> Vec<u8> {
    let prefix: &[u8] = match aad.kind {
        RecordKind::Chunk => b"braga-extra/chunk",
        RecordKind::Embedding => b"braga-extra/embedding",
    };
    let mut info = Vec::with_capacity(prefix.len() + aad.collection.len() + aad.chunk_id.len() + 4);
    info.extend_from_slice(prefix);
    info.push(0);
    info.extend_from_slice(aad.collection.as_bytes());
    info.push(0);
    if let Some(model) = &aad.model {
        info.extend_from_slice(model.as_bytes());
    }
    info.push(0);
    info.extend_from_slice(&aad.chunk_id);
    info
}

/// Serializes the frozen AAD contract and validates that `kind` and `model` agree.
fn aad_bytes(aad: &Aad) -> Result<Vec<u8>, SeamError> {
    match (aad.kind, aad.model.as_deref()) {
        (RecordKind::Chunk, None) => {}
        (RecordKind::Embedding, Some(model)) if !model.is_empty() => {}
        _ => return Err(SeamError::InvalidAad),
    }
    if aad.collection.is_empty() || aad.chunk_id.is_empty() {
        return Err(SeamError::InvalidAad);
    }

    let mut bytes = Vec::with_capacity(aad.collection.len() + aad.chunk_id.len() + 4);
    bytes.extend_from_slice(aad.collection.as_bytes());
    bytes.push(0);
    if let Some(model) = &aad.model {
        bytes.extend_from_slice(model.as_bytes());
    }
    bytes.push(0);
    bytes.extend_from_slice(&aad.chunk_id);
    bytes.push(0);
    bytes.push(match aad.kind {
        RecordKind::Chunk => 0x01,
        RecordKind::Embedding => 0x02,
    });
    Ok(bytes)
}

/// Builds the AAD binding for one chunk-content record.
pub(crate) fn chunk_aad(chunk_id: &[u8]) -> Aad {
    Aad {
        collection: COLLECTION.to_owned(),
        model: None,
        chunk_id: chunk_id.to_vec(),
        kind: RecordKind::Chunk,
    }
}

/// Builds the AAD binding for one embedding record.
pub(crate) fn embedding_aad(model: &str, chunk_id: &[u8]) -> Aad {
    Aad {
        collection: COLLECTION.to_owned(),
        model: Some(model.to_owned()),
        chunk_id: chunk_id.to_vec(),
        kind: RecordKind::Embedding,
    }
}

#[cfg(test)]
mod tests {
    use braga::seam::{ChunkEncryptor as _, RecordKind};

    use super::{XChachaEncryptor, chunk_aad, embedding_aad};
    use crate::key::ExtraKey;

    fn encryptor(byte: u8) -> XChachaEncryptor {
        XChachaEncryptor::new(ExtraKey::from_bytes([byte; 32]))
    }

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;

        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
        }
        encoded
    }

    #[test]
    fn known_answer_vector_is_pinned() {
        let aad = chunk_aad(&[0x11; 32]);
        let sealed =
            encryptor(0x24).encrypt_with_nonce(&aad, b"known answer", [0xA5; 24]).expect("seal");
        assert_eq!(
            hex(&sealed),
            "a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5e5ba367bf27dddb8fcab622a78622c47ee3adf0afbc6d605f6415093"
        );
    }

    #[test]
    fn aad_validation_rejects_a_missing_model_and_an_empty_chunk_id() {
        let aad = braga::seam::Aad {
            collection: "default".to_owned(),
            model: None,
            chunk_id: vec![],
            kind: RecordKind::Chunk,
        };
        assert!(encryptor(1).encrypt(&aad, b"x").is_err());
        let mut embedding = embedding_aad("m", &[4; 32]);
        embedding.model = None;
        assert!(encryptor(1).encrypt(&embedding, b"x").is_err());
    }
}
