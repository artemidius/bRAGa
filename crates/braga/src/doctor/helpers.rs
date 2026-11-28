// SPDX-License-Identifier: Apache-2.0

//! Small utilities shared by the `doctor` checks.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use braga_storage::Key;

/// Builds the sidecar path (`<db>-wal`, `<db>-shm`, ...) by appending to the file name.
pub(super) fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Reads a file whose absence is acceptable; other I/O failures are reported.
pub(super) fn read_optional(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// A long, per-run value that must never appear in a file at rest.
pub(super) fn new_canary() -> String {
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    format!("BRAGA-DOCTOR-CANARY-{}-{nanos:032x}", std::process::id())
}

/// Derives a key that is guaranteed different from `key`, without exposing either.
pub(super) fn wrong_key(key: &Key) -> Key {
    let mut bytes = *key.as_bytes();
    for byte in &mut bytes {
        *byte ^= 0xFF;
    }
    Key::from_bytes(bytes)
}

/// Sub-slice search, non-empty needle required because `windows(0)` panics.
pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
}
