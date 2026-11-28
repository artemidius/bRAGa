// SPDX-License-Identifier: AGPL-3.0-or-later

//! Master-key rotation: read every record under the old key, rewrite it under the new one.
//!
//! [`rekey`] rewrites `chunks.content` and `encrypted_embeddings.ciphertext` in one `SQLite`
//! transaction. When the opt-in `blind_index` table exists it is rebuilt in the same transaction,
//! because its token hashes depend on the master key. It is transactional at the `SQLite` layer
//! only: a process crash rolls the database back, but the caller is responsible for persisting the
//! new master key once the call returns (and for keeping the old one until then). There is no
//! in-database key marker, so a mixed rewrite fails the AEAD tag rather than silently exposing
//! plaintext.

use braga::{
    config::Config,
    seam::{ChunkEncryptor as _, SeamError},
};
use braga_storage::Key;
use rusqlite::params;

use crate::{
    blind,
    crypto::{XChachaEncryptor, chunk_aad, embedding_aad},
    db,
    error::Result,
    key::ExtraKey,
};

/// What one [`rekey`] rewrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RekeyOutcome {
    /// Chunk-content records re-encrypted.
    pub chunks: usize,
    /// Embedding-vector records re-encrypted.
    pub embeddings: usize,
}

/// Re-encrypts every stored record from `old` to `new`.
///
/// The database itself is opened with `db_key` (the `SQLCipher` key), which is separate from the
/// AEAD master key. All chunk rows are read and re-sealed first, then all encrypted embedding rows,
/// and the whole rewrite commits or rolls back as one transaction. When `blind_index` exists, its
/// postings are deleted and rewritten from the decrypted text under the new index key in the same
/// transaction, so a rotation never leaves a stale index behind.
///
/// # Errors
///
/// Returns [`crate::ExtraError`] when a record cannot be decrypted under `old`, and the storage or
/// `SQLite` errors wrapped by the same type.
pub fn rekey(
    config: &Config,
    db_key: &Key,
    old: &ExtraKey,
    new: &ExtraKey,
) -> Result<RekeyOutcome> {
    let old_encryptor = XChachaEncryptor::new(old.clone());
    let new_encryptor = XChachaEncryptor::new(new.clone());
    let connection = db::open(&config.storage.path, db_key)?;
    let transaction = connection.unchecked_transaction()?;

    let blind_key = if db::has_table(&transaction, "blind_index")? {
        Some(blind::derive_index_key(new)?)
    } else {
        None
    };
    if blind_key.is_some() {
        transaction.execute("DELETE FROM blind_index", [])?;
    }

    let chunks = {
        let mut statement = transaction.prepare("SELECT id, content FROM chunks ORDER BY rowid")?;
        statement
            .query_map([], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (chunk_id, stored) in chunks {
        let ciphertext = decode_hex(&stored).ok_or(SeamError::Decrypt)?;
        let aad = chunk_aad(&chunk_id);
        let plaintext = old_encryptor.decrypt(&aad, &ciphertext)?;
        if let Some(index_key) = &blind_key {
            let text = std::str::from_utf8(&plaintext).map_err(|_| {
                crate::ExtraError::InvalidData { reason: "chunk plaintext is not UTF-8" }
            })?;
            blind::insert_postings(&transaction, &**index_key, &chunk_id, text)?;
        }
        let rewritten = new_encryptor.encrypt(&aad, &plaintext)?;
        transaction.execute(
            "UPDATE chunks SET content = ?1 WHERE id = ?2",
            params![encode_hex(&rewritten), chunk_id],
        )?;
    }
    let chunk_count = db::count(&transaction, db::Table::Chunks)?;

    let embeddings = if db::has_table(&transaction, "encrypted_embeddings")? {
        let rows = {
            let mut statement = transaction
                .prepare("SELECT chunk_id, model, dim, ciphertext FROM encrypted_embeddings")?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (chunk_id, model, dim, ciphertext) in rows {
            let aad = embedding_aad(&model, &chunk_id);
            let plaintext = old_encryptor.decrypt(&aad, &ciphertext)?;
            let rewritten = new_encryptor.encrypt(&aad, &plaintext)?;
            transaction.execute(
                "UPDATE encrypted_embeddings SET ciphertext = ?1
                 WHERE chunk_id = ?2 AND model = ?3 AND dim = ?4",
                params![rewritten, chunk_id, model, dim],
            )?;
        }
        db::count(&transaction, db::Table::EncryptedEmbeddings)?
    } else {
        0
    };

    transaction.commit()?;
    Ok(RekeyOutcome { chunks: chunk_count, embeddings })
}

/// Encodes bytes as lowercase hex, the representation `chunks.content` uses in extra mode.
fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(DIGITS[usize::from(byte & 0x0F)] as char);
    }
    encoded
}

/// Decodes lowercase or uppercase hex, returning `None` for an odd length or a bad digit.
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut decoded = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().as_chunks::<2>().0 {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        decoded.push(u8::try_from(high * 16 + low).ok()?);
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::{decode_hex, encode_hex};

    #[test]
    fn hex_round_trips_and_rejects_odd_or_invalid_input() {
        let bytes = [0x00, 0x0F, 0xA5, 0xFF];
        assert_eq!(encode_hex(&bytes), "000fa5ff");
        assert_eq!(decode_hex("000fa5ff").expect("decode"), bytes);
        assert!(decode_hex("abc").is_none());
        assert!(decode_hex("zz").is_none());
    }
}
