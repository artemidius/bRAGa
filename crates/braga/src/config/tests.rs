// SPDX-License-Identifier: Apache-2.0

//! The configuration test suite.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::*;
use super::{resolve::resolve_from_env, schema::default_storage_path};

/// A complete, valid configuration document.
const FULL: &str = r#"
    [scan]
    dir = "/tmp/docs"

    [embedding]
    provider = "ollama"
    url = "http://localhost:11434"
    model = "nomic-embed-text"

    [storage]
    path = "/tmp/braga.sqlite3"
"#;

/// Resolves through the pure pipeline with a synthetic, borrow-friendly environment.
fn resolve_env(env: &[(&str, &str)], explicit: Option<&Path>) -> Result<Config, ConfigError> {
    resolve_from_env(
        explicit,
        env.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    )
}

/// Writes `contents` to a unique file under the system temp directory.
fn write_temp_config(name: &str, contents: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock").as_nanos();
    let path = std::env::temp_dir()
        .join(format!("braga-config-{name}-{}-{nanos}.toml", std::process::id()));
    std::fs::write(&path, contents).expect("temp config should be writable");
    path
}

#[test]
fn defaults_are_the_documented_values() {
    let config = Config::default();
    assert_eq!(config.scan, Scan { dir: PathBuf::from("."), auto_scan: true, interval_secs: 300 });
    assert!(config.scan.auto_scan);
    assert_eq!(config.scan.interval_secs, 300);
    assert_eq!(config.embedding, Embedding::default());
    assert_eq!(config.embedding.provider, "");
    assert_eq!(config.embedding.url, "");
    assert_eq!(config.embedding.model, "");
    assert_eq!(config.storage.path, default_storage_path());

    let storage = default_storage_path();
    assert!(
        storage.ends_with(".braga/index.db") || storage == Path::new("braga.sqlite3"),
        "unexpected default storage path: {storage:?}"
    );
}

/// One row of the environment table: the injected environment and the mutation it expects.
struct EnvCase {
    /// Row name used in assertion messages.
    name: &'static str,
    /// Synthetic environment for the row.
    env: &'static [(&'static str, &'static str)],
    /// Expected delta over [`Config::default`].
    mutate: fn(&mut Config),
}

#[test]
fn defaults_and_leaf_overrides() {
    let cases = [
        EnvCase { name: "built-in defaults", env: &[], mutate: |_| {} },
        EnvCase {
            name: "non-BRAGA variables are ignored",
            env: &[("PATH", "/usr/bin"), ("RUST_LOG", "debug")],
            mutate: |_| {},
        },
        EnvCase {
            name: "scan.dir",
            env: &[("BRAGA_SCAN__DIR", "/srv/docs")],
            mutate: |config| config.scan.dir = PathBuf::from("/srv/docs"),
        },
        EnvCase {
            name: "scan.auto_scan",
            env: &[("BRAGA_SCAN__AUTO_SCAN", "false")],
            mutate: |config| config.scan.auto_scan = false,
        },
        EnvCase {
            name: "scan.interval_secs",
            env: &[("BRAGA_SCAN__INTERVAL_SECS", "60")],
            mutate: |config| config.scan.interval_secs = 60,
        },
        EnvCase {
            name: "embedding.provider",
            env: &[("BRAGA_EMBEDDING__PROVIDER", "ollama")],
            mutate: |config| config.embedding.provider = "ollama".to_owned(),
        },
        EnvCase {
            name: "embedding.url",
            env: &[("BRAGA_EMBEDDING__URL", "http://localhost:11434")],
            mutate: |config| config.embedding.url = "http://localhost:11434".to_owned(),
        },
        EnvCase {
            name: "embedding.model",
            env: &[("BRAGA_EMBEDDING__MODEL", "nomic-embed-text")],
            mutate: |config| config.embedding.model = "nomic-embed-text".to_owned(),
        },
        EnvCase {
            name: "storage.path",
            env: &[("BRAGA_STORAGE__PATH", "/srv/braga.db")],
            mutate: |config| config.storage.path = PathBuf::from("/srv/braga.db"),
        },
    ];

    let defaults = Config::default();
    for EnvCase { name, env, mutate } in cases {
        let mut expected = defaults.clone();
        mutate(&mut expected);
        let resolved = resolve_env(env, None).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(resolved, expected, "{name}");
    }
}

#[test]
fn full_precedence_cross_product() {
    // Each field pins one precedence edge; no field is named by every layer.
    let file = write_temp_config(
        "precedence-file",
        r#"
            [scan]
            dir = "/file-dir"
            auto_scan = true
            interval_secs = 60

            [embedding]
            provider = "file-provider"
            model = "file-model"
        "#,
    );
    let explicit = write_temp_config(
        "precedence-explicit",
        "[scan]\ninterval_secs = 240\n\n[embedding]\nmodel = \"explicit-model\"\n",
    );
    let inline = "[scan]\ninterval_secs = 120\n\n[embedding]\nprovider = \"inline-provider\"\n";
    let env = vec![
        ("BRAGA_SCAN__DIR".to_owned(), "/leaf-dir".to_owned()),
        ("BRAGA_SCAN__AUTO_SCAN".to_owned(), "false".to_owned()),
        ("BRAGA_SCAN__INTERVAL_SECS".to_owned(), "30".to_owned()),
        ("BRAGA_EMBEDDING__PROVIDER".to_owned(), "leaf-provider".to_owned()),
        ("BRAGA_EMBEDDING__URL".to_owned(), "leaf-url".to_owned()),
        ("BRAGA_TOML".to_owned(), file.display().to_string()),
        ("BRAGA_CONFIG".to_owned(), inline.to_owned()),
    ];

    let resolved = resolve_from_env(Some(&explicit), env.into_iter());
    for path in [&file, &explicit] {
        std::fs::remove_file(path).expect("temp config should be removable");
    }

    let mut expected = Config::default();
    expected.scan.dir = PathBuf::from("/file-dir"); // BRAGA_TOML > leaf > default
    expected.scan.auto_scan = true; // BRAGA_TOML > leaf
    expected.scan.interval_secs = 240; // explicit > BRAGA_CONFIG > file > leaf
    expected.embedding.provider = "inline-provider".to_owned(); // BRAGA_CONFIG > file > leaf
    expected.embedding.url = "leaf-url".to_owned(); // leaf wins; higher layers are silent
    expected.embedding.model = "explicit-model".to_owned(); // explicit > file > default
    assert_eq!(resolved.expect("layers should resolve"), expected);
}

#[test]
fn malformed_leaf_values_fail_closed() {
    let cases = [
        ("BRAGA_SCAN__AUTO_SCAN", "yes"),
        ("BRAGA_SCAN__AUTO_SCAN", "1"),
        ("BRAGA_SCAN__AUTO_SCAN", ""),
        ("BRAGA_SCAN__INTERVAL_SECS", "-1"),
        ("BRAGA_SCAN__INTERVAL_SECS", "abc"),
        ("BRAGA_SCAN__INTERVAL_SECS", "18446744073709551616"),
    ];

    for (key, value) in cases {
        let result = resolve_env(&[(key, value)], None);
        let Err(error) = result else {
            panic!("{key}={value} must fail closed");
        };
        assert!(
            matches!(&error, ConfigError::InvalidEnvValue { key: name, .. } if name == key),
            "{key}={value}: unexpected error: {error}"
        );
    }
}

#[test]
fn partial_scan_deserialize_uses_field_defaults() {
    let scan: Scan = toml::from_str("dir = \"/x\"").expect("partial scan should parse");

    assert!(scan.auto_scan, "a named dir must not flip auto_scan to false");
    assert_eq!(scan.interval_secs, 300, "a named dir must not zero interval_secs");
}

#[test]
fn invalid_scan_interval_fails_closed_at_start() {
    use braga_storage::{Key, Store};

    use crate::status::ScanEngine;

    let root = tempfile::tempdir().expect("create scan root");
    let db_dir = tempfile::tempdir().expect("create database directory");
    let key = Key::from_bytes([0x6B; 32]);
    let db = db_dir.path().join("braga.sqlite3");

    let mut config = Config::default();
    config.scan.dir = root.path().to_path_buf();
    config.scan.auto_scan = true;
    config.scan.interval_secs = 0;

    let store = Store::open(&db, &key).expect("open store");
    let error =
        ScanEngine::start(config.clone(), store).expect_err("a zero interval must fail closed");
    assert!(matches!(error, ConfigError::InvalidScanInterval));

    config.scan.auto_scan = false;
    let store = Store::open(&db, &key).expect("reopen store");
    let engine = ScanEngine::start(config, store).expect("manual-only scans ignore the interval");
    engine.stop();
}

#[test]
fn explicit_path_beats_inline_config() {
    let explicit = write_temp_config("explicit-wins", "[embedding]\nmodel = \"explicit-model\"\n");
    let env =
        vec![("BRAGA_CONFIG".to_owned(), "[embedding]\nmodel = \"inline-model\"\n".to_owned())];

    let resolved = resolve_from_env(Some(&explicit), env.into_iter());
    std::fs::remove_file(&explicit).expect("temp config should be removable");

    let mut expected = Config::default();
    expected.embedding.model = "explicit-model".to_owned();
    assert_eq!(resolved.expect("layers should resolve"), expected);
}

#[test]
fn toml_layers_merge_per_field() {
    let file = write_temp_config(
        "merge",
        "[scan]\ndir = \"/from-file\"\n\n[embedding]\nprovider = \"file-provider\"\n",
    );
    let inline = "[embedding]\nurl = \"http://inline:11434\"\n";
    let env = vec![
        ("BRAGA_SCAN__DIR".to_owned(), "/from-leaf".to_owned()),
        ("BRAGA_EMBEDDING__MODEL".to_owned(), "leaf-model".to_owned()),
        ("BRAGA_TOML".to_owned(), file.display().to_string()),
        ("BRAGA_CONFIG".to_owned(), inline.to_owned()),
    ];

    let resolved = resolve_from_env(None, env.into_iter());
    std::fs::remove_file(&file).expect("temp config should be removable");

    let mut expected = Config::default();
    expected.scan.dir = PathBuf::from("/from-file"); // the file beats the leaf
    expected.embedding.provider = "file-provider".to_owned();
    expected.embedding.url = "http://inline:11434".to_owned(); // inline beats the file
    expected.embedding.model = "leaf-model".to_owned(); // falls through the higher layers
    assert_eq!(resolved.expect("layers should resolve"), expected);
}

#[test]
fn parses_full_config() {
    let config = Config::from_toml_str(FULL).expect("full config should parse");
    assert_eq!(config.scan.dir, PathBuf::from("/tmp/docs"));
    assert_eq!(config.embedding.provider, "ollama");
    assert_eq!(config.embedding.url, "http://localhost:11434");
    assert_eq!(config.embedding.model, "nomic-embed-text");
    assert_eq!(config.storage.path, PathBuf::from("/tmp/braga.sqlite3"));
}

#[test]
fn missing_fields_fall_back_to_defaults() {
    let config =
        Config::from_toml_str("[scan]\ndir = \"/tmp/docs\"").expect("partial config parses");
    assert_eq!(config.scan.dir, PathBuf::from("/tmp/docs"));
    assert_eq!(config.embedding, Embedding::default());
    assert_eq!(config.storage, Storage::default());
}

#[test]
fn unknown_keys_fail_closed_in_every_layer() {
    let cases = [
        ("top level", "bogus = true"),
        ("scan", "[scan]\nbogus = true"),
        ("embedding", "[embedding]\nbogus = true"),
        ("storage", "[storage]\nbogus = true"),
    ];

    for (level, contents) in cases {
        let parsed = Config::from_toml_str(contents);
        assert!(
            matches!(parsed, Err(ConfigError::Toml(_))),
            "from_toml_str must reject an unknown key in {level}"
        );

        let inline = resolve_env(&[("BRAGA_CONFIG", contents)], None);
        assert!(
            matches!(inline, Err(ConfigError::Toml(_))),
            "BRAGA_CONFIG must reject an unknown key in {level}"
        );

        let path = write_temp_config(level, contents);
        let file_value = path.display().to_string();
        let from_file = resolve_env(&[("BRAGA_TOML", &file_value)], None);
        std::fs::remove_file(&path).expect("temp config should be removable");
        assert!(
            matches!(from_file, Err(ConfigError::Toml(_))),
            "BRAGA_TOML must reject an unknown key in {level}"
        );
    }
}

#[test]
fn unrecognized_environment_variables_fail_closed() {
    type Matcher = fn(&ConfigError) -> bool;
    let cases: [(&str, Matcher); 6] = [
        ("BRAGA_BOGUS", |error| matches!(error, ConfigError::UnknownEnvVar(_))),
        ("BRAGA_", |error| matches!(error, ConfigError::UnknownEnvVar(_))),
        ("BRAGA_EMBEDDING", |error| matches!(error, ConfigError::InvalidEnvVar(_))),
        ("BRAGA_A__B__C", |error| matches!(error, ConfigError::InvalidEnvVar(_))),
        ("BRAGA_EMBEDDING__NOPE", |error| matches!(error, ConfigError::InvalidEnvVar(_))),
        ("BRAGA_scan__dir", |error| matches!(error, ConfigError::InvalidEnvVar(_))),
    ];

    for (key, is_expected) in cases {
        let result = resolve_env(&[(key, "value")], None);
        let Err(error) = result else {
            panic!("{key} must fail closed");
        };
        assert!(is_expected(&error), "{key}: unexpected error: {error}");
    }
}

#[test]
fn default_toml_round_trips() {
    let rendered = Config::default_toml().expect("defaults should serialize");
    for table in ["[scan]", "[embedding]", "[storage]"] {
        assert!(rendered.contains(table), "default TOML is missing {table}:\n{rendered}");
    }
    assert!(rendered.contains("dir = \".\""), "scan.dir should serialize as '.':\n{rendered}");
    assert!(
        rendered.contains("auto_scan = true"),
        "scan.auto_scan should serialize as true:\n{rendered}"
    );
    assert!(
        rendered.contains("interval_secs = 300"),
        "scan.interval_secs should serialize as 300:\n{rendered}"
    );
    assert_eq!(
        Config::from_toml_str(&rendered).expect("default TOML should parse"),
        Config::default()
    );
}

#[test]
fn load_round_trips_a_file() {
    let path = write_temp_config("load", FULL);
    let loaded = Config::load(&path);
    std::fs::remove_file(&path).expect("temp config should be removable");

    let expected = Config::from_toml_str(FULL).expect("full config should parse");
    assert_eq!(loaded.expect("config should load"), expected);
}
