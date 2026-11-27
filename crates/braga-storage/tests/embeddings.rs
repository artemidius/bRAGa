// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the `vec0` activation and `model_registry` pinning surface, over the real
//! `SQLCipher` + `sqlite-vec` build.
//!
//! The round-trip test is the first green signal for the `vec_f32` BLOB mechanics: it inserts a
//! known vector through [`Store::insert_embedding`] and reads the stored BLOB back verbatim.

use std::path::{Path, PathBuf};

use braga_storage::{Key, NewChunk, Store, StoreError};
use rusqlite::Connection;
use tempfile::TempDir;

const MODEL: &str = "test-model";

fn temp_db() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let path = dir.path().join("braga.sqlite3");
    (dir, path)
}

fn key(byte: u8) -> Key {
    Key::from_bytes([byte; 32])
}

fn open_store(path: &Path) -> Store {
    Store::open(path, &key(0xA1)).expect("open encrypted store")
}

/// Opens the encrypted database outside [`Store`], so tests can inspect virtual tables directly.
fn open_raw(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open raw encrypted database");
    conn.key(None::<&str>, key(0xA1).as_bytes(), false).expect("apply key");
    conn
}

fn chunk<'a>(id: &'a [u8; 32], content: &'a str) -> NewChunk<'a> {
    NewChunk { id, byte_offset: 0, line_start: 1, line_end: 1, content }
}

/// Seeds one file with one chunk and returns the chunk id.
fn seed_chunk(store: &Store) -> [u8; 32] {
    let id = [0x11_u8; 32];
    store.sync_file("notes/a.md", &[0x22_u8; 32], &[chunk(&id, "chunk body")]).expect("sync file");
    id
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get(0),
    )
    .expect("inspect sqlite_master")
}

fn count_embeddings(conn: &Connection, dim: u32) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM embeddings_d{dim}"), [], |row| row.get(0))
        .expect("count embeddings")
}

/// Decodes the little-endian `f32` BLOB a `vec0` embedding column stores.
fn decode(blob: &[u8]) -> Vec<f32> {
    blob.as_chunks::<4>().0.iter().map(|bytes| f32::from_le_bytes(*bytes)).collect()
}

#[test]
fn register_model_pins_the_shape_and_is_idempotent() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);

    assert_eq!(store.registered_model(MODEL).expect("read empty registry"), None);

    store.register_model(MODEL, 4, "cosine").expect("register model");
    assert_eq!(
        store.registered_model(MODEL).expect("read registry"),
        Some((4, "cosine".to_owned()))
    );

    store.register_model(MODEL, 4, "cosine").expect("re-registering the identical row is a no-op");
    assert!(matches!(
        store.register_model(MODEL, 8, "cosine"),
        Err(StoreError::ModelConflict { model }) if model == MODEL
    ));
    assert!(matches!(
        store.register_model(MODEL, 4, "l2"),
        Err(StoreError::ModelConflict { model }) if model == MODEL
    ));
    assert_eq!(
        store.registered_model(MODEL).expect("read pinned registry"),
        Some((4, "cosine".to_owned())),
        "a rejected change must not rewrite the pinned row"
    );
}

#[test]
fn ensure_embedding_table_is_idempotent_and_validates_its_parameters() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);

    store.ensure_embedding_table(4, "cosine").expect("create embeddings_d4");
    store.ensure_embedding_table(4, "cosine").expect("creating it again is a no-op");
    store.ensure_embedding_table(8, "l2").expect("create a second dimension");

    let conn = open_raw(&path);
    assert!(table_exists(&conn, "embeddings_d4"));
    assert!(table_exists(&conn, "embeddings_d8"));

    for dim in [0, 16_385] {
        assert!(matches!(
            store.ensure_embedding_table(dim, "cosine"),
            Err(StoreError::InvalidVec0Parameter { parameter: "dim" })
        ));
    }
    for metric in ["l1", "dot", ""] {
        assert!(matches!(
            store.ensure_embedding_table(4, metric),
            Err(StoreError::InvalidVec0Parameter { parameter: "metric" })
        ));
    }
}

#[test]
fn insert_embedding_round_trips_a_known_vector_through_vec_f32() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let chunk_id = seed_chunk(&store);
    let vector = [0.25_f32, -1.5, 2.0, 3.75];

    store.ensure_embedding_table(4, "cosine").expect("create embeddings_d4");
    store.register_model(MODEL, 4, "cosine").expect("register model");
    store.insert_embedding(&chunk_id, MODEL, 4, &vector).expect("insert embedding");

    assert!(store.embedding_exists(MODEL, 4, &chunk_id).expect("lookup hit"));
    assert!(
        !store.embedding_exists("other-model", 4, &chunk_id).expect("lookup miss on the model"),
        "the cache key includes the model"
    );
    assert!(
        !store.embedding_exists(MODEL, 4, &[0x99_u8; 32]).expect("lookup miss on the chunk"),
        "the cache key includes the chunk id"
    );

    let conn = open_raw(&path);
    let (rowid, stored): (i64, Vec<u8>) = conn
        .query_row(
            "SELECT rowid, embedding FROM embeddings_d4 WHERE chunk_id = ?1",
            [chunk_id.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read the stored embedding");
    assert_eq!(decode(&stored), vector, "the vector must round-trip through vec_f32");
    assert_eq!(
        rowid,
        conn.query_row("SELECT rowid FROM chunks WHERE id = ?1", [chunk_id.as_slice()], |row| {
            row.get(0)
        })
        .expect("read the chunk rowid"),
        "the vec0 rowid must equal chunks.rowid"
    );
}

#[test]
fn a_wrong_length_vector_fails_closed() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let chunk_id = seed_chunk(&store);

    store.ensure_embedding_table(4, "cosine").expect("create embeddings_d4");

    assert!(matches!(
        store.insert_embedding(&chunk_id, MODEL, 4, &[1.0, 2.0, 3.0]),
        Err(StoreError::Backend { .. })
    ));
    assert_eq!(count_embeddings(&open_raw(&path), 4), 0);
}

#[test]
fn an_unknown_chunk_id_fails_closed_without_an_orphan_row() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);

    store.ensure_embedding_table(4, "cosine").expect("create embeddings_d4");

    assert!(matches!(
        store.insert_embedding(&[0x99_u8; 32], MODEL, 4, &[1.0, 2.0, 3.0, 4.0]),
        Err(StoreError::Backend { .. })
    ));
    assert_eq!(
        count_embeddings(&open_raw(&path), 4),
        0,
        "an unresolvable chunk must not leave an auto-rowid embedding behind"
    );
}
