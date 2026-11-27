// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the synchronous [`Scanner`] over real temporary files and a real
//! encrypted [`Store`].

use std::{
    fs,
    path::{Path, PathBuf},
};

use braga::scanner::{ScanError, ScanOutcome, Scanner};
use braga_storage::{Key, Store};
use rusqlite::Connection;
use tempfile::TempDir;

/// A scan root plus an encrypted database, both in temporary directories.
struct Fixture {
    root: TempDir,
    db_dir: TempDir,
    key: Key,
}

impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("create scan root"),
            db_dir: tempfile::tempdir().expect("create database directory"),
            key: Key::from_bytes([0x5A; 32]),
        }
    }

    fn root(&self) -> &Path {
        self.root.path()
    }

    fn db(&self) -> PathBuf {
        self.db_dir.path().join("braga.sqlite3")
    }

    fn store(&self) -> Store {
        Store::open(&self.db(), &self.key).expect("open store")
    }

    fn scanner(&self) -> Scanner {
        Scanner::new(self.root().to_path_buf(), self.store())
    }

    /// A second connection on the same encrypted database, for inspecting what the scanner wrote.
    fn conn(&self) -> Connection {
        let conn = Connection::open(self.db()).expect("open raw encrypted database");
        conn.key(None::<&str>, self.key.as_bytes(), false).expect("apply key");
        conn
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&path, bytes).expect("write test file");
    }
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

/// `(files.version, files.content_hash, chunk count)` for the single stored file.
fn snapshot(conn: &Connection) -> (i64, Vec<u8>, i64) {
    let (version, hash) = conn
        .query_row("SELECT version, content_hash FROM files", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("read file row");
    (version, hash, count(conn, "SELECT count(*) FROM chunks"))
}

#[test]
fn empty_directory_reports_nothing() {
    let fixture = Fixture::new();
    let outcome = fixture.scanner().reconcile().expect("reconcile");

    assert_eq!(outcome, ScanOutcome::default());
    assert_eq!(count(&fixture.conn(), "SELECT count(*) FROM files"), 0);
}

#[test]
fn added_file_is_indexed() {
    let fixture = Fixture::new();
    fixture.write("notes.md", b"hello braga index");

    let outcome = fixture.scanner().reconcile().expect("reconcile");

    assert_eq!(outcome, ScanOutcome { added: 1, ..ScanOutcome::default() });
    let conn = fixture.conn();
    assert_eq!(count(&conn, "SELECT count(*) FROM files"), 1);
    assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 1);
    assert_eq!(fts_hits(&conn, "hello"), 1);
    let stored: String =
        conn.query_row("SELECT path FROM files", [], |row| row.get(0)).expect("read path");
    let expected = fixture.root().join("notes.md");
    assert_eq!(stored, expected.to_str().expect("the temporary path is UTF-8"));
}

#[test]
fn no_change_rescan_is_idempotent() {
    let fixture = Fixture::new();
    fixture.write("a.md", b"stable content");

    let scanner = fixture.scanner();
    assert_eq!(scanner.reconcile().expect("first reconcile").added, 1);
    let before = snapshot(&fixture.conn());

    let outcome = scanner.reconcile().expect("second reconcile");

    assert_eq!(outcome.added, 0);
    assert_eq!(outcome.modified, 0);
    assert_eq!(outcome.deleted, 0);
    assert_eq!(outcome.skipped, 0);
    assert_eq!(outcome.unchanged, 1);
    assert_eq!(snapshot(&fixture.conn()), before, "an unchanged rescan writes nothing");
}

#[test]
fn modified_file_replaces_chunks() {
    let fixture = Fixture::new();
    fixture.write("a.md", b"alpha oldword");
    assert_eq!(fixture.scanner().reconcile().expect("first reconcile").added, 1);

    fixture.write("a.md", b"beta newword");
    let outcome = fixture.scanner().reconcile().expect("second reconcile");

    assert_eq!(outcome.added, 0);
    assert_eq!(outcome.modified, 1);
    assert_eq!(outcome.unchanged, 0);
    let (version, _, chunk_count) = snapshot(&fixture.conn());
    assert_eq!(version, 2, "a detected change bumps the file version");
    assert_eq!(chunk_count, 1);
    let conn = fixture.conn();
    assert_eq!(fts_hits(&conn, "oldword"), 0, "the old text is unindexed");
    assert_eq!(fts_hits(&conn, "newword"), 1);
}

#[test]
fn deleted_file_is_garbage_collected() {
    let fixture = Fixture::new();
    fixture.write("a.md", b"alpha oldword");
    assert_eq!(fixture.scanner().reconcile().expect("first reconcile").added, 1);

    fs::remove_file(fixture.root().join("a.md")).expect("remove the file");
    let outcome = fixture.scanner().reconcile().expect("second reconcile");

    assert_eq!(outcome.deleted, 1);
    assert_eq!(outcome.added, 0);
    assert_eq!(outcome.modified, 0);
    let conn = fixture.conn();
    assert_eq!(count(&conn, "SELECT count(*) FROM files"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 0);
    assert_eq!(fts_hits(&conn, "oldword"), 0);
}

#[test]
fn binary_file_is_skipped() {
    let fixture = Fixture::new();
    fixture.write("text.md", b"plain text");
    fixture.write("blob.bin", &[0xFF, 0xFE, 0x00, 0x80]);

    let outcome = fixture.scanner().reconcile().expect("reconcile");

    assert_eq!(outcome.added, 1);
    assert_eq!(outcome.skipped, 1);
    let conn = fixture.conn();
    assert_eq!(count(&conn, "SELECT count(*) FROM files"), 1, "the binary file has no row");
}

#[test]
fn text_file_that_becomes_binary_is_garbage_collected() {
    let fixture = Fixture::new();
    fixture.write("a.md", b"plain text");
    assert_eq!(fixture.scanner().reconcile().expect("first reconcile").added, 1);

    fixture.write("a.md", &[0xFF, 0xFE, 0x00]);
    let outcome = fixture.scanner().reconcile().expect("second reconcile");

    assert_eq!(outcome.deleted, 1, "an indexed file that turns binary is GC'd like a deletion");
    assert_eq!(outcome.skipped, 1);
    assert_eq!(outcome.modified, 0);
    let conn = fixture.conn();
    assert_eq!(count(&conn, "SELECT count(*) FROM files"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 0);
}

#[test]
fn nested_directories_are_walked() {
    let fixture = Fixture::new();
    fixture.write("top.md", b"top level");
    fixture.write("sub/mid.md", b"middle level");
    fixture.write("sub/deeper/leaf.md", b"leaf level");

    let outcome = fixture.scanner().reconcile().expect("reconcile");

    assert_eq!(outcome.added, 3);
    assert_eq!(count(&fixture.conn(), "SELECT count(*) FROM files"), 3);
}

#[cfg(unix)]
#[test]
fn symlinks_are_skipped() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    fixture.write("real/a.md", b"alpha");
    fixture.write("real/b.md", b"beta");
    symlink(fixture.root().join("real/a.md"), fixture.root().join("link.md"))
        .expect("symlink a file");
    symlink(fixture.root().join("real"), fixture.root().join("linkdir")).expect("symlink a dir");

    let outcome = fixture.scanner().reconcile().expect("reconcile");

    assert_eq!(outcome.added, 2, "only the real directory's files are indexed");
    assert_eq!(outcome.skipped, 2, "the symlinked file and directory are skipped");
}

/// Linux-only: macOS's VFS rejects a non-`UTF-8` file name outright (`EILSEQ`), so the fixture
/// cannot exist there. CI runs this on Ubuntu.
#[cfg(target_os = "linux")]
#[test]
fn non_utf8_filename_is_skipped() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let fixture = Fixture::new();
    fixture.write("ok.md", b"plain text");
    let name = OsStr::from_bytes(b"bad-\xff-name.md");
    fs::write(fixture.root().join(name), b"plain text").expect("write a non-UTF-8 name");

    let outcome = fixture.scanner().reconcile().expect("reconcile");

    assert_eq!(outcome.added, 1);
    assert_eq!(outcome.skipped, 1);
    assert_eq!(count(&fixture.conn(), "SELECT count(*) FROM files"), 1);
}

#[cfg(unix)]
#[test]
fn unreadable_file_returns_an_error() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = Fixture::new();
    fixture.write("locked.md", b"secret");
    let path = fixture.root().join("locked.md");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("make it unreadable");

    let error = fixture.scanner().reconcile().expect_err("an unreadable file fails closed");

    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("restore permissions");
    assert!(matches!(error, ScanError::Io(_)), "expected ScanError::Io, got: {error}");
}

#[test]
fn missing_scan_root_is_an_error() {
    let fixture = Fixture::new();
    let scanner = Scanner::new(fixture.root().join("absent"), fixture.store());

    let error = scanner.reconcile().expect_err("a missing root fails closed");

    assert!(matches!(error, ScanError::Io(_)), "expected ScanError::Io, got: {error}");
}
