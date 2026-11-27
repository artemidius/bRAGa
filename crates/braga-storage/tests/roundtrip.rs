// SPDX-License-Identifier: Apache-2.0

//! Integration tests for key management, schema v1, migrations, and the encrypted single-file
//! invariant. They exercise the real `SQLCipher` + `sqlite-vec` + FTS5 build linked into the test
//! binary.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use braga_storage::{Key, KeyFileError, Store, StoreError};
use rusqlite::{Connection, OptionalExtension as _, params};
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
    format!("BRAGA-STAGE-4-PLAINTEXT-CANARY-{nanos:032x}-").repeat(32)
}

fn hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(64);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

/// Opens the encrypted database outside [`Store`], so tests can inspect and seed rows directly.
fn open_raw(path: &Path, key: &Key) -> Connection {
    let conn = Connection::open(path).expect("open raw encrypted database");
    conn.key(None::<&str>, key.as_bytes(), false).expect("apply key");
    conn
}

fn schema_version(conn: &Connection) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0))
        .optional()
        .expect("read schema version")
}

fn migration_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM migrations", [], |row| row.get(0))
        .expect("count applied migrations")
}

#[test]
fn generate_produces_distinct_keys() {
    let first = Key::generate();
    let second = Key::generate();

    assert_ne!(first, second, "two generated keys must differ");
    assert_ne!(first.as_bytes(), &[0_u8; 32], "a generated key must not be all zeroes");
    assert_eq!(format!("{first:?}"), "Key([REDACTED])");
}

#[test]
fn key_file_roundtrip_and_unix_permissions() {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let home = dir.path().join("braga-home");
    let path = home.join("key");
    let key = Key::generate();

    key.to_file(&path).expect("write key file");
    assert_eq!(Key::from_file(&path).expect("read key file"), key);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let file_mode = fs::metadata(&path).expect("key metadata").permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "key file must be owner-only");
        let dir_mode = fs::metadata(&home).expect("parent metadata").permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "key directory must be owner-only");
    }
}

#[test]
fn key_file_rejects_wrong_length() {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let path = dir.path().join("short-key");
    fs::write(&path, [0xAA_u8; 31]).expect("write short key file");

    let error = Key::from_file(&path).expect_err("short key must be rejected");
    assert!(matches!(error, KeyFileError::InvalidLength { found: 31, .. }));
    assert!(error.to_string().contains("31"), "the length should be reported");
}

#[test]
fn key_file_never_clobbers() {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let path = dir.path().join("key");
    fs::write(&path, [0x11_u8; 32]).expect("write existing key file");

    let error = Key::generate().to_file(&path).expect_err("existing key must not be clobbered");
    assert!(matches!(error, KeyFileError::AlreadyExists { .. }));
    assert_eq!(fs::read(&path).expect("read existing key"), [0x11_u8; 32]);
}

#[test]
fn fresh_database_is_schema_v1() {
    let (_dir, path) = temp_db();
    let key = key(0x11);
    Store::open(&path, &key).expect("open encrypted database").close().expect("close");

    let conn = open_raw(&path, &key);
    assert_eq!(schema_version(&conn).as_deref(), Some("1"));
    assert_eq!(migration_count(&conn), 1);

    for table in ["meta", "migrations", "files", "chunks", "chunks_fts", "model_registry"] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
                [table],
                |row| row.get(0),
            )
            .expect("query sqlite_master");
        assert!(exists, "schema v1 must create {table}");
    }
}

#[test]
fn reopening_is_idempotent() {
    let (_dir, path) = temp_db();
    let key = key(0x12);

    for _ in 0..2 {
        Store::open(&path, &key).expect("open encrypted database").close().expect("close");
    }

    let conn = open_raw(&path, &key);
    assert_eq!(schema_version(&conn).as_deref(), Some("1"));
    assert_eq!(migration_count(&conn), 1);
}

#[test]
fn key_before_any_statement() {
    let (_dir, path) = temp_db();
    let key = key(0x55);

    // `open` succeeds only when the key is applied first: `SQLCipher` 4.7+ hard-fails on any
    // pre-key statement.
    Store::open(&path, &key)
        .expect("key must be applied before any other statement")
        .close()
        .expect("close");
}

#[test]
fn wrong_key_is_rejected() {
    let (_dir, path) = temp_db();

    {
        let store = Store::open(&path, &key(0x21)).expect("open encrypted database");
        store.checkpoint().expect("checkpoint WAL");
    }

    let error = Store::open(&path, &key(0x22)).err().expect("a different key must not decrypt");
    assert!(matches!(error, StoreError::WrongKey), "expected WrongKey, got: {error}");
}

#[test]
fn newer_database_is_rejected() {
    let (_dir, path) = temp_db();
    let key = key(0x31);
    Store::open(&path, &key).expect("open encrypted database").close().expect("close");

    {
        let conn = open_raw(&path, &key);
        conn.execute("UPDATE meta SET value = '2' WHERE key = 'schema_version'", [])
            .expect("bump stored schema version");
    }

    let error = Store::open(&path, &key).err().expect("newer database must not open");
    assert!(
        matches!(error, StoreError::NewerDatabase { found: 2, supported: 1 }),
        "expected NewerDatabase, got: {error}"
    );
}

#[test]
fn errors_never_leak_key_material() {
    let (_dir, path) = temp_db();
    let correct = Key::from_bytes([0xA5; 32]);
    let wrong = Key::from_bytes([0x5A; 32]);
    Store::open(&path, &correct).expect("open encrypted database").close().expect("close");

    let error = Store::open(&path, &wrong).err().expect("wrong key must fail");
    let wrong_hex = hex(wrong.as_bytes());
    for rendered in [error.to_string(), format!("{error:?}")] {
        assert!(!rendered.contains(&wrong_hex), "error leaked the key's hex form: {rendered}");
        assert!(!rendered.contains("5a5a"), "error leaked key bytes: {rendered}");
    }
    assert_eq!(format!("{wrong:?}"), "Key([REDACTED])");

    // A keyfile whose contents are key material must not have them echoed back in the error.
    let dir = tempfile::tempdir().expect("create temporary directory");
    let path = dir.path().join("bad-key");
    fs::write(&path, wrong_hex.as_bytes()).expect("write bad key file");
    let error = Key::from_file(&path).expect_err("wrong length must fail");
    assert!(!error.to_string().contains(&wrong_hex), "keyfile error leaked the contents");
}

#[test]
fn single_file_invariant() {
    let (_dir, path) = temp_db();
    let backup = path.with_file_name("backup.sqlite3");
    let db_key = key(0x41);
    let canary = canary();
    let wal = sidecar(&path, "-wal");
    let shm = sidecar(&path, "-shm");

    let store = Store::open(&path, &db_key).expect("open encrypted database");
    let writer = open_raw(&path, &db_key);
    writer
        .execute(
            "INSERT INTO files (path, content_hash, updated_at) VALUES (?1, ?2, 0)",
            params![canary, canary.as_bytes()],
        )
        .expect("insert canary row");

    // The live WAL must not contain the canary in plaintext.
    let wal_bytes = fs::read(&wal).expect("read -wal");
    assert!(!wal_bytes.is_empty(), "-wal must be non-empty for this assertion");
    assert!(!contains(&wal_bytes, canary.as_bytes()), "plaintext canary found in -wal");
    if shm.exists() {
        let shm_bytes = fs::read(&shm).expect("read -shm");
        assert!(!contains(&shm_bytes, canary.as_bytes()), "plaintext canary found in -shm");
    }
    drop(writer);

    store.checkpoint().expect("checkpoint WAL");
    let db_bytes = fs::read(&path).expect("read main database");
    assert!(!contains(&db_bytes, canary.as_bytes()), "plaintext canary found in main db");
    let wal_bytes = fs::read(&wal).expect("read truncated -wal");
    assert!(!contains(&wal_bytes, canary.as_bytes()), "plaintext canary found in -wal");
    if shm.exists() {
        let shm_bytes = fs::read(&shm).expect("read -shm");
        assert!(!contains(&shm_bytes, canary.as_bytes()), "plaintext canary found in -shm");
    }

    store.vacuum_into(&backup).expect("vacuum into an encrypted backup");
    let backup_bytes = fs::read(&backup).expect("read backup");
    assert!(!contains(&backup_bytes, canary.as_bytes()), "plaintext canary found in backup");
    drop(store);

    // The backup reopens with the key...
    let conn = open_raw(&backup, &db_key);
    let stored: String =
        conn.query_row("SELECT path FROM files", [], |row| row.get(0)).expect("read canary");
    assert_eq!(stored, canary);
    drop(conn);

    // ...and fails without it.
    let error = Store::open(&backup, &key(0x42)).err().expect("backup must need the key");
    assert!(matches!(error, StoreError::WrongKey), "expected WrongKey, got: {error}");

    // The main database is still readable after the backup.
    let conn = open_raw(&path, &db_key);
    let stored: String =
        conn.query_row("SELECT path FROM files", [], |row| row.get(0)).expect("read canary");
    assert_eq!(stored, canary);
}
