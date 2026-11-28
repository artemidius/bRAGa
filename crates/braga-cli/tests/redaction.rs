// SPDX-License-Identifier: Apache-2.0

//! The telemetry leak gate: verbose commands must log their spans, never file content or key
//! material, and telemetry must stay off by default.

mod common;

use std::fs;

const CANARY: &str = "BRAGA-CLI-REDACTION-CANARY-0f5c2e7a-unique-file-content";
const KEY: [u8; 32] = [0xA7; 32];

#[test]
fn verbose_scan_logs_its_span_but_not_content_or_key_material() {
    let fixture = common::Fixture::new(&[("secret.md", CANARY)], KEY);

    let output = fixture.run(&["--verbose", "scan"]);
    assert!(output.status.success(), "{}", common::text(&output.stderr));

    let stdout = common::text(&output.stdout);
    let stderr = common::text(&output.stderr);

    assert!(stdout.contains("added: 1"), "results stay on stdout:\n{stdout}");
    assert!(stderr.contains("scan"), "the scan span is the positive control:\n{stderr}");
    assert!(stderr.contains("embed.call"), "the embed event must fire:\n{stderr}");

    assert!(!stderr.contains(CANARY), "file content must never be logged:\n{stderr}");
    assert!(
        !common::contains(&output.stderr, &KEY),
        "raw key bytes must never be logged:\n{stderr}"
    );
    assert!(
        !stderr.contains(&common::hex(&KEY)),
        "the hex-encoded key must never be logged:\n{stderr}"
    );
}

#[test]
fn init_does_not_echo_the_generated_key() {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let config = dir.path().join("braga.toml");
    let key = dir.path().join("key");

    let output = common::run_in(
        dir.path(),
        &[
            "--verbose",
            "init",
            "--config",
            config.to_str().unwrap(),
            "--key-file",
            key.to_str().unwrap(),
        ],
    );
    assert!(output.status.success(), "{}", common::text(&output.stderr));

    let key_bytes = fs::read(&key).expect("read the generated key");
    assert_eq!(key_bytes.len(), 32, "the key file is exactly 32 bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = fs::metadata(&key).expect("stat the key file").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the key file is owner-only");
    }

    let stderr = common::text(&output.stderr);
    assert!(stderr.contains("init"), "verbose init must log its positive control:\n{stderr}");
    assert!(
        !common::contains(&output.stderr, &key_bytes),
        "raw key bytes must never be logged:\n{stderr}"
    );
    assert!(
        !stderr.contains(&common::hex(&key_bytes)),
        "the hex-encoded key must never be logged:\n{stderr}"
    );
}

#[test]
fn telemetry_is_off_by_default() {
    let fixture = common::Fixture::new(&[("a.md", "alpha bravo\n")], KEY);

    let output = fixture.run(&["scan"]);
    assert!(output.status.success(), "{}", common::text(&output.stderr));
    assert!(common::text(&output.stdout).contains("added: 1"), "results still reach stdout");
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty without --verbose or BRAGA_LOG:\n{}",
        common::text(&output.stderr)
    );
}
