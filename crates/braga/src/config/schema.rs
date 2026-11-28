// SPDX-License-Identifier: Apache-2.0

//! The typed configuration schema and its built-in defaults.

use std::path::PathBuf;

/// Top-level `bRAGa` configuration.
///
/// See the [module documentation](super) for how the layers are merged.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// File-scanning settings.
    pub scan: Scan,
    /// Embedding provider settings.
    pub embedding: Embedding,
    /// Storage settings.
    pub storage: Storage,
}

// Hand-written rather than derived so that this impl stays the single place showing the assembled
// defaults; each section still owns its own values.
#[expect(
    clippy::derivable_impls,
    reason = "the assembled defaults are documented explicitly at the top level"
)]
impl Default for Config {
    fn default() -> Self {
        Self { scan: Scan::default(), embedding: Embedding::default(), storage: Storage::default() }
    }
}

/// File-scanning settings (`[scan]`).
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scan {
    /// Directory that is scanned for documents.
    pub dir: PathBuf,
    /// Whether the worker periodically re-reconciles on its own (`true`) or only on start and on
    /// [`ScanEngine::update`](crate::status::ScanEngine::update) (`false`).
    #[serde(default = "default_auto_scan")]
    pub auto_scan: bool,
    /// Seconds between periodic reconciles when [`auto_scan`](Self::auto_scan) is `true`.
    ///
    /// `0` is rejected at engine start while `auto_scan` is `true` (see
    /// [`crate::status::ScanEngine::start`]).
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
}

impl Default for Scan {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("."),
            auto_scan: default_auto_scan(),
            interval_secs: default_interval_secs(),
        }
    }
}

/// Default `scan.auto_scan`: `true`.
fn default_auto_scan() -> bool {
    true
}

/// Default `scan.interval_secs`: `300`.
fn default_interval_secs() -> u64 {
    300
}

/// Embedding provider settings (`[embedding]`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Embedding {
    /// Provider name (for example `ollama`).
    pub provider: String,
    /// Endpoint of the embedding service.
    pub url: String,
    /// Model used to embed documents and queries.
    pub model: String,
}

/// Storage settings (`[storage]`).
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Storage {
    /// Path of the `SQLite` database file.
    #[serde(default = "default_storage_path")]
    pub path: PathBuf,
}

impl Default for Storage {
    fn default() -> Self {
        Self { path: default_storage_path() }
    }
}

/// Default `SQLite` database path: `~/.braga/index.db`, or `braga.sqlite3` when no home directory
/// is known.
///
/// Placeholder until Stage 3 hardens platform directory handling with XDG base directories.
pub(super) fn default_storage_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty());
    match home {
        Some(home) => PathBuf::from(home).join(".braga").join("index.db"),
        None => PathBuf::from("braga.sqlite3"),
    }
}

/// Errors returned while resolving configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A referenced configuration file could not be read.
    #[error("failed to read configuration file: {0}")]
    Io(#[from] std::io::Error),
    /// A configuration layer is not valid `TOML`, or it names an unknown key.
    #[error("failed to parse configuration: {0}")]
    Toml(#[from] toml::de::Error),
    /// The default configuration could not be serialized to `TOML`.
    #[error("failed to serialize configuration: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// A `BRAGA_`-prefixed variable is not part of the recognized namespace.
    #[error("unrecognized environment variable `{0}`")]
    UnknownEnvVar(String),
    /// A recognized `BRAGA_`-prefixed variable is malformed or names an unknown leaf.
    #[error("invalid environment variable `{0}`")]
    InvalidEnvVar(String),
    /// A recognized leaf variable held a value that cannot be parsed for its field type.
    #[error("invalid value `{value}` for environment variable `{key}`")]
    InvalidEnvValue {
        /// The variable name, including the `BRAGA_` prefix.
        key: String,
        /// The unparsable value, as supplied.
        value: String,
    },
    /// `scan.auto_scan` is `true` but `scan.interval_secs` is `0`, so no periodic trigger is
    /// possible.
    #[error("auto_scan requires a non-zero scan.interval_secs")]
    InvalidScanInterval,
}
