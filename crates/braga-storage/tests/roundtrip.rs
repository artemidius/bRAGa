// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the storage spike. They exercise the real `SQLCipher` + `sqlite-vec` +
//! FTS5 build linked into the test binary.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use braga_storage::{
    key::Key,
    store::{Store, StoreError},
};
use tempfile::TempDir;

fn key(byte: u8) -> Key {
    Key::from_bytes([byte; 32])
}

fn temp_db() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let path = dir.path().join("braga.sqlite3");
    (dir, path)
}

/// Builds the sidecar path (`<db>-wal`, `<db>-shm`, ...) by appending to the file name.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
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
    format!("BRAGA-STAGE-3-PLAINTEXT-CANARY-{nanos:032x}-").repeat(32)
}

#[test]
fn roundtrip_encrypted_wal() {
    let (_dir, path) = temp_db();
    let key = key(0x11);
    let marker = "stage-3-roundtrip-marker";

    {
        let store = Store::open(&path, &key).expect("open encrypted database");
        store.create_scratch().expect("create scratch tables");
        store.put(marker).expect("insert marker");

        let wal = sidecar(&path, "-wal");
        let len = fs::metadata(&wal).expect("read -wal metadata").len();
        assert!(len > 0, "-wal must be non-empty before checkpoint");

        store.checkpoint().expect("checkpoint WAL");
    }

    let store = Store::open(&path, &key).expect("reopen with the same key");
    assert_eq!(store.get().expect("read marker back"), marker);
}

#[test]
fn no_plaintext_at_rest() {
    let (_dir, path) = temp_db();
    let key = key(0x22);
    let canary = canary();

    {
        let store = Store::open(&path, &key).expect("open encrypted database");
        store.create_scratch().expect("create scratch tables");
        store.put(&canary).expect("insert canary");

        let wal = sidecar(&path, "-wal");
        let shm = sidecar(&path, "-shm");

        // Scan the live, non-empty WAL before checkpointing; a truncated WAL proves nothing.
        let wal_bytes = fs::read(&wal).expect("read -wal");
        assert!(!wal_bytes.is_empty(), "-wal must be non-empty for this assertion");
        assert!(!contains(&wal_bytes, canary.as_bytes()), "plaintext canary found in -wal");
        if shm.exists() {
            let shm_bytes = fs::read(&shm).expect("read -shm");
            assert!(!contains(&shm_bytes, canary.as_bytes()), "plaintext canary found in -shm");
        }

        store.checkpoint().expect("checkpoint WAL");

        let db_bytes = fs::read(&path).expect("read main database");
        assert!(!contains(&db_bytes, canary.as_bytes()), "plaintext canary found in main db");
        let wal_bytes = fs::read(&wal).expect("read truncated -wal");
        assert!(!contains(&wal_bytes, canary.as_bytes()), "plaintext canary found in -wal");
        if shm.exists() {
            let shm_bytes = fs::read(&shm).expect("read -shm");
            assert!(!contains(&shm_bytes, canary.as_bytes()), "plaintext canary found in -shm");
        }
    }

    let store = Store::open(&path, &key).expect("reopen with the same key");
    assert_eq!(store.get().expect("read canary back"), canary);
}

#[test]
fn extensions_available() {
    let (_dir, path) = temp_db();

    // Opening a store registers `sqlite-vec` as a process-wide auto-extension.
    let store = Store::open(&path, &key(0x33)).expect("open encrypted database");
    store.create_scratch().expect("create FTS5 and vec0 tables");

    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory database");
    let version: String =
        conn.query_row("SELECT vec_version()", [], |row| row.get(0)).expect("load sqlite-vec");
    assert!(version.starts_with('v'), "unexpected sqlite-vec version: {version}");

    conn.execute_batch(
        "CREATE VIRTUAL TABLE docs USING fts5(body);
         INSERT INTO docs(body) VALUES ('sqlcipher enables encrypted storage');",
    )
    .expect("create and fill FTS5 table");

    let matches: i64 = conn
        .query_row("SELECT count(*) FROM docs WHERE docs MATCH 'encrypted'", [], |row| row.get(0))
        .expect("query FTS5 index");
    assert_eq!(matches, 1);
}

#[test]
fn wrong_key_rejected() {
    let (_dir, path) = temp_db();

    {
        let store = Store::open(&path, &key(0x44)).expect("open encrypted database");
        store.create_scratch().expect("create scratch tables");
        store.put("secret-marker").expect("insert marker");
    }

    let error = Store::open(&path, &key(0x45))
        .and_then(|store| store.get())
        .expect_err("a different key must not decrypt the database");
    assert!(matches!(error, StoreError::Sqlite(_)), "expected a SQLite error, got: {error}");
    assert!(error.to_string().contains("not a database"), "expected SQLITE_NOTADB, got: {error}");
}

#[test]
fn key_before_any_statement() {
    let (_dir, path) = temp_db();
    let key = key(0x55);

    // `open` succeeds only when `PRAGMA key` is the first statement: `SQLCipher` 4.7+ hard-fails on
    // any pre-key statement.
    let store = Store::open(&path, &key).expect("key must be applied before any other statement");
    store.create_scratch().expect("create scratch tables");
    store.put("key-first-marker").expect("insert marker");
    assert_eq!(store.get().expect("read marker back"), "key-first-marker");
}
