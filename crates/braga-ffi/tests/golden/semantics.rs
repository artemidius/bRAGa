// SPDX-License-Identifier: Apache-2.0

//! Open/close, error-code, init, and offline `open → update → query → doctor` tests.

use std::{fs, path::Path};

use serde_json::Value;

use crate::{
    ffi::{self, BragaResult, BragaStatusCode},
    harness::{self, Fixture},
};

/// Renders a path as the UTF-8 string the ABI expects.
fn path_string(path: &Path) -> String {
    path.to_str().expect("fixture path is UTF-8").to_owned()
}

#[test]
fn open_and_close_round_trip() {
    let fixture = Fixture::new();
    let (code, handle) = ffi::open(Some(fixture.config()), Some(fixture.key()));

    assert_eq!(code, BragaResult::Ok);
    assert!(!handle.is_null());
    assert_eq!(ffi::status(handle), BragaStatusCode::UpToDate);
    assert_eq!(ffi::close(handle), BragaResult::Ok);
    assert!(ffi::last_error().is_empty(), "{}", ffi::last_error());
}

#[test]
fn open_eagerly_rejects_a_wrong_key() {
    let fixture = Fixture::new();
    let (code, handle) = ffi::open(Some(fixture.config()), Some(fixture.key()));
    assert_eq!(code, BragaResult::Ok);
    assert_eq!(ffi::close(handle), BragaResult::Ok);

    let wrong = fixture.write("wrong-key", &[0x11; 32]);
    let (code, handle) = ffi::open(Some(fixture.config()), Some(wrong.as_str()));

    assert_eq!(code, BragaResult::Store);
    assert!(handle.is_null(), "an error must not return a handle");
    assert!(ffi::last_error().contains("wrong database key"), "{}", ffi::last_error());
}

#[test]
fn missing_config_and_malformed_keys_report_sanitized_errors() {
    let fixture = Fixture::new();
    let missing = path_string(&fixture.root().join("missing.toml"));
    let (code, handle) = ffi::open(Some(missing.as_str()), Some(fixture.key()));
    assert_eq!(code, BragaResult::Config);
    assert!(handle.is_null());
    assert!(ffi::last_error().contains("configuration"), "{}", ffi::last_error());

    // The malformed key's bytes must never appear in the message.
    let bad_key = fixture.write("bad-key", b"not-a-key");
    let (code, handle) = ffi::open(Some(fixture.config()), Some(bad_key.as_str()));
    assert_eq!(code, BragaResult::Key);
    assert!(handle.is_null());
    let message = ffi::last_error();
    assert!(message.contains("key file"), "{message}");
    assert!(message.contains("expected exactly 32 bytes"), "{message}");
    assert!(!message.contains("not-a-key"), "key content leaked: {message}");
}

#[test]
fn toml_errors_do_not_echo_file_content() {
    let fixture = Fixture::new();
    let broken = fixture.write("broken.toml", b"[scan]\ndir = \"SUPER-SECRET\" nope\n");

    let (code, handle) = ffi::open(Some(broken.as_str()), Some(fixture.key()));

    assert_eq!(code, BragaResult::Config);
    assert!(handle.is_null());
    assert_eq!(ffi::last_error(), "invalid configuration");
}

#[test]
fn init_writes_a_config_and_key_once() {
    let fixture = Fixture::new();
    let config = path_string(&fixture.root().join("init.toml"));
    let key = path_string(&fixture.root().join("nested").join("init-key"));

    // SAFETY: both paths borrow live Rust strings.
    let code = unsafe { ffi::braga_init(config.as_ptr(), config.len(), key.as_ptr(), key.len()) };
    assert_eq!(code, BragaResult::Ok);
    assert!(Path::new(&config).is_file(), "config was not written");
    assert_eq!(fs::read(&key).expect("read the generated key").len(), 32);

    // Both writes are create-new, so a second run must refuse to clobber them.
    // SAFETY: as above.
    let code = unsafe { ffi::braga_init(config.as_ptr(), config.len(), key.as_ptr(), key.len()) };
    assert_eq!(code, BragaResult::Io);
    assert!(ffi::last_error().contains("already exists"), "{}", ffi::last_error());
}

#[test]
fn semantic_round_trip_stays_offline() {
    let fixture = Fixture::new();
    let (code, handle) = ffi::open(Some(fixture.config()), Some(fixture.key()));
    assert_eq!(code, BragaResult::Ok);

    // An empty index is reported as NotFound, not as an empty successful query.
    let (code, json) = ffi::query(handle, harness::QUERY, 1);
    assert_eq!(code, BragaResult::NotFound);
    assert!(json.is_none(), "an error must not return JSON");

    assert_eq!(ffi::update(handle), BragaResult::Ok);
    assert_eq!(ffi::status(handle), BragaStatusCode::UpToDate);

    let (code, json) = ffi::query(handle, harness::QUERY, 1);
    assert_eq!(code, BragaResult::Ok);
    let hits: Value = serde_json::from_str(&json.expect("query returned JSON"))
        .expect("query output parses as JSON");
    let hits = hits.as_array().expect("query output is an array");
    assert_eq!(hits.len(), 1, "the single sample chunk must be the top hit");
    let hit = &hits[0];
    assert!(
        hit["content"].as_str().expect("content").contains("quick brown fox"),
        "{}",
        hit["content"]
    );
    assert!(hit["path"].as_str().expect("path").ends_with("notes.md"), "{}", hit["path"]);
    assert_eq!(hit["chunk_id"].as_str().expect("chunk_id").len(), 64);
    assert!(hit["line_start"].as_u64().is_some(), "line_start must be a number");
    assert!(hit["score"].as_f64().expect("score") > 0.0);

    let (code, json) = ffi::doctor(handle);
    assert_eq!(code, BragaResult::Ok);
    let report: Value = serde_json::from_str(&json.expect("doctor returned JSON"))
        .expect("doctor output parses as JSON");
    assert_eq!(report["ok"], Value::Bool(true), "doctor must pass: {report}");
    assert_eq!(report["checks"].as_array().expect("checks").len(), 6);

    assert_eq!(ffi::close(handle), BragaResult::Ok);
}
