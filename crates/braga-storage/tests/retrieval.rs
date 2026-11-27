// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the retrieval-facing [`Store`] surface: `vector_search`, `fts_search`, and
//! `chunk_metadata`, over the real `SQLCipher` + `sqlite-vec` + FTS5 build.
//!
//! The fixtures are deliberately small and explicit: every expected order is checked against the
//! rowids read from a second raw connection, so the assertions do not depend on insertion order.

use std::path::{Path, PathBuf};

use braga_storage::{ChunkRow, Key, NewChunk, Store, StoreError};
use rusqlite::Connection;
use tempfile::TempDir;

const MODEL: &str = "test-model";

fn temp_db() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let path = dir.path().join("braga.sqlite3");
    (dir, path)
}

fn key() -> Key {
    Key::from_bytes([0xA1; 32])
}

fn open_store(path: &Path) -> Store {
    Store::open(path, &key()).expect("open encrypted store")
}

/// Opens the encrypted database outside [`Store`], so tests can read rowids directly.
fn open_raw(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open raw encrypted database");
    conn.key(None::<&str>, key().as_bytes(), false).expect("apply key");
    conn
}

fn rowid_of(path: &Path, id: &[u8; 32]) -> i64 {
    open_raw(path)
        .query_row("SELECT rowid FROM chunks WHERE id = ?1", [id.as_slice()], |row| row.get(0))
        .expect("read chunk rowid")
}

fn chunk<'a>(id: &'a [u8; 32], line_start: i64, line_end: i64, content: &'a str) -> NewChunk<'a> {
    NewChunk { id, byte_offset: 0, line_start, line_end, content }
}

/// Seeds the four-dimension fixture shared by the vector tests.
fn seed_vectors(store: &Store) -> [[u8; 32]; 3] {
    let ids = [[0x01_u8; 32], [0x02_u8; 32], [0x03_u8; 32]];
    let chunks = [
        chunk(&ids[0], 1, 1, "alpha"),
        chunk(&ids[1], 2, 2, "beta"),
        chunk(&ids[2], 3, 3, "gamma"),
    ];
    store.sync_file("docs/vectors.md", &[0x10_u8; 32], &chunks).expect("seed vector chunks");
    store.ensure_embedding_table(4, "l2").expect("create embeddings_d4");
    store.register_model(MODEL, 4, "l2").expect("register model");
    let vectors = [[1.0_f32, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]];
    for (id, vector) in ids.iter().zip(&vectors) {
        store.insert_embedding(id, MODEL, 4, vector).expect("insert embedding");
    }
    ids
}

#[test]
fn vector_search_returns_nearest_neighbors_in_distance_order() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let ids = seed_vectors(&store);

    let hits = store.vector_search(4, &[1.0, 0.0, 0.0, 0.0], 3).expect("search vectors");

    let expected: Vec<i64> = ids.iter().map(|id| rowid_of(&path, id)).collect();
    assert_eq!(hits.iter().map(|hit| hit.rowid).collect::<Vec<_>>(), expected);
    assert!(hits[0].distance.abs() < 1e-6, "the identical vector is the nearest");
    assert!(
        hits.windows(2).all(|pair| pair[0].distance <= pair[1].distance),
        "hits must arrive in ascending distance order"
    );
}

#[test]
fn vector_search_caps_results_at_top_k() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let ids = seed_vectors(&store);

    let hits = store.vector_search(4, &[1.0, 0.0, 0.0, 0.0], 2).expect("search vectors");

    let expected: Vec<i64> = ids[..2].iter().map(|id| rowid_of(&path, id)).collect();
    assert_eq!(hits.iter().map(|hit| hit.rowid).collect::<Vec<_>>(), expected);
    assert_eq!(hits.len(), 2);
}

#[test]
fn fts_search_returns_matches_in_relevance_order() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let sparse = [0x11_u8; 32];
    let unrelated = [0x12_u8; 32];
    let dense = [0x13_u8; 32];
    let chunks = [
        chunk(&sparse, 1, 1, "widget"),
        chunk(&unrelated, 2, 2, "irrelevant text"),
        chunk(&dense, 3, 3, "widget widget widget widget"),
    ];
    store.sync_file("docs/keywords.md", &[0x20_u8; 32], &chunks).expect("seed keyword chunks");

    let hits = store.fts_search("widget", 5).expect("search keywords");

    assert_eq!(
        hits,
        vec![rowid_of(&path, &dense), rowid_of(&path, &sparse)],
        "the higher term frequency must rank first, and unrelated chunks must not match"
    );
    assert_eq!(
        store.fts_search("widget", 1).expect("capped search"),
        vec![rowid_of(&path, &dense)]
    );
}

#[test]
fn chunk_metadata_joins_the_path_line_range_and_content() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let a_first = [0x21_u8; 32];
    let a_second = [0x22_u8; 32];
    let b_only = [0x23_u8; 32];
    store
        .sync_file(
            "docs/a.md",
            &[0x30_u8; 32],
            &[chunk(&a_first, 1, 2, "first chunk"), chunk(&a_second, 3, 4, "second chunk")],
        )
        .expect("seed docs/a.md");
    store
        .sync_file("docs/b.md", &[0x31_u8; 32], &[chunk(&b_only, 5, 6, "third chunk")])
        .expect("seed docs/b.md");

    let mut rows = store
        .chunk_metadata(&[rowid_of(&path, &a_first), rowid_of(&path, &b_only)])
        .expect("read metadata");

    rows.sort_by(|left, right| left.chunk_id.cmp(&right.chunk_id));
    assert_eq!(
        rows[0],
        ChunkRow {
            chunk_id: a_first.to_vec(),
            path: "docs/a.md".to_owned(),
            line_start: 1,
            line_end: 2,
            content: "first chunk".to_owned(),
        }
    );
    assert_eq!(rows[1].path, "docs/b.md");
    assert_eq!((rows[1].line_start, rows[1].line_end), (5, 6));
    assert_eq!(rows[1].content, "third chunk");

    assert!(
        store.chunk_metadata(&[]).expect("empty metadata query").is_empty(),
        "an empty rowid set must not query at all"
    );
}

#[test]
fn fts_search_rejects_a_malformed_expression() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    store
        .sync_file("docs/a.md", &[0x40_u8; 32], &[chunk(&[0x41_u8; 32], 1, 1, "text")])
        .expect("seed one chunk");

    assert!(matches!(store.fts_search("\"", 5), Err(StoreError::Backend { .. })));
}
