// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the scanner-facing [`Store`] surface: `list_files`, `sync_file`,
//! `delete_file`, and `refresh_fts`, over the real `SQLCipher` + FTS5 build.
//!
//! The tests inspect rows through a second raw connection, exactly as the schema tests do, so the
//! assertions read the same encrypted database the `Store` wrote.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use braga_storage::{Key, NewChunk, Store};
use rusqlite::Connection;
use tempfile::TempDir;

fn temp_db() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let path = dir.path().join("braga.sqlite3");
    (dir, path)
}

fn key(byte: u8) -> Key {
    Key::from_bytes([byte; 32])
}

/// Builds the sidecar path (`<db>-wal`, `<db>-shm`, ...) by appending to the file name.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Opens the encrypted database outside [`Store`], so tests can inspect and mutate rows directly.
fn open_raw(path: &Path, key: &Key) -> Connection {
    let conn = Connection::open(path).expect("open raw encrypted database");
    conn.key(None::<&str>, key.as_bytes(), false).expect("apply key");
    conn
}

/// A `NewChunk` borrowing `id` and `content`; offsets are irrelevant to these assertions.
fn chunk<'a>(id: &'a [u8; 32], content: &'a str) -> NewChunk<'a> {
    NewChunk { id, byte_offset: 0, line_start: 1, line_end: 1, content }
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).expect("count rows")
}

/// Number of `chunks_fts` entries the FTS5 index matches for `term`.
fn fts_hits(conn: &Connection, term: &str) -> i64 {
    conn.query_row("SELECT count(*) FROM chunks_fts WHERE chunks_fts MATCH ?1", [term], |row| {
        row.get(0)
    })
    .expect("count fts hits")
}

fn version(conn: &Connection, path: &str) -> i64 {
    conn.query_row("SELECT version FROM files WHERE path = ?1", [path], |row| row.get(0))
        .expect("read file version")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
}

/// A long, per-run value that is never expected to appear in a file at rest.
fn canary() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after the epoch")
        .as_nanos();
    format!("BRAGA-STAGE-6A-PLAINTEXT-CANARY-{nanos:032x}-").repeat(32)
}

#[test]
fn list_files_is_empty_then_returns_the_synced_row() {
    let (_dir, path) = temp_db();
    let store = Store::open(&path, &key(0x11)).expect("open store");
    assert!(store.list_files().expect("list files").is_empty(), "a fresh store has no files");

    let hash = [0xA1_u8; 32];
    let id = [0x01_u8; 32];
    store.sync_file("notes/a.md", &hash, &[chunk(&id, "first chunk body")]).expect("sync file");

    let rows = store.list_files().expect("list files");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, "notes/a.md");
    assert_eq!(rows[0].content_hash.as_slice(), hash.as_slice());
    assert!(rows[0].id > 0, "the row carries its integer primary key");
}

#[test]
fn sync_file_add_writes_the_file_chunks_and_fts_entries() {
    let (_dir, path) = temp_db();
    let store = Store::open(&path, &key(0x12)).expect("open store");
    let (alpha, beta) = ([0x11_u8; 32], [0x22_u8; 32]);

    store
        .sync_file(
            "a.md",
            &[0xB2_u8; 32],
            &[chunk(&alpha, "alpha needle"), chunk(&beta, "beta needle")],
        )
        .expect("sync file");

    let conn = open_raw(&path, &key(0x12));
    assert_eq!(count(&conn, "SELECT count(*) FROM files"), 1);
    assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 2);
    assert_eq!(fts_hits(&conn, "alpha"), 1);
    assert_eq!(fts_hits(&conn, "beta"), 1);
    assert_eq!(version(&conn, "a.md"), 1);
}

#[test]
fn sync_file_modify_replaces_chunks_and_fts_entries() {
    let (_dir, path) = temp_db();
    let store = Store::open(&path, &key(0x13)).expect("open store");
    let (alpha, gamma, delta) = ([0x11_u8; 32], [0x22_u8; 32], [0x33_u8; 32]);

    store.sync_file("a.md", &[0xB1_u8; 32], &[chunk(&alpha, "alpha oldword")]).expect("sync file");
    store
        .sync_file(
            "a.md",
            &[0xB2_u8; 32],
            &[chunk(&gamma, "gamma newword"), chunk(&delta, "delta newword")],
        )
        .expect("resync file");

    let conn = open_raw(&path, &key(0x13));
    assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 2, "the old chunk is gone");
    assert_eq!(fts_hits(&conn, "oldword"), 0, "the old chunk is unindexed");
    assert_eq!(fts_hits(&conn, "newword"), 2, "both replacement chunks are indexed");
    assert_eq!(version(&conn, "a.md"), 2, "a detected change bumps the version");
    let stored: Vec<u8> = conn
        .query_row("SELECT content_hash FROM files WHERE path = 'a.md'", [], |row| row.get(0))
        .expect("read content hash");
    assert_eq!(stored, [0xB2_u8; 32], "the hash is replaced with the new one");
}

#[test]
fn delete_file_removes_chunks_fts_and_the_file_row() {
    let (_dir, path) = temp_db();
    let store = Store::open(&path, &key(0x14)).expect("open store");
    let id = [0x11_u8; 32];

    store.sync_file("a.md", &[0xB3_u8; 32], &[chunk(&id, "alpha needle")]).expect("sync file");
    let file_id = store.list_files().expect("list files")[0].id;
    store.delete_file(file_id).expect("delete file");

    let conn = open_raw(&path, &key(0x14));
    assert_eq!(count(&conn, "SELECT count(*) FROM files"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 0, "chunks cascade with the file");
    assert_eq!(fts_hits(&conn, "alpha"), 0, "no stale FTS entry survives the delete");
}

#[test]
fn delete_file_on_an_unknown_id_is_a_no_op() {
    let (_dir, path) = temp_db();
    let store = Store::open(&path, &key(0x15)).expect("open store");
    let id = [0x11_u8; 32];
    store.sync_file("a.md", &[0xB4_u8; 32], &[chunk(&id, "alpha needle")]).expect("sync file");

    store.delete_file(424_242).expect("an unknown id must not fail");

    assert_eq!(store.list_files().expect("list files").len(), 1, "the real row is untouched");
    let conn = open_raw(&path, &key(0x15));
    assert_eq!(fts_hits(&conn, "alpha"), 1);
}

#[test]
fn deleting_chunks_before_fts_leaves_a_stale_match() {
    let (_dir, path) = temp_db();
    let store = Store::open(&path, &key(0x16)).expect("open store");
    let id = [0x11_u8; 32];
    store.sync_file("a.md", &[0xB5_u8; 32], &[chunk(&id, "alpha needle")]).expect("sync file");

    // Pins the ordering invariant: removing the `chunks` rows without first unindexing them
    // leaves the FTS entry behind, because `chunks` deletion does not cascade into `chunks_fts`.
    {
        let conn = open_raw(&path, &key(0x16));
        conn.execute("DELETE FROM chunks", []).expect("delete chunks behind the index's back");
    }

    let conn = open_raw(&path, &key(0x16));
    assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 0);
    assert_eq!(fts_hits(&conn, "alpha"), 1, "the stale entry is the leak `sync_file` avoids");
}

#[test]
fn refresh_fts_rebuilds_the_index_from_the_content_table() {
    let (_dir, path) = temp_db();
    let store = Store::open(&path, &key(0x17)).expect("open store");
    let id = [0x11_u8; 32];
    store.sync_file("a.md", &[0xB6_u8; 32], &[chunk(&id, "alpha needle")]).expect("sync file");

    let restored = [0x22_u8; 32];
    {
        let conn = open_raw(&path, &key(0x17));
        let file_id: i64 = conn
            .query_row("SELECT id FROM files WHERE path = 'a.md'", [], |row| row.get(0))
            .expect("read file id");
        conn.execute(
            "INSERT INTO chunks (id, file_id, byte_offset, line_start, line_end, content)
             VALUES (?1, ?2, 0, 1, 1, 'restored word')",
            rusqlite::params![restored, file_id],
        )
        .expect("insert a chunk without indexing it");
    }

    let conn = open_raw(&path, &key(0x17));
    assert_eq!(fts_hits(&conn, "restored"), 0, "the raw insert bypassed the index");
    drop(conn);

    store.refresh_fts().expect("rebuild the fts index");

    let conn = open_raw(&path, &key(0x17));
    assert_eq!(fts_hits(&conn, "restored"), 1, "a rebuild indexes every chunk row");
    assert_eq!(fts_hits(&conn, "alpha"), 1, "and keeps the existing entries");
}

#[test]
fn chunk_content_is_encrypted_at_rest() {
    let canary = canary();
    let (_dir, path) = temp_db();
    let store = Store::open(&path, &key(0x18)).expect("open store");
    let id = [0x11_u8; 32];

    store.sync_file("secret.md", &[0xB7_u8; 32], &[chunk(&id, &canary)]).expect("sync file");
    store.checkpoint().expect("checkpoint WAL");

    let mut inspected = 0;
    for file in [path.clone(), sidecar(&path, "-wal"), sidecar(&path, "-shm")] {
        let Ok(bytes) = fs::read(&file) else { continue };
        inspected += 1;
        assert!(
            !contains(&bytes, canary.as_bytes()),
            "plaintext chunk content found in {}",
            file.display()
        );
    }
    assert!(inspected >= 1, "the main database file must exist");
}
