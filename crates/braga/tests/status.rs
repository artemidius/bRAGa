// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the background [`ScanEngine`] over real temporary files and a real
//! encrypted [`Store`].

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use braga::{
    config::Config,
    scanner::ScanError,
    status::{DataStatus, ScanEngine},
};
use braga_storage::{Key, Store};
use rusqlite::Connection;
use tempfile::TempDir;

/// Upper bound on waiting for an expected worker state; generous, because it is only a hang guard.
const WAIT: Duration = Duration::from_secs(10);

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

    /// A connection on the same encrypted database, for inspecting what the worker wrote.
    fn store(&self) -> Store {
        Store::open(&self.db(), &self.key).expect("open store")
    }

    fn config(&self, auto_scan: bool, interval_secs: u64) -> Config {
        let mut config = Config::default();
        config.scan.dir = self.root().to_path_buf();
        config.scan.auto_scan = auto_scan;
        config.scan.interval_secs = interval_secs;
        config
    }

    fn engine(&self, auto_scan: bool, interval_secs: u64) -> ScanEngine {
        ScanEngine::start(self.config(auto_scan, interval_secs), self.store())
            .expect("start the engine")
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&path, bytes).expect("write test file");
    }

    fn path_of(&self, relative: &str) -> String {
        self.root().join(relative).to_str().expect("the temporary path is UTF-8").to_owned()
    }

    /// A raw connection on the same encrypted database, for schema-level assertions.
    fn conn(&self) -> Connection {
        let conn = Connection::open(self.db()).expect("open raw encrypted database");
        conn.key(None::<&str>, self.key.as_bytes(), false).expect("apply key");
        conn
    }
}

/// Polls `condition` until it holds or `timeout` elapses; returns whether it held.
fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Waits for the worker to report `desired`.
fn wait_for_status(engine: &ScanEngine, desired: DataStatus) -> bool {
    wait_until(WAIT, || engine.status() == desired)
}

/// Whether `path` is one of the store's indexed files.
fn is_indexed(store: &Store, path: &str) -> bool {
    store.list_files().expect("list files").iter().any(|file| file.path == path)
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).expect("count rows")
}

fn version(conn: &Connection) -> i64 {
    conn.query_row("SELECT version FROM files", [], |row| row.get(0)).expect("read version")
}

/// Number of `chunks_fts` entries the FTS5 index matches for `term`.
fn fts_hits(conn: &Connection, term: &str) -> i64 {
    conn.query_row("SELECT count(*) FROM chunks_fts WHERE chunks_fts MATCH ?1", [term], |row| {
        row.get(0)
    })
    .expect("count fts hits")
}

/// Pins the property the whole design leans on: `ScanEngine` is shareable across threads.
#[test]
fn scan_engine_is_send_and_sync() {
    fn assert_send<T: Send + Sync>() {}

    assert_send::<ScanEngine>();
}

#[test]
fn add_transitions_to_up_to_date_and_indexes() {
    let fixture = Fixture::new();
    let engine = fixture.engine(false, 0);
    assert!(wait_for_status(&engine, DataStatus::UpToDate), "on-start reconcile did not settle");

    fixture.write("notes.md", b"hello braga status");
    engine.update().expect("manual update");

    assert_eq!(engine.status(), DataStatus::UpToDate);
    let inspector = fixture.store();
    let files = inspector.list_files().expect("list files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, fixture.path_of("notes.md"));
    engine.stop();
}

#[test]
fn modify_and_delete_transitions() {
    let fixture = Fixture::new();
    let engine = fixture.engine(false, 0);
    assert!(wait_for_status(&engine, DataStatus::UpToDate), "on-start reconcile did not settle");

    fixture.write("a.md", b"alpha oldword");
    engine.update().expect("index the addition");
    assert_eq!(engine.status(), DataStatus::UpToDate);
    assert_eq!(version(&fixture.conn()), 1);

    fixture.write("a.md", b"beta newword");
    engine.update().expect("index the modification");
    assert_eq!(engine.status(), DataStatus::UpToDate);
    let conn = fixture.conn();
    assert_eq!(version(&conn), 2, "a detected change bumps the file version");
    assert_eq!(fts_hits(&conn, "oldword"), 0, "the old text is unindexed");
    assert_eq!(fts_hits(&conn, "newword"), 1);

    fs::remove_file(fixture.root().join("a.md")).expect("remove the file");
    engine.update().expect("index the deletion");
    assert_eq!(engine.status(), DataStatus::UpToDate);
    let conn = fixture.conn();
    assert_eq!(count(&conn, "SELECT count(*) FROM files"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 0);

    engine.stop();
}

#[test]
fn status_is_non_blocking_during_a_long_reconcile() {
    let fixture = Fixture::new();
    let engine = fixture.engine(false, 0);
    assert!(wait_for_status(&engine, DataStatus::UpToDate), "on-start reconcile did not settle");

    for index in 0..3000 {
        let body = format!("small document body {index} for the reconcile");
        fixture.write(&format!("docs/{index:05}.md"), body.as_bytes());
    }

    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        let updater = scope.spawn(|| {
            let result = engine.update();
            done.store(true, Ordering::Release);
            result
        });

        let mut saw_in_flight = false;
        let mut calls = 0_u32;
        while !done.load(Ordering::Acquire) {
            let started = Instant::now();
            let status = engine.status();
            let elapsed = started.elapsed();
            assert!(elapsed < Duration::from_millis(500), "status() blocked for {elapsed:?}");
            assert!(
                matches!(
                    status,
                    DataStatus::UpToDate
                        | DataStatus::Checking
                        | DataStatus::Updating
                        | DataStatus::Error
                ),
                "status() returned a value outside the four locked variants"
            );
            saw_in_flight |= matches!(status, DataStatus::Checking | DataStatus::Updating);
            calls += 1;
            thread::sleep(Duration::from_millis(1));
        }

        assert!(calls > 0, "the polling loop never observed the in-flight reconcile");
        assert!(saw_in_flight, "a long reconcile never exposed an in-flight status");
        updater.join().expect("update thread").expect("update");
    });

    assert_eq!(engine.status(), DataStatus::UpToDate);
    assert_eq!(fixture.store().list_files().expect("list files").len(), 3000);
    engine.stop();
}

#[cfg(unix)]
#[test]
fn update_returns_the_last_error_and_leaves_status_error() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = Fixture::new();
    fixture.write("a.md", b"secret");
    let engine = fixture.engine(false, 0);
    assert!(wait_for_status(&engine, DataStatus::UpToDate), "on-start reconcile did not settle");
    assert!(is_indexed(&fixture.store(), &fixture.path_of("a.md")));

    fs::set_permissions(fixture.root(), fs::Permissions::from_mode(0o000))
        .expect("make the scan root unreadable");
    let result = engine.update();
    let status = engine.status();
    fs::set_permissions(fixture.root(), fs::Permissions::from_mode(0o755))
        .expect("restore permissions");

    assert!(matches!(result, Err(ScanError::Io(_))), "expected ScanError::Io, got: {result:?}");
    assert_eq!(status, DataStatus::Error);
    assert_eq!(engine.status(), DataStatus::Error, "the failure persists until the next reconcile");
    engine.stop();
}

#[test]
fn stop_terminates_promptly() {
    let fixture = Fixture::new();
    let engine = fixture.engine(false, 0);
    assert!(wait_for_status(&engine, DataStatus::UpToDate), "on-start reconcile did not settle");
    let started = Instant::now();
    engine.stop();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(5), "stop() took {elapsed:?}");

    let engine = fixture.engine(false, 0);
    assert!(wait_for_status(&engine, DataStatus::UpToDate), "on-start reconcile did not settle");
    fixture.write("a.md", b"manual update");
    engine.update().expect("manual update");
    let started = Instant::now();
    engine.stop();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(5), "stop() after an update took {elapsed:?}");
}

#[test]
fn auto_scan_interval_indexes_without_update() {
    let fixture = Fixture::new();
    let engine = fixture.engine(true, 1);
    assert!(wait_for_status(&engine, DataStatus::UpToDate), "on-start reconcile did not settle");

    fixture.write("periodic.md", b"periodic scan content");
    let inspector = fixture.store();
    let path = fixture.path_of("periodic.md");
    let indexed = wait_until(Duration::from_secs(5), || is_indexed(&inspector, &path));

    assert!(indexed, "the periodic trigger did not index a new file");
    assert_eq!(engine.status(), DataStatus::UpToDate);
    engine.stop();
}

#[test]
fn auto_scan_false_indexes_on_start_and_manual_update_only() {
    let fixture = Fixture::new();
    fixture.write("existing.md", b"before the engine starts");
    let engine = fixture.engine(false, 0);
    assert!(wait_for_status(&engine, DataStatus::UpToDate), "on-start reconcile did not settle");

    let inspector = fixture.store();
    let existing = fixture.path_of("existing.md");
    assert!(is_indexed(&inspector, &existing), "the on-start reconcile missed a pre-existing file");

    fixture.write("added.md", b"after the engine starts");
    let added = fixture.path_of("added.md");
    thread::sleep(Duration::from_millis(300));
    assert!(!is_indexed(&inspector, &added), "a manual-only engine must not rescan on a timer");
    assert_eq!(engine.status(), DataStatus::UpToDate);

    engine.update().expect("manual update");
    assert!(is_indexed(&inspector, &added), "a manual update should index the new file");
    assert_eq!(engine.status(), DataStatus::UpToDate);
    engine.stop();
}
