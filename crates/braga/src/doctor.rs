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
//! 1. `config.valid` — the resolved [`Config`] satisfies its invariants, and the built-in defaults
//!    survive a TOML round-trip.
//! 2. `store.open` — the configured store opens and closes with the supplied [`Key`].
//! 3. `embed.roundtrip` — a probe chunk is seeded, embedded, and retrieved as its own top hit,
//!    exercising chunker → `Store` → `vec0` → FTS5 → RRF.
//! 4. `crypto.roundtrip` — a unique canary survives checkpoint, close, and reopen, and a different
//!    key is rejected with [`StoreError::WrongKey`].
//! 5. `encryption.at_rest` — the canary never appears in plaintext in the scratch `-wal`, `-shm`,
//!    or main database.
//! 6. `embed.live` — optional, gated by `BRAGA_DOCTOR_LIVE_EMBED`: the configured provider answers
//!    one probe. When it is skipped, the check still passes and its `detail` begins `skipped`, the
//!    documented way a machine consumer tells a skip from a real pass.
//!
//! Checks 3–5 run against a `tempfile` scratch directory, never against
//! [`config.storage.path`](Config), so a failed doctor cannot corrupt the live index and a passing
//! one leaves no canary behind. `store.open` is the one check that touches the configured path: a
//! missing file is created (the same side effect as any store open), so point `doctor` at a real
//! configuration only deliberately.
//!
//! # Offline by default
//!
//! `doctor` never touches the network unless `BRAGA_DOCTOR_LIVE_EMBED` is set to any non-empty
//! value, so the default (CI) run is hermetic: config validity, key/DB plumbing, schema v1 + WAL,
//! embed → store → query, the cryptographic round-trip, and encryption at rest all run offline
//! with a fixed [`Embedder`].
//!
//! # What `doctor` does not claim
//!
//! It cannot verify that a live embedding provider responds (that is check 6, opt-in), and it says
//! nothing about the source folder, keyfile custody, or live process memory. Those are the
//! threat-model's documented non-goals; `doctor` makes no security claim it cannot check.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use braga_chunker::{ContentHash, content_hash};
use braga_embedder::Embedder;
use braga_storage::{Key, NewChunk, Store, StoreError};
use thiserror::Error;

use crate::{
    config::Config,
    embedding::{EmbeddingPipeline, Metric, build_embedder},
    retrieval,
};

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

/// Runs the fixed check set over `config`, `key`, and `embedder`.
///
/// Per-check failures are [`Check`] entries inside the returned report; only a scratch-directory
/// setup failure returns `Err`.
///
/// # Errors
///
/// Returns [`DoctorError::Io`] if the scratch directory for the destructive checks cannot be
/// created.
pub fn doctor(
    config: &Config,
    key: &Key,
    embedder: &dyn Embedder,
) -> Result<DoctorReport, DoctorError> {
    let scratch = tempfile::tempdir()?;
    let [embed, crypto, at_rest] = scratch_checks(scratch.path(), key, embedder);
    let mut checks = Vec::with_capacity(6);
    checks.push(check_config(config));
    checks.push(check_store(config, key));
    checks.extend([embed, crypto, at_rest]);
    checks.push(check_live(config));
    let ok = checks.iter().all(|check| check.passed);
    Ok(DoctorReport { checks, ok })
}

/// `config.valid`: the resolved configuration satisfies its invariants and its defaults round-trip.
fn check_config(config: &Config) -> Check {
    if config.scan.auto_scan && config.scan.interval_secs == 0 {
        return Check::failed(
            CONFIG_VALID,
            "scan.auto_scan is true but scan.interval_secs is 0".to_owned(),
        );
    }

    let round_trip = Config::default_toml()
        .map_err(|error| error.to_string())
        .and_then(|toml| Config::from_toml_str(&toml).map_err(|error| error.to_string()));
    match round_trip {
        Ok(defaults) if defaults == Config::default() => Check::passed(
            CONFIG_VALID,
            "the resolved config is valid and the default TOML round-trips".to_owned(),
        ),
        Ok(_) => Check::failed(
            CONFIG_VALID,
            "the default TOML does not parse back to the built-in defaults".to_owned(),
        ),
        Err(error) => {
            Check::failed(CONFIG_VALID, format!("the default TOML self-check failed: {error}"))
        }
    }
}

/// `store.open`: the configured store opens and closes with the supplied key.
fn check_store(config: &Config, key: &Key) -> Check {
    match Store::open(&config.storage.path, key) {
        Ok(store) => match store.close() {
            Ok(()) => Check::passed(
                STORE_OPEN,
                "the configured store opened and closed with the supplied key".to_owned(),
            ),
            Err(error) => Check::failed(
                STORE_OPEN,
                format!("the configured store did not close cleanly: {error}"),
            ),
        },
        Err(error) => {
            Check::failed(STORE_OPEN, format!("the configured store did not open: {error}"))
        }
    }
}

/// `embed.live`: opt-in probe of the configured provider.
fn check_live(config: &Config) -> Check {
    match std::env::var(LIVE_ENV) {
        Ok(value) if !value.is_empty() => {}
        _ => return Check::passed(EMBED_LIVE, LIVE_SKIPPED.to_owned()),
    }

    match build_embedder(&config.embedding) {
        Ok(embedder) => match embedder.embed(&[LIVE_PROBE]) {
            Ok(vectors) => Check::passed(
                EMBED_LIVE,
                format!("the configured provider returned {} vector(s)", vectors.len()),
            ),
            Err(error) => {
                Check::failed(EMBED_LIVE, format!("the configured provider failed: {error}"))
            }
        },
        Err(error) => {
            Check::failed(EMBED_LIVE, format!("the embedding configuration is unusable: {error}"))
        }
    }
}

/// Runs the three destructive checks against one scratch store, in order.
fn scratch_checks(dir: &Path, key: &Key, embedder: &dyn Embedder) -> [Check; 3] {
    let db = dir.join("doctor.sqlite3");
    let wal = sidecar(&db, "-wal");
    let shm = sidecar(&db, "-shm");

    let store = match Store::open(&db, key) {
        Ok(store) => store,
        Err(error) => {
            let detail = format!("the scratch store could not be opened: {error}");
            return [
                Check::failed(EMBED_ROUNDTRIP, detail.clone()),
                Check::failed(CRYPTO_ROUNDTRIP, detail.clone()),
                Check::failed(ENCRYPTION_AT_REST, detail),
            ];
        }
    };

    let embed = embed_roundtrip(&store, embedder);

    let canary = new_canary();
    let canary_hash = content_hash(canary.as_bytes());
    let mut crypto_failures = Vec::new();
    if let Err(error) = store.sync_file(&canary, canary_hash.as_bytes(), &[]) {
        crypto_failures.push(format!("writing the canary failed: {error}"));
    }

    // The pending canary lives in the write-ahead log before the checkpoint; the main database
    // holds it after. Both are captured raw, exactly as they sit on disk.
    let wal_before = read_optional(&wal);
    let shm_before = read_optional(&shm);
    if let Err(error) = store.checkpoint() {
        crypto_failures.push(format!("checkpointing the scratch store failed: {error}"));
    }
    let db_after = std::fs::read(&db).map(Some);
    let wal_after = read_optional(&wal);
    if let Err(error) = store.close() {
        crypto_failures.push(format!("closing the scratch store failed: {error}"));
    }

    let readback = canary_readback(&db, key, &canary, canary_hash.as_bytes());
    if let Err(failure) = &readback {
        crypto_failures.push(failure.clone());
    }
    if let Err(failure) = canary_rejects_wrong_key(&db, &wrong_key(key)) {
        crypto_failures.push(failure);
    }

    let crypto = if crypto_failures.is_empty() {
        Check::passed(
            CRYPTO_ROUNDTRIP,
            "the canary survived checkpoint, close, and reopen, and a different key was rejected"
                .to_owned(),
        )
    } else {
        Check::failed(CRYPTO_ROUNDTRIP, crypto_failures.join("; "))
    };
    let at_rest = at_rest_check(
        [
            ("the scratch -wal before the checkpoint", wal_before),
            ("the scratch -shm before the checkpoint", shm_before),
            ("the scratch database after the checkpoint", db_after),
            ("the scratch -wal after the checkpoint", wal_after),
        ],
        canary.as_bytes(),
        readback.is_ok(),
    );
    [embed, crypto, at_rest]
}

/// `embed.roundtrip`: seed one chunk, embed it, and retrieve it as the top hit.
fn embed_roundtrip(store: &Store, embedder: &dyn Embedder) -> Check {
    let chunk_id = content_hash(PROBE_TEXT.as_bytes());
    let chunk = NewChunk {
        id: chunk_id.as_bytes(),
        byte_offset: 0,
        line_start: 1,
        line_end: 1,
        content: PROBE_TEXT,
    };
    if let Err(error) = store.sync_file(PROBE_PATH, chunk_id.as_bytes(), &[chunk]) {
        return Check::failed(EMBED_ROUNDTRIP, format!("seeding the probe chunk failed: {error}"));
    }

    let chunks: [(ContentHash, &str); 1] = [(chunk_id, PROBE_TEXT)];
    if let Err(error) = EmbeddingPipeline::new(Metric::Cosine).run(store, embedder, &chunks) {
        return Check::failed(
            EMBED_ROUNDTRIP,
            format!("embedding the probe chunk failed: {error}"),
        );
    }

    match retrieval::query(store, embedder, PROBE_QUERY, 1) {
        Ok(hits) => match hits.first() {
            Some(hit) if hit.chunk_id.as_slice() == chunk_id.as_bytes() => Check::passed(
                EMBED_ROUNDTRIP,
                "the probe chunk round-tripped through the store and was its own top hit"
                    .to_owned(),
            ),
            Some(hit) => Check::failed(
                EMBED_ROUNDTRIP,
                format!(
                    "the top hit was {:?} at {}, not the seeded probe chunk",
                    hit.content, hit.path
                ),
            ),
            None => Check::failed(
                EMBED_ROUNDTRIP,
                "retrieval returned no chunks for the seeded probe".to_owned(),
            ),
        },
        Err(error) => Check::failed(EMBED_ROUNDTRIP, format!("querying the probe failed: {error}")),
    }
}

/// Reopens `db` with `key` and asserts the canary file row survived.
fn canary_readback(
    db: &Path,
    key: &Key,
    canary: &str,
    canary_hash: &[u8; 32],
) -> Result<(), String> {
    let store = Store::open(db, key)
        .map_err(|error| format!("reopening the scratch store failed: {error}"))?;
    let files = store
        .list_files()
        .map_err(|error| format!("listing the canary file rows failed: {error}"))?;
    let survived =
        files.iter().any(|row| row.path == canary && row.content_hash == canary_hash.as_slice());
    store.close().map_err(|error| format!("closing the reopened scratch store failed: {error}"))?;
    if survived {
        Ok(())
    } else {
        Err("the canary file row did not survive checkpoint, close, and reopen".to_owned())
    }
}

/// Asserts that `wrong` cannot decrypt `db`.
fn canary_rejects_wrong_key(db: &Path, wrong: &Key) -> Result<(), String> {
    match Store::open(db, wrong) {
        Ok(store) => {
            if let Err(error) = store.close() {
                return Err(format!("closing a store opened with a different key failed: {error}"));
            }
            Err("a different key opened the encrypted scratch database".to_owned())
        }
        Err(StoreError::WrongKey) => Ok(()),
        Err(error) => Err(format!("a different key failed unexpectedly: {error}")),
    }
}

/// `encryption.at_rest`: the canary must not appear in the raw scratch files.
///
/// `evidence` carries the buffers captured around the checkpoint, and `proven` records whether the
/// readback proved the canary was actually written before its absence is trusted.
fn at_rest_check(
    evidence: [(&'static str, std::io::Result<Option<Vec<u8>>>); 4],
    canary: &[u8],
    proven: bool,
) -> Check {
    let mut failures = Vec::new();
    let mut scanned_files = 0_usize;
    let mut scanned_bytes = 0_usize;
    for (label, read) in evidence {
        match read {
            Ok(Some(bytes)) => {
                scanned_files += 1;
                scanned_bytes += bytes.len();
                if contains(&bytes, canary) {
                    failures.push(format!("found the plaintext canary in {label}"));
                }
            }
            // A missing optional sidecar is not evidence either way; the main database is
            // required, so its read can only fail.
            Ok(None) => {}
            Err(error) => failures.push(format!("could not read {label}: {error}")),
        }
    }

    if !failures.is_empty() {
        return Check::failed(ENCRYPTION_AT_REST, failures.join("; "));
    }
    if !proven {
        return Check::failed(
            ENCRYPTION_AT_REST,
            "the canary was not proven written, so the plaintext scan is inconclusive".to_owned(),
        );
    }
    Check::passed(
        ENCRYPTION_AT_REST,
        format!(
            "the plaintext canary is absent from {scanned_files} scratch file(s) ({scanned_bytes} bytes)"
        ),
    )
}

/// Builds the sidecar path (`<db>-wal`, `<db>-shm`, ...) by appending to the file name.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Reads a file whose absence is acceptable; other I/O failures are reported.
fn read_optional(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// A long, per-run value that must never appear in a file at rest.
fn new_canary() -> String {
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    format!("BRAGA-DOCTOR-CANARY-{}-{nanos:032x}", std::process::id())
}

/// Derives a key that is guaranteed different from `key`, without exposing either.
fn wrong_key(key: &Key) -> Key {
    let mut bytes = *key.as_bytes();
    for byte in &mut bytes {
        *byte ^= 0xFF;
    }
    Key::from_bytes(bytes)
}

/// Sub-slice search, non-empty needle required because `windows(0)` panics.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

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
