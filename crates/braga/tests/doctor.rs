// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the `doctor` claims verifier over a real encrypted store.
//!
//! The fixed-width embedder is a test double; storage (`SQLCipher` + `sqlite-vec` + FTS5) and the
//! optional live HTTP path are the real implementations. The live check reads a process-global
//! variable, so every run is serialized behind [`LIVE_LOCK`].

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::TcpListener,
    sync::{Mutex, MutexGuard, PoisonError},
    thread,
};

use braga::{
    config::{Config, Embedding},
    doctor::{Check, DoctorReport, doctor},
};
use braga_chunker::content_hash;
use braga_embedder::{EmbedError, Embedder};
use braga_storage::{Key, NewChunk, Store};
use tempfile::TempDir;

/// Serializes every doctor run, because the live check reads the process environment.
static LIVE_LOCK: Mutex<()> = Mutex::new(());

/// The environment variable that opts into the live-provider check.
const LIVE_ENV: &str = "BRAGA_DOCTOR_LIVE_EMBED";

/// The fake embedder's model name.
const MODEL: &str = "doctor-fake";

/// Takes the process-wide doctor lock.
fn lock() -> MutexGuard<'static, ()> {
    LIVE_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clears the live-check gate; callers must hold [`LIVE_LOCK`].
fn clear_live() {
    // SAFETY: every test that touches the variable holds `LIVE_LOCK`.
    unsafe { std::env::remove_var(LIVE_ENV) };
}

/// Enables the live check for its lifetime; callers must hold [`LIVE_LOCK`].
struct LiveEnv;

impl LiveEnv {
    /// Sets the gate and returns a guard that clears it on drop.
    fn enable() -> Self {
        // SAFETY: every test that touches the variable holds `LIVE_LOCK`.
        unsafe { std::env::set_var(LIVE_ENV, "1") };
        Self
    }
}

impl Drop for LiveEnv {
    fn drop(&mut self) {
        // SAFETY: every test that touches the variable holds `LIVE_LOCK`.
        unsafe { std::env::remove_var(LIVE_ENV) };
    }
}

/// A fixed-width embedder whose probe and query texts map to the same vector.
struct FakeEmbedder {
    model: String,
    width: usize,
}

impl FakeEmbedder {
    /// Builds the four-wide test double.
    fn new() -> Self {
        Self { model: MODEL.to_owned(), width: 4 }
    }

    /// Maps the probe and its near-identical query to one vector, so top-1 is exact.
    fn vector(text: &str) -> Vec<f32> {
        if text.contains("quick brown fox") {
            vec![1.0, 0.0, 0.0, 0.0]
        } else {
            vec![0.0, 1.0, 0.0, 0.0]
        }
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(inputs.iter().map(|text| Self::vector(text)).collect())
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> Option<usize> {
        Some(self.width)
    }
}

/// A config whose store lives in a fresh temporary directory.
fn temp_config() -> (TempDir, Config) {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let mut config = Config::default();
    config.storage.path = dir.path().join("braga.sqlite3");
    (dir, config)
}

/// A fixed key for tests.
fn key(byte: u8) -> Key {
    Key::from_bytes([byte; 32])
}

/// Looks up one named check.
fn find<'a>(report: &'a DoctorReport, name: &str) -> &'a Check {
    report
        .checks
        .iter()
        .find(|check| check.name == name)
        .unwrap_or_else(|| panic!("missing check {name}"))
}

/// Serves one canned JSON response on a loopback socket and returns the endpoint URL.
fn serve_json(body: &'static str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let address = listener.local_addr().expect("stub address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read header");
            if line == "\r\n" || line.is_empty() {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                content_length = value.trim().parse().expect("content length");
            }
        }
        let mut request_body = vec![0_u8; content_length];
        reader.read_exact(&mut request_body).expect("read request body");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("write response");
    });
    (format!("http://{address}/embed"), handle)
}

#[test]
fn all_six_checks_pass_offline_and_serialize() {
    let _guard = lock();
    clear_live();
    let (_dir, config) = temp_config();
    let report = doctor(&config, &key(0x5A), &FakeEmbedder::new()).expect("doctor runs");

    let names: Vec<&str> = report.checks.iter().map(|check| check.name).collect();
    assert_eq!(
        names,
        [
            "config.valid",
            "store.open",
            "embed.roundtrip",
            "crypto.roundtrip",
            "encryption.at_rest",
            "embed.live",
        ]
    );
    let failed: Vec<(&str, &str)> = report
        .checks
        .iter()
        .filter(|check| !check.passed)
        .map(|check| (check.name, &*check.detail))
        .collect();
    assert!(report.ok, "every check must pass; failures: {failed:?}");

    let live = find(&report, "embed.live");
    assert!(live.detail.starts_with("skipped"), "{}", live.detail);

    let crypto = find(&report, "crypto.roundtrip");
    assert!(crypto.detail.contains("a different key was rejected"), "{}", crypto.detail);

    let json = report.to_json().expect("the report must serialize");
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("`to_json` must produce valid JSON");
    assert_eq!(parsed["ok"], serde_json::json!(true));
    assert_eq!(parsed["checks"].as_array().expect("checks array").len(), 6);
    for name in names {
        assert!(json.contains(&format!("\"{name}\"")), "JSON is missing {name}:\n{json}");
    }
}

#[test]
fn config_check_rejects_a_zero_scan_interval() {
    let _guard = lock();
    clear_live();
    let (_dir, mut config) = temp_config();
    config.scan.auto_scan = true;
    config.scan.interval_secs = 0;
    let report = doctor(&config, &key(0x5A), &FakeEmbedder::new()).expect("doctor runs");

    let check = find(&report, "config.valid");
    assert!(!check.passed);
    assert!(check.detail.contains("interval_secs"), "{}", check.detail);
    assert!(!report.ok);
}

#[test]
fn store_open_fails_when_the_key_does_not_decrypt_the_index() {
    let _guard = lock();
    clear_live();
    let (_dir, config) = temp_config();
    Store::open(&config.storage.path, &key(0x11))
        .expect("create the configured store")
        .close()
        .expect("close");

    let report = doctor(&config, &key(0x22), &FakeEmbedder::new()).expect("doctor runs");

    let check = find(&report, "store.open");
    assert!(!check.passed);
    assert!(check.detail.contains("wrong database key"), "{}", check.detail);
    assert!(!report.ok);
}

#[test]
fn live_check_is_skipped_without_the_environment_gate() {
    let _guard = lock();
    clear_live();
    let (_dir, config) = temp_config();
    let report = doctor(&config, &key(0x5A), &FakeEmbedder::new()).expect("doctor runs");

    let check = find(&report, "embed.live");
    assert!(check.passed);
    assert_eq!(
        check.detail,
        "skipped (set BRAGA_DOCTOR_LIVE_EMBED=1 to verify the configured provider)"
    );
}

#[test]
fn live_check_passes_through_a_loopback_provider() {
    let _guard = lock();
    clear_live();
    let (_dir, mut config) = temp_config();
    let (url, server) = serve_json(r#"{"data":[{"index":0,"embedding":[1.0,2.0,3.0]}]}"#);
    config.embedding =
        Embedding { provider: "openai".to_owned(), url, model: "model-a".to_owned() };
    let _live = LiveEnv::enable();

    let report = doctor(&config, &key(0x5A), &FakeEmbedder::new()).expect("doctor runs");

    let check = find(&report, "embed.live");
    assert!(check.passed, "{}", check.detail);
    assert!(!check.detail.starts_with("skipped"), "{}", check.detail);
    server.join().expect("stub server exits");
}

#[test]
fn live_check_reports_a_refused_endpoint() {
    let _guard = lock();
    clear_live();
    let (_dir, mut config) = temp_config();
    config.embedding = Embedding {
        provider: "ollama".to_owned(),
        url: "http://127.0.0.1:1/embed".to_owned(),
        model: "model-a".to_owned(),
    };
    let _live = LiveEnv::enable();

    let report = doctor(&config, &key(0x5A), &FakeEmbedder::new()).expect("doctor runs");

    let check = find(&report, "embed.live");
    assert!(!check.passed);
    assert!(check.detail.contains("embedding transport error"), "{}", check.detail);
    assert!(!report.ok);
}

#[test]
fn doctor_leaves_a_preexisting_store_untouched() {
    let _guard = lock();
    clear_live();
    let (_dir, config) = temp_config();
    let key = key(0x5A);
    let text = "the configured file that doctor must not touch";
    let id = content_hash(text.as_bytes());
    {
        let store = Store::open(&config.storage.path, &key).expect("create the configured store");
        let chunk = NewChunk {
            id: id.as_bytes(),
            byte_offset: 0,
            line_start: 1,
            line_end: 1,
            content: text,
        };
        store.sync_file("kept.md", id.as_bytes(), &[chunk]).expect("seed the configured store");
        store.close().expect("close");
    }

    let report = doctor(&config, &key, &FakeEmbedder::new()).expect("doctor runs");
    assert!(report.ok);

    let store = Store::open(&config.storage.path, &key).expect("reopen the configured store");
    let files = store.list_files().expect("list the configured files");
    assert_eq!(files.len(), 1, "doctor must not add file rows");
    assert_eq!(files[0].path, "kept.md");
    assert_eq!(files[0].content_hash.as_slice(), id.as_bytes());
    let rowids = store.fts_search("configured", 5).expect("search the kept chunk");
    assert_eq!(rowids.len(), 1, "doctor must not add chunk rows");
    let chunks = store.chunk_metadata(&rowids).expect("read the kept chunk");
    assert_eq!(chunks[0].content, text);
    assert_eq!(chunks[0].path, "kept.md");
}
