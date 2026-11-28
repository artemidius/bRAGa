// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 256-bit master key used by the AEAD layer.
//!
//! This key is distinct from the `SQLCipher` [`Key`](braga_storage::Key) that opens the database:
//! the database key protects the file, while this master key derives the per-record AEAD keys.
//! Neither key is persisted by this crate; callers supply the master key per operation.

use std::fmt;

use braga::seam::{KeyProvider, SeamError};
use zeroize::Zeroize;

/// A 256-bit `braga-extra` master key.
///
/// The bytes are zeroized when the value is dropped, and `Debug` redacts them so a key can never
/// leak through logging. Cloning a key makes an independent copy that is zeroized on its own drop;
/// prefer borrowing one key per operation.
#[derive(Clone)]
pub struct ExtraKey([u8; 32]);

impl ExtraKey {
    /// Wraps exactly 32 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Generates a fresh 256-bit key from the operating system `CSPRNG`.
    ///
    /// # Panics
    ///
    /// Panics if the operating system `CSPRNG` is unavailable. On the desktop platforms this crate
    /// targets, that indicates a broken system rather than a recoverable condition.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).expect("operating system CSPRNG must be available");
        Self(bytes)
    }
}

impl KeyProvider for ExtraKey {
    fn master_key(&self) -> Result<[u8; 32], SeamError> {
        Ok(self.0)
    }
}

impl Zeroize for ExtraKey {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for ExtraKey {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl fmt::Debug for ExtraKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExtraKey([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use braga::seam::KeyProvider as _;

    use super::ExtraKey;

    #[test]
    fn debug_redacts_the_master_key() {
        let key = ExtraKey::from_bytes([0x42; 32]);
        assert_eq!(format!("{key:?}"), "ExtraKey([REDACTED])");
    }

    #[test]
    fn master_key_returns_the_wrapped_bytes() {
        let key = ExtraKey::from_bytes([0x17; 32]);
        assert_eq!(key.master_key().expect("read master key"), [0x17; 32]);
    }
}
