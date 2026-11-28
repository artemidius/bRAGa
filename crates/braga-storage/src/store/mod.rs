// SPDX-License-Identifier: Apache-2.0

//! The encrypted [`Store`] facade over `SQLCipher`.

use std::sync::{Arc, Once};

use rusqlite::{Connection, ffi};

use crate::seam::{Aad, ChunkEncryptor, RecordKind, SeamError};

mod builder;
mod error;
mod query;
mod read;
mod write;

pub use builder::StoreBuilder;
pub use error::StoreError;

/// Entry-point type expected by `sqlite3_auto_extension`.
type AutoExtension = unsafe extern "C" fn(
    *mut ffi::sqlite3,
    *mut *mut std::ffi::c_char,
    *const ffi::sqlite3_api_routines,
) -> std::ffi::c_int;

static REGISTER_VEC: Once = Once::new();

/// Registers `sqlite-vec` as a process-wide auto-extension.
///
/// This must run before the first connection is opened. `sqlite-vec` declares
/// `sqlite3_vec_init` with a `()` signature and transmutes it; we repeat that cast with an
/// explicit target type.
fn register_vector_extension() {
    REGISTER_VEC.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the C entry point exported by the statically linked
        // `sqlite-vec` library. SQLite calls it as an extension initializer, which is exactly the
        // signature declared by `AutoExtension`.
        let init: AutoExtension =
            unsafe { std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ()) };
        // SAFETY: `init` is a valid SQLite extension entry point and registration is idempotent.
        unsafe {
            ffi::sqlite3_auto_extension(Some(init));
        }
    });
}

/// One `files` row as read for scanning.
///
/// `content_hash` stays an opaque byte vector so this crate never depends on the chunker; its
/// rustdoc records the invariant the writer must honor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    /// `files.id`, the integer primary key that `chunks.file_id` references.
    pub id: i64,
    /// `files.path`, the file's UTF-8 path, exactly as the writer stored it.
    pub path: String,
    /// `files.content_hash`, the 32-byte `SHA-256` digest of the file's bytes.
    pub content_hash: Vec<u8>,
}

/// A chunk to write for a file, mirroring the `chunks` columns.
///
/// `file_id` is absent because [`Store::sync_file`] supplies the id of the row it upserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewChunk<'a> {
    /// `chunks.id`: the 32-byte `SHA-256` of [`content`](Self::content).
    pub id: &'a [u8],
    /// `chunks.byte_offset`: byte offset of `content` in the source file.
    pub byte_offset: i64,
    /// `chunks.line_start`: 1-based, inclusive first line of `content`.
    pub line_start: i64,
    /// `chunks.line_end`: 1-based, inclusive last line of `content`.
    pub line_end: i64,
    /// `chunks.content`: the exact chunk text.
    pub content: &'a str,
}

/// One ranked KNN hit: `chunks.rowid` and its `vec0` distance (ascending).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorHit {
    /// `chunks.rowid` of the hit, the join key back to `chunks`.
    pub rowid: i64,
    /// The `vec0` distance under the table's metric; smaller is closer.
    pub distance: f32,
}

/// One `chunks` row joined to its `files.path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRow {
    /// `chunks.id`, the 32-byte `SHA-256` digest of the chunk bytes.
    pub chunk_id: Vec<u8>,
    /// `files.path` of the file the chunk belongs to.
    pub path: String,
    /// `chunks.line_start`: 1-based, inclusive first line of the chunk.
    pub line_start: i64,
    /// `chunks.line_end`: 1-based, inclusive last line of the chunk.
    pub line_end: i64,
    /// `chunks.content`: the exact chunk text.
    pub content: String,
}

/// An open, encrypted `SQLite` database.
///
/// `Store` owns its connection; dropping it closes the database. Reopening is closing (or
/// dropping) it followed by [`Store::open`]. Neither `rusqlite` nor `sqlite-vec` types appear in
/// its public API.
///
/// Key rotation is reserved for a later stage: the intended primitive is `Store::rekey(&self,
/// new_key: &Key)`, backed by `sqlite3_rekey_v2` (reachable as `Connection::key(_, new_key, true)`).
/// It is not implemented here; `braga-extra` defines the rotation contract first.
pub struct Store {
    conn: Connection,
    encryptor: Option<Arc<dyn ChunkEncryptor>>,
}

/// The single collection name carried in AAD by the v1 store.
const COLLECTION: &str = "default";

/// Builds the AAD binding for one chunk-content record.
fn chunk_aad(chunk_id: &[u8]) -> Aad {
    Aad {
        collection: COLLECTION.to_owned(),
        model: None,
        chunk_id: chunk_id.to_vec(),
        kind: RecordKind::Chunk,
    }
}

/// Builds the AAD binding for one embedding record.
fn embedding_aad(model: &str, chunk_id: &[u8]) -> Aad {
    Aad {
        collection: COLLECTION.to_owned(),
        model: Some(model.to_owned()),
        chunk_id: chunk_id.to_vec(),
        kind: RecordKind::Embedding,
    }
}

/// Encrypts chunk text and hex-encodes it for the `TEXT` column of `chunks`.
fn seal_text(
    encryptor: &dyn ChunkEncryptor,
    aad: &Aad,
    plaintext: &str,
) -> Result<String, StoreError> {
    Ok(encode_hex(&encryptor.encrypt(aad, plaintext.as_bytes())?))
}

/// Hex-decodes and decrypts one `chunks.content` value.
fn open_text(
    encryptor: &dyn ChunkEncryptor,
    aad: &Aad,
    stored: &str,
) -> Result<String, StoreError> {
    let ciphertext = decode_hex(stored).ok_or(StoreError::Seam(SeamError::Decrypt))?;
    let plaintext = encryptor.decrypt(aad, &ciphertext)?;
    String::from_utf8(plaintext).map_err(|_| StoreError::Seam(SeamError::Decrypt))
}

/// Encodes bytes as lowercase hex, the conservative `TEXT`-column representation.
fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(DIGITS[usize::from(byte & 0x0F)] as char);
    }
    encoded
}

/// Decodes hex, returning `None` for an odd length or a non-hex digit.
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

impl Store {
    /// Whether the lazily-created `encrypted_embeddings` table exists yet.
    fn has_encrypted_embeddings(&self) -> Result<bool, StoreError> {
        self.conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'table' AND name = 'encrypted_embeddings'
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }
}

/// Validates the `vec0` dimension and distance metric accepted by
/// [`Store::ensure_embedding_table`].
///
/// `1..=16384` is the documented width bound; the metric names are the frozen public set, which
/// this crate validates even though the pinned `sqlite-vec` implements only `l2` and `cosine`.
fn validate_vec0_parameter(dim: u32, metric: &str) -> Result<(), StoreError> {
    if !(1..=16384).contains(&dim) {
        return Err(StoreError::InvalidVec0Parameter { parameter: "dim" });
    }
    match metric {
        "cosine" | "l2" | "ip" => Ok(()),
        _ => Err(StoreError::InvalidVec0Parameter { parameter: "metric" }),
    }
}

#[cfg(test)]
mod tests {
    use super::{NewChunk, Store};
    use crate::key::Key;

    #[test]
    fn all_chunks_is_empty_then_returns_each_chunk_id_and_content() {
        let dir = tempfile::tempdir().expect("create temporary directory");
        let store = Store::open(&dir.path().join("braga.sqlite3"), &Key::from_bytes([0x21; 32]))
            .expect("open store");

        assert_eq!(store.all_chunks().expect("list an empty index"), Vec::new());

        let (first, second) = ([0xA1_u8; 32], [0xB2_u8; 32]);
        store
            .sync_file(
                "a.md",
                &[0xC3_u8; 32],
                &[
                    NewChunk {
                        id: &first,
                        byte_offset: 0,
                        line_start: 1,
                        line_end: 1,
                        content: "first chunk",
                    },
                    NewChunk {
                        id: &second,
                        byte_offset: 10,
                        line_start: 2,
                        line_end: 2,
                        content: "second chunk",
                    },
                ],
            )
            .expect("sync file");

        assert_eq!(
            store.all_chunks().expect("list the index"),
            vec![
                (first.to_vec(), "first chunk".to_owned()),
                (second.to_vec(), "second chunk".to_owned()),
            ]
        );
    }
}
