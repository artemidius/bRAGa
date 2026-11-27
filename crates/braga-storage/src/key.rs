// SPDX-License-Identifier: Apache-2.0

//! Raw 256-bit database keys.

use std::fmt;

/// A raw 256-bit `SQLCipher` key.
///
/// `Debug` deliberately redacts the bytes so a key can never leak through logging.
#[derive(Clone, PartialEq, Eq)]
pub struct Key([u8; 32]);

impl Key {
    /// Creates a key from exactly 32 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrows the raw key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key([REDACTED])")
    }
}
