// SPDX-License-Identifier: Apache-2.0

//! Subprocess smoke tests for the `braga` binary.

mod common;

use std::fs;

use braga::config::Config;

const KEY: [u8; 32] = [0x5C; 32];

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("create temporary directory")
}

#[test]
fn help_and_version_succeed() {
    let dir = temp_dir();

    let help = common::run_in(dir.path(), &["--help"]);
    assert!(help.status.success(), "{}", common::text(&help.stderr));
    let stdout = common::text(&help.stdout);
    assert!(stdout.contains("Usage"), "help should print usage:\n{stdout}");
    for name in ["init", "scan", "query", "doctor"] {
        assert!(stdout.contains(name), "help should list `{name}`:\n{stdout}");
    }

    let version = common::run_in(dir.path(), &["--version"]);
    assert!(version.status.success(), "{}", common::text(&version.stderr));
    assert!(
        common::text(&version.stdout).contains(env!("CARGO_PKG_VERSION")),
        "version should print the crate version"
    );
}

#[test]
fn init_writes_config_and_key_and_refuses_to_clobber() {
    let dir = temp_dir();
    let home = std::env::var("HOME").expect("the test environment sets HOME");
    let config = dir.path().join("braga.toml");
    let key = dir.path().join("key");
    let args = ["init", "--config", config.to_str().unwrap(), "--key-file", key.to_str().unwrap()];

    // The default `storage.path` embeds `$HOME`, so the child must see the test's HOME for the
    // written config to equal `Config::default_toml()` rendered here. The two write targets below
    // remain this test's temp directory, and the key file is explicit, so nothing touches $HOME.
    let first = common::run_in(std::path::Path::new(&home), &args);
    assert!(first.status.success(), "{}", common::text(&first.stderr));
    assert_eq!(
        fs::read_to_string(&config).expect("read written config"),
        Config::default_toml().expect("render the defaults")
    );
    let key_bytes = fs::read(&key).expect("read written key");
    assert_eq!(key_bytes.len(), 32, "the key file is exactly 32 bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = fs::metadata(&key).expect("stat the key file").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the key file is owner-only");
    }

    let config_before = fs::read(&config).expect("read config before the second init");
    let second = common::run_in(std::path::Path::new(&home), &args);
    assert!(!second.status.success(), "a second init must not succeed");
    assert!(
        common::text(&second.stderr).contains("already exists"),
        "the failure names the existing file: {}",
        common::text(&second.stderr)
    );
    assert_eq!(fs::read(&config).expect("read config after"), config_before);
    assert_eq!(fs::read(&key).expect("read key after"), key_bytes);
}

#[test]
fn scan_reports_counts_and_a_rescan_is_a_full_cache_hit() {
    let fixture = common::Fixture::new(&[("a.md", "alpha one\n"), ("b.md", "bravo two\n")], KEY);

    let first = fixture.run(&["scan"]);
    assert!(first.status.success(), "{}", common::text(&first.stderr));
    let stdout = common::text(&first.stdout);
    assert!(stdout.contains("added: 2"), "the first scan adds both files:\n{stdout}");
    assert!(stdout.contains("embedded: 2"), "both chunks are embedded:\n{stdout}");
    assert!(stdout.contains("cache_hits: 0"), "{stdout}");

    let second = fixture.run(&["scan"]);
    assert!(second.status.success(), "{}", common::text(&second.stderr));
    let stdout = common::text(&second.stdout);
    assert!(stdout.contains("added: 0"), "{stdout}");
    assert!(stdout.contains("unchanged: 2"), "{stdout}");
    assert!(stdout.contains("embedded: 0"), "the rescan embeds nothing:\n{stdout}");
    assert!(stdout.contains("cache_hits: 2"), "the rescan is a full cache hit:\n{stdout}");
}

#[test]
fn query_returns_the_matching_chunk_after_scan() {
    let fixture = common::Fixture::new(&[("a.md", "alpha bravo\n")], KEY);

    let scan = fixture.run(&["scan"]);
    assert!(scan.status.success(), "{}", common::text(&scan.stderr));

    let query = fixture.run(&["query", "alpha"]);
    assert!(query.status.success(), "{}", common::text(&query.stderr));
    let stdout = common::text(&query.stdout);
    assert!(stdout.contains("a.md:1-1"), "the hit identifies its source:\n{stdout}");
    assert!(stdout.contains("alpha bravo"), "the hit carries the chunk text:\n{stdout}");
}

#[test]
fn doctor_without_a_usable_embedder_fails_clearly() {
    let dir = temp_dir();
    let config = dir.path().join("braga.toml");
    let key = dir.path().join("key");
    fs::write(&config, Config::default_toml().expect("render the defaults"))
        .expect("write default config");
    fs::write(&key, KEY).expect("write key file");

    let output = common::run_in(
        dir.path(),
        &["doctor", "--config", config.to_str().unwrap(), "--key-file", key.to_str().unwrap()],
    );
    assert!(!output.status.success(), "an unusable [embedding] must fail");
    assert!(
        common::text(&output.stderr).contains("embedding"),
        "the failure names the unusable embedding config: {}",
        common::text(&output.stderr)
    );
}

#[test]
fn doctor_prints_json_and_exits_zero_with_a_usable_embedder() {
    let fixture = common::Fixture::new(&[], KEY);

    let output = fixture.run(&["doctor"]);
    assert!(output.status.success(), "{}", common::text(&output.stderr));
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("doctor prints JSON");
    assert_eq!(report["ok"], true, "every check should pass");

    let checks = report["checks"].as_array().expect("the report lists its checks");
    let names: Vec<&str> =
        checks.iter().map(|check| check["name"].as_str().expect("check name")).collect();
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
}
