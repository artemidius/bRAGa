// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the Stage 12a crypto seam, over the real `SQLCipher` build.
//!
//! [`FakeEncryptor`] is a tag-and-XOR stand-in for the AEAD that `braga-extra` will implement. It
//! is not cryptography; it proves the store routes every chunk and embedding through the seam, that
//! base mode is untouched, and that a wrong key fails closed.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use braga_storage::{
    Key, NewChunk, Store, StoreError,
    seam::{Aad, ChunkEncryptor, SeamError},
};
use rusqlite::Connection;
use tempfile::TempDir;

const MODEL: &str = "test-model";
const DB_KEY: u8 = 0xA1;
/// All-hex text, so plaintext at rest would survive as a visible canary even in a hex column.
const CANARY: &str = "deadbeefcafebabe";

/// A reversible fake: one tag byte, then the plaintext `XOR`ed with the key.
struct FakeEncryptor {
    key: u8,
}

impl ChunkEncryptor for FakeEncryptor {
    fn encrypt(&self, _aad: &Aad, plaintext: &[u8]) -> Result<Vec<u8>, SeamError> {
        let mut ciphertext = Vec::with_capacity(plaintext.len() + 1);
        ciphertext.push(self.key ^ 0xA5);
        ciphertext.extend(plaintext.iter().map(|byte| byte ^ self.key));
        Ok(ciphertext)
    }

    fn decrypt(&self, _aad: &Aad, ciphertext: &[u8]) -> Result<Vec<u8>, SeamError> {
        let (tag, body) = ciphertext.split_first().ok_or(SeamError::Decrypt)?;
        if *tag != self.key ^ 0xA5 {
            return Err(SeamError::Decrypt);
        }
        Ok(body.iter().map(|byte| byte ^ self.key).collect())
    }
}

fn key(byte: u8) -> Key {
    Key::from_bytes([byte; 32])
}

fn temp_db() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let path = dir.path().join("braga.sqlite3");
    (dir, path)
}

fn extra_store(path: &Path, encryptor_key: u8) -> Store {
    Store::builder()
        .with_encryptor(Arc::new(FakeEncryptor { key: encryptor_key }))
        .open(path, &key(DB_KEY))
        .expect("open store with the fake encryptor")
}

/// Opens the encrypted database outside [`Store`], so tests can inspect ciphertext at rest.
fn open_raw(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open raw encrypted database");
    conn.key(None::<&str>, key(DB_KEY).as_bytes(), false).expect("apply key");
    conn
}

fn chunk<'a>(id: &'a [u8; 32], content: &'a str) -> NewChunk<'a> {
    NewChunk { id, byte_offset: 0, line_start: 1, line_end: 1, content }
}

fn rowid(path: &Path, id: &[u8; 32]) -> i64 {
    open_raw(path)
        .query_row("SELECT rowid FROM chunks WHERE id = ?1", [id.as_slice()], |row| row.get(0))
        .expect("read chunk rowid")
}

fn le_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[test]
fn extra_mode_round_trips_chunks_and_embeddings_across_reopen() {
    let (_dir, path) = temp_db();
    let chunk_id = [0x11_u8; 32];
    let vector = [0.25_f32, -1.5, 2.0, 3.75];
    {
        let store = extra_store(&path, 0x11);
        store
            .sync_file("notes/a.md", &[0x22_u8; 32], &[chunk(&chunk_id, CANARY)])
            .expect("sync file");
        store.insert_embedding(&chunk_id, MODEL, 4, &vector).expect("insert embedding");
        assert_eq!(
            store.all_chunks().expect("read chunks"),
            vec![(chunk_id.to_vec(), CANARY.into())]
        );
        assert!(store.embedding_exists(MODEL, 4, &chunk_id).expect("lookup hit"));
    }

    let store = extra_store(&path, 0x11);
    assert_eq!(store.all_chunks().expect("read reopened chunks").len(), 1);
    assert_eq!(
        store.chunk_metadata(&[rowid(&path, &chunk_id)]).expect("read metadata")[0].content,
        CANARY
    );
    assert_eq!(
        store.list_embeddings(MODEL, 4).expect("list embeddings"),
        vec![(chunk_id.to_vec(), vector.to_vec())]
    );
}

#[test]
fn base_and_extra_modes_return_identical_plaintext() {
    let (_base_dir, base_path) = temp_db();
    let (_extra_dir, extra_path) = temp_db();
    let base = Store::open(&base_path, &key(DB_KEY)).expect("open base store");
    let extra = extra_store(&extra_path, 0x11);
    let chunk_id = [0x31_u8; 32];
    let vector = [1.0_f32, 2.0, 3.0, 4.0];
    base.ensure_embedding_table(4, "cosine").expect("create base embeddings_d4");
    for store in [&base, &extra] {
        store
            .sync_file("notes/b.md", &[0x32_u8; 32], &[chunk(&chunk_id, CANARY)])
            .expect("sync file");
        store.insert_embedding(&chunk_id, MODEL, 4, &vector).expect("insert embedding");
    }

    assert_eq!(base.all_chunks().expect("base chunks"), extra.all_chunks().expect("extra chunks"));
    assert_eq!(
        base.chunk_metadata(&[rowid(&base_path, &chunk_id)]).expect("base metadata"),
        extra.chunk_metadata(&[rowid(&extra_path, &chunk_id)]).expect("extra metadata")
    );
    assert!(base.embedding_exists(MODEL, 4, &chunk_id).expect("base lookup"));
    assert!(extra.embedding_exists(MODEL, 4, &chunk_id).expect("extra lookup"));
}

#[test]
fn extra_mode_leaves_no_plaintext_canary_at_rest() {
    let (_dir, path) = temp_db();
    let chunk_id = [0x41_u8; 32];
    let vector = [0.25_f32, -1.5, 2.0, 3.75];
    let store = extra_store(&path, 0x5A);
    store.sync_file("notes/c.md", &[0x42_u8; 32], &[chunk(&chunk_id, CANARY)]).expect("sync file");
    store.insert_embedding(&chunk_id, MODEL, 4, &vector).expect("insert embedding");

    let conn = open_raw(&path);
    let stored: String = conn
        .query_row("SELECT content FROM chunks WHERE id = ?1", [chunk_id.as_slice()], |row| {
            row.get(0)
        })
        .expect("read stored chunk");
    assert!(!stored.contains(CANARY), "chunk plaintext must not reach `chunks.content`");
    let fts_hits: i64 = conn
        .query_row("SELECT count(*) FROM chunks_fts WHERE chunks_fts MATCH ?1", [CANARY], |row| {
            row.get(0)
        })
        .expect("search the fts index");
    assert_eq!(fts_hits, 0, "extra mode must not populate the plaintext FTS index");

    let ciphertext: Vec<u8> = conn
        .query_row(
            "SELECT ciphertext FROM encrypted_embeddings
             WHERE chunk_id = ?1 AND model = ?2 AND dim = 4",
            rusqlite::params![chunk_id.as_slice(), MODEL],
            |row| row.get(0),
        )
        .expect("read stored embedding ciphertext");
    let plaintext = le_bytes(&vector);
    assert!(!contains(&ciphertext, &plaintext), "vector plaintext must not reach the BLOB table");
    assert_eq!(ciphertext.len(), plaintext.len() + 1, "the fake encryptor must have framed it");
}

#[test]
fn extra_mode_fails_closed_on_base_only_search_paths() {
    let (_dir, path) = temp_db();
    let store = extra_store(&path, 0x11);

    assert!(matches!(store.vector_search(4, &[1.0; 4], 1), Err(StoreError::SeamUnavailable)));
    assert!(matches!(store.fts_search("anything", 1), Err(StoreError::SeamUnavailable)));
    assert!(matches!(store.refresh_fts(), Err(StoreError::SeamUnavailable)));

    let (_base_dir, base_path) = temp_db();
    let base = Store::open(&base_path, &key(DB_KEY)).expect("open base store");
    assert!(matches!(base.list_embeddings(MODEL, 4), Err(StoreError::SeamUnavailable)));
}

#[test]
fn a_wrong_encryptor_key_fails_closed_with_a_decrypt_error() {
    let (_dir, path) = temp_db();
    let chunk_id = [0x51_u8; 32];
    let vector = [1.0_f32, 2.0, 3.0, 4.0];
    {
        let store = extra_store(&path, 0x11);
        store
            .sync_file("notes/d.md", &[0x52_u8; 32], &[chunk(&chunk_id, CANARY)])
            .expect("sync file");
        store.insert_embedding(&chunk_id, MODEL, 4, &vector).expect("insert embedding");
    }

    let wrong = extra_store(&path, 0x22);
    assert!(matches!(wrong.all_chunks(), Err(StoreError::Seam(SeamError::Decrypt))));
    assert!(matches!(
        wrong.chunk_metadata(&[rowid(&path, &chunk_id)]),
        Err(StoreError::Seam(SeamError::Decrypt))
    ));
    assert!(matches!(wrong.list_embeddings(MODEL, 4), Err(StoreError::Seam(SeamError::Decrypt))));
}

#[test]
fn a_base_written_database_read_in_extra_mode_fails_closed() {
    let (_dir, path) = temp_db();
    let chunk_id = [0x61_u8; 32];
    Store::open(&path, &key(DB_KEY))
        .expect("open base store")
        .sync_file("notes/e.md", &[0x62_u8; 32], &[chunk(&chunk_id, "plain chunk")])
        .expect("sync file");

    let extra = extra_store(&path, 0x11);
    assert!(matches!(extra.all_chunks(), Err(StoreError::Seam(SeamError::Decrypt))));
}

#[test]
fn delete_file_garbage_collects_encrypted_embeddings() {
    let (_dir, path) = temp_db();
    let chunk_id = [0x71_u8; 32];
    let store = extra_store(&path, 0x11);
    store.sync_file("notes/f.md", &[0x72_u8; 32], &[chunk(&chunk_id, CANARY)]).expect("sync file");
    store.insert_embedding(&chunk_id, MODEL, 4, &[1.0, 2.0, 3.0, 4.0]).expect("insert embedding");
    assert_eq!(store.list_embeddings(MODEL, 4).expect("list before delete").len(), 1);

    let file_id = store.list_files().expect("list files")[0].id;
    store.delete_file(file_id).expect("delete file");

    assert_eq!(store.list_embeddings(MODEL, 4).expect("list after delete"), Vec::new());
    assert!(!store.embedding_exists(MODEL, 4, &chunk_id).expect("lookup after delete"));
}
