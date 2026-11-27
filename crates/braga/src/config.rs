// SPDX-License-Identifier: Apache-2.0

//! Configuration model for `bRAGa`.
//!
//! Configuration is read from a `TOML` file (`braga.toml` in later stages). Every section is
//! optional and falls back to [`Default`]; unknown keys are rejected so that a typo fails closed
//! instead of being silently ignored.

use std::path::{Path, PathBuf};

/// Top-level `bRAGa` configuration.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Directory that is scanned for documents.
    pub scan_dir: PathBuf,
    /// Embedding provider settings.
    pub embedding: Embedding,
    /// Storage settings.
    pub storage: Storage,
}

impl Default for Config {
    /// Stage-1 placeholder defaults; Stage 2 owns the real defaults and environment precedence.
    fn default() -> Self {
        Self {
            scan_dir: PathBuf::from("."),
            embedding: Embedding::default(),
            storage: Storage { path: default_storage_path() },
        }
    }
}

impl Config {
    /// Loads configuration from the `TOML` file at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if the file cannot be read, and [`ConfigError::Toml`] if the
    /// contents are not valid configuration (including unknown keys).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        Self::from_toml_str(&contents)
    }

    /// Parses configuration from a `TOML` string.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Toml`] if the string is not valid configuration (including unknown
    /// keys).
    pub fn from_toml_str(contents: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(contents)?)
    }
}

/// Embedding provider settings.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Embedding {
    /// Provider name (for example `ollama`).
    pub provider: String,
    /// Endpoint of the embedding service.
    pub url: String,
    /// Model used to embed documents and queries.
    pub model: String,
}

/// Storage settings.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Storage {
    /// Path of the `SQLite` database file.
    #[serde(default = "default_storage_path")]
    pub path: PathBuf,
}

/// Default `SQLite` database path.
fn default_storage_path() -> PathBuf {
    PathBuf::from("braga.sqlite3")
}

/// Errors returned while loading configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[error("failed to read configuration file: {0}")]
    Io(#[from] std::io::Error),
    /// The configuration is not valid `TOML`, or it contains unknown keys.
    #[error("failed to parse configuration file: {0}")]
    Toml(#[from] toml::de::Error),
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    const FULL: &str = r#"
        scan_dir = "/tmp/docs"

        [embedding]
        provider = "ollama"
        url = "http://localhost:11434"
        model = "nomic-embed-text"

        [storage]
        path = "/tmp/braga.sqlite3"
    "#;

    #[test]
    fn default_config_uses_placeholder_values() {
        let config = Config::default();
        assert_eq!(config.scan_dir, PathBuf::from("."));
        assert_eq!(config.embedding, Embedding::default());
        assert_eq!(config.embedding.provider, "");
        assert_eq!(config.embedding.url, "");
        assert_eq!(config.embedding.model, "");
        assert_eq!(config.storage.path, PathBuf::from("braga.sqlite3"));
    }

    #[test]
    fn parses_full_config() {
        let config = Config::from_toml_str(FULL).expect("full config should parse");
        assert_eq!(config.scan_dir, PathBuf::from("/tmp/docs"));
        assert_eq!(config.embedding.provider, "ollama");
        assert_eq!(config.embedding.url, "http://localhost:11434");
        assert_eq!(config.embedding.model, "nomic-embed-text");
        assert_eq!(config.storage.path, PathBuf::from("/tmp/braga.sqlite3"));
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config = Config::from_toml_str(r#"scan_dir = "/tmp/docs""#)
            .expect("partial config should parse");
        assert_eq!(config.scan_dir, PathBuf::from("/tmp/docs"));
        assert_eq!(config.embedding, Embedding::default());
        assert_eq!(config.storage.path, PathBuf::from("braga.sqlite3"));
    }

    #[test]
    fn unknown_key_fails_closed() {
        let error = Config::from_toml_str("scan_dir = \".\"\nunknown_key = true")
            .expect_err("unknown keys must be rejected");
        assert!(matches!(error, ConfigError::Toml(_)));
    }

    #[test]
    fn load_round_trips_a_file() {
        let pid = std::process::id();
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock").as_nanos();
        let path = std::env::temp_dir().join(format!("braga-config-{pid}-{nanos}.toml"));
        std::fs::write(&path, FULL).expect("temp config should be writable");

        let loaded = Config::load(&path);
        let cleanup = std::fs::remove_file(&path);
        cleanup.expect("temp config should be removable");

        let expected = Config::from_toml_str(FULL).expect("full config should parse");
        assert_eq!(loaded.expect("config should load"), expected);
    }
}
