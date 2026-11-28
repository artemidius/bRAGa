// SPDX-License-Identifier: AGPL-3.0-or-later

//! Opt-in blind index: truncated keyed hashes of tokens over encrypted chunk text.
//!
//! The index stores one row per `(token hash, chunk_id)` and no plaintext token. A token hash is
//! `HMAC-SHA256(key = HKDF-SHA256(master, info = "blind-index"), token)[..8]`, truncated to
//! [`TOKEN_LEN`] bytes. Everything is maintained through this crate's own `SQLCipher` connection,
//! so the Apache seam stays closed and no plaintext text is ever written.
//!
//! The index is **opt-in and default off**: nothing creates `blind_index` until [`build`] is
//! called, and [`search`] returns an empty result when the table does not exist. Building an index
//! changes what a copy of the database file reveals; [`LEAKAGE.md`] (included below) documents the
//! full leakage table.
//!
//! The index keys are domain-separated from chunk and embedding keys, and [`crate::rekey()`]
//! rebuilds the postings when it has to rewrite the records, because token hashes depend on the
//! master key. A stale index from an older key would simply find nothing rather than decrypt
//! anything. [`build`] is a full rebuild, so call it after [`crate::update()`] whenever the index is
//! enabled; it is the only operation that adds new chunks to it.
//!
//! [`LEAKAGE.md`]: ../LEAKAGE
#![doc = include_str!("../LEAKAGE.md")]

use std::collections::HashSet;

use braga::{
    config::Config,
    seam::{KeyProvider as _, SeamError},
};
use braga_storage::{ChunkRow, Key};
use hkdf::Hkdf;
use hmac::{Hmac, Mac as _};
use rusqlite::{Connection, params};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::{
    db,
    error::{ExtraError, Result},
    key::ExtraKey,
};

/// Length of one truncated blind-index token, in bytes.
pub const TOKEN_LEN: usize = 8;

/// The HKDF `info` string that separates the blind-index key from the record keys.
const INDEX_INFO: &[u8] = b"blind-index";

/// What one [`build`] wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlindIndexOutcome {
    /// Chunks whose text was tokenized.
    pub chunks: usize,
    /// Distinct `(token hash, chunk_id)` postings written.
    pub postings: usize,
}

/// One chunk found by [`search`].
///
/// The rows are unranked: a blind index records membership only, not term frequency or position.
/// They are ordered by path and then line for a deterministic result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlindHit {
    /// `chunks.id`, the 32-byte `SHA-256` digest of the chunk bytes.
    pub chunk_id: Vec<u8>,
    /// `files.path` of the file the chunk belongs to.
    pub path: String,
    /// 1-based, inclusive first line of the chunk.
    pub line_start: u32,
    /// 1-based, inclusive last line of the chunk.
    pub line_end: u32,
    /// The exact decrypted chunk text.
    pub content: String,
}

/// Creates or replaces the blind index for every chunk in the store.
///
/// The store is opened with an encryptor built from `master`, so the chunk text is decrypted in
/// memory and only truncated token hashes are written. The whole rebuild is one transaction: it
/// either replaces the previous postings or leaves them untouched. `db_key` opens the database;
/// `master` derives the index key and is not stored.
///
/// # Errors
///
/// Returns the storage, seam, or `SQLite` errors wrapped by [`crate::ExtraError`], including
/// [`crate::ExtraError::Seam`] when the store is not in extra mode or `master` is wrong.
pub fn build(config: &Config, db_key: &Key, master: &ExtraKey) -> Result<BlindIndexOutcome> {
    let store = db::extra_store(config, db_key, master)?;
    let chunks = store.all_chunks()?;
    drop(store);

    let index_key = derive_index_key(master)?;
    let connection = db::open(&config.storage.path, db_key)?;
    let transaction = connection.unchecked_transaction()?;
    ensure_table(&transaction)?;
    transaction.execute("DELETE FROM blind_index", [])?;
    let mut postings = 0;
    for (chunk_id, text) in &chunks {
        postings += insert_postings(&transaction, &*index_key, chunk_id, text)?;
    }
    transaction.commit()?;
    Ok(BlindIndexOutcome { chunks: chunks.len(), postings })
}

/// Finds every chunk that has `token` as an alphanumeric word and hydrates it through the
/// decrypting store.
///
/// `token` is one word; matching is case-insensitive and splits on non-alphanumeric characters,
/// exactly like [`build`]. A value with several words is rejected rather than silently searching
/// only the first. When no `blind_index` table exists (the default) the result is empty, without
/// opening an encryptor. Results are unranked and ordered by path, then line.
///
/// # Errors
///
/// Returns [`ExtraError::InvalidData`] when `token` is not exactly one word, and the storage,
/// seam, or `SQLite` errors wrapped by [`crate::ExtraError`]. A wrong `master` finds nothing,
/// because it derives different token hashes; it cannot decrypt chunk text.
pub fn search(
    config: &Config,
    db_key: &Key,
    master: &ExtraKey,
    token: &str,
) -> Result<Vec<BlindHit>> {
    let Some(query) = single_token(token)? else {
        return Ok(Vec::new());
    };
    let connection = db::open(&config.storage.path, db_key)?;
    if !db::has_table(&connection, "blind_index")? {
        return Ok(Vec::new());
    }

    let index_key = derive_index_key(master)?;
    let hash = hash_token(&*index_key, &query);
    let mut statement = connection.prepare("SELECT chunk_id FROM blind_index WHERE token = ?1")?;
    let chunk_ids = statement
        .query_map([hash.as_slice()], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    let mut rowids = Vec::with_capacity(chunk_ids.len());
    for chunk_id in &chunk_ids {
        // A concurrent delete can remove a chunk after the posting was read; skip it.
        if let Some(rowid) = db::rowid_for(&connection, chunk_id)? {
            rowids.push(rowid);
        }
    }
    drop(connection);
    if rowids.is_empty() {
        return Ok(Vec::new());
    }

    let store = db::extra_store(config, db_key, master)?;
    let mut hits =
        store.chunk_metadata(&rowids)?.into_iter().map(hit).collect::<Result<Vec<_>>>()?;
    hits.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line_start.cmp(&right.line_start))
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    Ok(hits)
}

/// Derives the 32-byte blind-index key with `HKDF-SHA256` and the `blind-index` info string.
pub(crate) fn derive_index_key(master: &ExtraKey) -> Result<Zeroizing<[u8; 32]>> {
    let master = Zeroizing::new(master.master_key()?);
    let hkdf = Hkdf::<Sha256>::new(Some(&[0_u8; 32]), &*master);
    let mut key = Zeroizing::new([0_u8; 32]);
    hkdf.expand(INDEX_INFO, &mut *key).map_err(|_| SeamError::Encrypt)?;
    Ok(key)
}

/// Creates the postings table if it does not exist yet.
pub(crate) fn ensure_table(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS blind_index (
             token BLOB,
             chunk_id BLOB,
             PRIMARY KEY (token, chunk_id)
         )",
    )?;
    Ok(())
}

/// Inserts the distinct postings of `text` for `chunk_id`, returning how many were new.
///
/// The caller removes old rows in its transaction; `INSERT OR IGNORE` makes a repeated
/// `(token, chunk_id)` pair a no-op.
pub(crate) fn insert_postings(
    connection: &Connection,
    index_key: &[u8],
    chunk_id: &[u8],
    text: &str,
) -> Result<usize> {
    let mut hashes = HashSet::new();
    for token in tokens(text) {
        hashes.insert(hash_token(index_key, &token));
    }
    let mut statement = connection
        .prepare("INSERT OR IGNORE INTO blind_index (token, chunk_id) VALUES (?1, ?2)")?;
    let mut inserted = 0;
    for hash in hashes {
        inserted += statement.execute(params![hash.as_slice(), chunk_id])?;
    }
    Ok(inserted)
}

/// Splits text into lowercased alphanumeric tokens; punctuation and whitespace separate them.
fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
}

/// Returns `token` in normalized form, `None` for empty input, or an error for several words.
fn single_token(text: &str) -> Result<Option<String>> {
    let mut tokens = tokens(text);
    let first = tokens.next();
    if tokens.next().is_some() {
        return Err(ExtraError::InvalidData {
            reason: "blind-index search accepts exactly one token",
        });
    }
    Ok(first)
}

/// Computes the truncated keyed hash of one normalized token.
fn hash_token(index_key: &[u8], token: &str) -> [u8; TOKEN_LEN] {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(index_key).expect("HMAC-SHA256 accepts a 32-byte key");
    mac.update(token.as_bytes());
    let digest = mac.finalize().into_bytes();
    let mut hash = [0_u8; TOKEN_LEN];
    hash.copy_from_slice(&digest[..TOKEN_LEN]);
    hash
}

/// Converts a stored row into the public result, validating the line range.
fn hit(row: ChunkRow) -> Result<BlindHit> {
    let line_start = u32::try_from(row.line_start)
        .map_err(|_| ExtraError::InvalidData { reason: "chunk line_start does not fit u32" })?;
    let line_end = u32::try_from(row.line_end)
        .map_err(|_| ExtraError::InvalidData { reason: "chunk line_end does not fit u32" })?;
    Ok(BlindHit {
        chunk_id: row.chunk_id,
        path: row.path,
        line_start,
        line_end,
        content: row.content,
    })
}

#[cfg(test)]
mod tests {
    use super::{TOKEN_LEN, derive_index_key, hash_token, single_token, tokens};
    use crate::key::ExtraKey;

    #[test]
    fn tokens_split_on_punctuation_and_lowercase() {
        assert_eq!(tokens("Hello, WORLD!").collect::<Vec<_>>(), ["hello", "world"]);
        assert_eq!(tokens("naïve-café_42").collect::<Vec<_>>(), ["naïve", "café", "42"]);
        assert_eq!(tokens("...").collect::<Vec<_>>(), Vec::<String>::new());
    }

    #[test]
    fn search_token_must_be_exactly_one_word() {
        assert_eq!(single_token("  Needle ").expect("one token"), Some("needle".to_owned()));
        assert_eq!(single_token("  ").expect("empty"), None);
        assert!(single_token("two words").is_err());
    }

    #[test]
    fn token_hash_is_truncated_and_key_bound() {
        let first = derive_index_key(&ExtraKey::from_bytes([0x11; 32])).expect("derive index key");
        let second = derive_index_key(&ExtraKey::from_bytes([0x22; 32])).expect("derive other key");
        let hash = hash_token(first.as_ref(), "needle");
        assert_eq!(hash.len(), TOKEN_LEN);
        assert_eq!(hash, hash_token(first.as_ref(), "needle"));
        assert_ne!(hash, hash_token(second.as_ref(), "needle"));
        assert_ne!(hash, hash_token(first.as_ref(), "haystack"));
    }
}
