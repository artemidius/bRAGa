// SPDX-License-Identifier: Apache-2.0

//! The destructive checks that run against a throwaway scratch store.

use std::path::Path;

use braga_chunker::{ContentHash, content_hash};
use braga_embedder::Embedder;
use braga_storage::{Key, NewChunk, Store, StoreError};

use crate::embedding::{EmbeddingPipeline, Metric};
use crate::retrieval;

use super::helpers::{contains, new_canary, read_optional, sidecar, wrong_key};
use super::{
    CRYPTO_ROUNDTRIP, Check, EMBED_ROUNDTRIP, ENCRYPTION_AT_REST, PROBE_PATH, PROBE_QUERY,
    PROBE_TEXT,
};

/// Runs the three destructive checks against one scratch store, in order.
pub(super) fn scratch_checks(dir: &Path, key: &Key, embedder: &dyn Embedder) -> [Check; 3] {
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
pub(super) fn canary_rejects_wrong_key(db: &Path, wrong: &Key) -> Result<(), String> {
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
pub(super) fn at_rest_check(
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
