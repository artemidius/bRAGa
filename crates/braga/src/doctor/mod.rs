// SPDX-License-Identifier: Apache-2.0

//! The claims verifier behind the `doctor` gate.
//!
//! [`doctor`] runs a fixed, ordered set of checks over the Stage 1–7b stack and returns a
//! [`DoctorReport`] whose [`ok`](DoctorReport::ok) is the conjunction of every check. A failing
//! check is reported as a [`Check`] entry, not as an error: [`DoctorError`] is reserved for a
//! scratch-directory setup failure, so a machine consumer always receives a report when one can be
//! built. Config resolution is the caller's job; a resolution failure surfaces before `doctor`.
//!
//! The checks are, in order:
//!
//! 1. `config.valid` — the resolved [`Config`](crate::config::Config) satisfies its invariants, and
//!    the built-in defaults survive a TOML round-trip.
//! 2. `store.open` — the configured store opens and closes with the supplied
//!    [`Key`](braga_storage::Key).
//! 3. `embed.roundtrip` — a probe chunk is seeded, embedded, and retrieved as its own top hit,
//!    exercising chunker → `Store` → `vec0` → FTS5 → RRF.
//! 4. `crypto.roundtrip` — a unique canary survives checkpoint, close, and reopen, and a different
//!    key is rejected with
//!    [`StoreError::WrongKey`](braga_storage::StoreError::WrongKey).
//! 5. `encryption.at_rest` — the canary never appears in plaintext in the scratch `-wal`, `-shm`,
//!    or main database.
//! 6. `embed.live` — optional, gated by `BRAGA_DOCTOR_LIVE_EMBED`: the configured provider answers
//!    one probe. When it is skipped, the check still passes and its `detail` begins `skipped`, the
//!    documented way a machine consumer tells a skip from a real pass.
//!
//! Checks 3–5 run against a `tempfile` scratch directory, never against
//! [`config.storage.path`](crate::config::Config), so a failed doctor cannot corrupt the live index
//! and a passing one leaves no canary behind. `store.open` is the one check that touches the
//! configured path: a missing file is created (the same side effect as any store open), so point
//! `doctor` at a real configuration only deliberately.
//!
//! # Offline by default
//!
//! `doctor` never touches the network unless `BRAGA_DOCTOR_LIVE_EMBED` is set to any non-empty
//! value, so the default (CI) run is hermetic: config validity, key/DB plumbing, schema v1 + WAL,
//! embed → store → query, the cryptographic round-trip, and encryption at rest all run offline
//! with a fixed [`Embedder`](braga_embedder::Embedder).
//!
//! # What `doctor` does not claim
//!
//! It cannot verify that a live embedding provider responds (that is check 6, opt-in), and it says
//! nothing about the source folder, keyfile custody, or live process memory. Those are the
//! threat-model's documented non-goals; `doctor` makes no security claim it cannot check.

use thiserror::Error;

mod checks;
mod helpers;
mod scratch;

pub use checks::doctor;

/// Stable check name for the configuration check.
const CONFIG_VALID: &str = "config.valid";
/// Stable check name for the store-open check.
const STORE_OPEN: &str = "store.open";
/// Stable check name for the embed round-trip check.
const EMBED_ROUNDTRIP: &str = "embed.roundtrip";
/// Stable check name for the cryptographic round-trip check.
const CRYPTO_ROUNDTRIP: &str = "crypto.roundtrip";
/// Stable check name for the encryption-at-rest check.
const ENCRYPTION_AT_REST: &str = "encryption.at_rest";
/// Stable check name for the optional live-provider check.
const EMBED_LIVE: &str = "embed.live";

/// Text seeded as the probe chunk; its hash is the expected retrieval id.
const PROBE_TEXT: &str = "doctor probe: the quick brown fox jumps over the lazy dog";
/// Near-identical query text, without the colon that FTS5 would parse as a column filter.
const PROBE_QUERY: &str = "the quick brown fox jumps over a lazy dog";
/// Path of the scratch file that holds the probe chunk.
const PROBE_PATH: &str = "doctor-probe.md";

/// Environment variable that opts into the live-provider check.
const LIVE_ENV: &str = "BRAGA_DOCTOR_LIVE_EMBED";
/// Detail reported when the live check is skipped.
const LIVE_SKIPPED: &str =
    "skipped (set BRAGA_DOCTOR_LIVE_EMBED=1 to verify the configured provider)";
/// Text sent to the configured provider by the live check.
const LIVE_PROBE: &str = "bRAGa doctor liveness probe";

/// One result of a [`doctor`] check.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Check {
    /// Stable dotted key, for example `store.open`.
    pub name: &'static str,
    /// Whether the check passed.
    pub passed: bool,
    /// Human-readable outcome; sanitized, never key material, and only doctor's own probe text can
    /// appear.
    pub detail: String,
}

impl Check {
    /// Builds a passing check.
    fn passed(name: &'static str, detail: String) -> Self {
        Self { name, passed: true, detail }
    }

    /// Builds a failing check.
    fn failed(name: &'static str, detail: String) -> Self {
        Self { name, passed: false, detail }
    }
}

/// The ordered result of one [`doctor`] run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorReport {
    /// Every check, in the fixed order documented on the [module](self).
    pub checks: Vec<Check>,
    /// The conjunction of every check's `passed` flag.
    pub ok: bool,
}

impl DoctorReport {
    /// Renders the report as pretty-printed JSON.
    ///
    /// # Errors
    ///
    /// Returns [`serde_json::Error`] if the report cannot be serialized, which would be a bug
    /// rather than a doctor finding.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Error returned when `doctor` cannot build a report at all.
#[derive(Debug, Error)]
pub enum DoctorError {
    /// The scratch directory for the destructive checks could not be created.
    #[error("doctor setup failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use braga_storage::{Key, Store};

    use super::helpers::{new_canary, wrong_key};
    use super::scratch::{at_rest_check, canary_rejects_wrong_key};

    /// Evidence containing no canary, with one required and one optional sidecar present.
    fn clean_evidence() -> [(&'static str, std::io::Result<Option<Vec<u8>>>); 4] {
        [
            ("the scratch -wal before the checkpoint", Ok(Some(Vec::new()))),
            ("the scratch -shm before the checkpoint", Ok(None)),
            ("the scratch database after the checkpoint", Ok(Some(vec![0; 16]))),
            ("the scratch -wal after the checkpoint", Ok(Some(Vec::new()))),
        ]
    }

    /// The at-rest check must flag plaintext, not just silently pass.
    #[test]
    fn at_rest_check_flags_a_plaintext_canary_in_a_scratch_wal() {
        let canary = new_canary();
        let mut injected = b"SQLite format 3\0".to_vec();
        injected.extend_from_slice(canary.as_bytes());
        let check = at_rest_check(
            [
                ("the scratch -wal before the checkpoint", Ok(Some(injected))),
                ("the scratch -shm before the checkpoint", Ok(None)),
                ("the scratch database after the checkpoint", Ok(Some(vec![0; 16]))),
                ("the scratch -wal after the checkpoint", Ok(Some(Vec::new()))),
            ],
            canary.as_bytes(),
            true,
        );

        assert!(!check.passed, "an injected canary must fail the scan");
        assert!(check.detail.contains("found the plaintext canary"), "{}", check.detail);
        assert!(check.detail.contains("before the checkpoint"), "{}", check.detail);
    }

    /// Absence is trusted only once the canary has been proven written.
    #[test]
    fn at_rest_check_is_inconclusive_until_the_canary_is_proven_written() {
        let canary = new_canary();

        let proven = at_rest_check(clean_evidence(), canary.as_bytes(), true);
        assert!(proven.passed, "{}", proven.detail);

        let unproven = at_rest_check(clean_evidence(), canary.as_bytes(), false);
        assert!(!unproven.passed, "an unproven canary must not pass");
        assert!(unproven.detail.contains("inconclusive"), "{}", unproven.detail);
    }

    /// The wrong-key assertion must fail when the "wrong" key is in fact the store's key.
    #[test]
    fn canary_rejects_wrong_key_catches_a_key_that_does_open_the_store() {
        let dir = tempfile::tempdir().expect("create scratch directory");
        let db = dir.path().join("doctor.sqlite3");
        let key = Key::from_bytes([0x11; 32]);
        Store::open(&db, &key).expect("open the scratch store").close().expect("close");

        assert!(
            canary_rejects_wrong_key(&db, &key).is_err(),
            "a key that opens the store must fail the negative assertion"
        );
        assert!(
            canary_rejects_wrong_key(&db, &wrong_key(&key)).is_ok(),
            "a genuinely different key must be rejected"
        );
    }
}
