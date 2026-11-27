// SPDX-License-Identifier: Apache-2.0

//! `SHA-256` content identity shared by files and chunks.

use std::fmt;

use sha2::{Digest, Sha256};

/// `SHA-256` digest used for both `chunks.id` and `files.content_hash`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Returns the raw 32-byte digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8; 32]> for ContentHash {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ContentHash({self})")
    }
}

/// `SHA-256` of `bytes`; the single content-hash primitive for chunks and files.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> ContentHash {
    let digest = Sha256::digest(bytes);
    let mut hash = [0_u8; 32];
    hash.copy_from_slice(&digest);
    ContentHash(hash)
}

#[cfg(test)]
mod tests {
    use super::content_hash;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            content_hash(b"").to_string(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            content_hash(b"abc").to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn display_is_lowercase_hex_of_the_digest() {
        let hash = content_hash(b"abc");
        let hex = hash.to_string();
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert_eq!(format!("{hash:?}"), format!("ContentHash({hex})"));
    }

    #[test]
    fn content_hash_is_deterministic_and_distinguishes_bytes() {
        assert_eq!(content_hash(b"same"), content_hash(b"same"));
        assert_ne!(content_hash(b"a"), content_hash(b"b"));
        assert_eq!(content_hash(b"abc").as_bytes().len(), 32);
        assert_eq!(content_hash(b"abc").as_bytes(), content_hash(b"abc").as_ref());
    }
}
