// SPDX-License-Identifier: Apache-2.0

//! The `BRAGA_*__*` environment layer: partial tables and leaf application.

use std::path::PathBuf;

use super::{Config, ConfigError};

/// Prefix of every environment variable that participates in configuration resolution.
pub(super) const ENV_PREFIX: &str = "BRAGA_";

/// A configuration layer with every field optional, so only the named fields are applied.
///
/// Every struct rejects unknown keys, so a typo in any layer fails closed just like it does in the
/// fully typed schema.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct PartialConfig {
    /// Partial `[scan]` table.
    scan: PartialScan,
    /// Partial `[embedding]` table.
    embedding: PartialEmbedding,
    /// Partial `[storage]` table.
    storage: PartialStorage,
}

impl PartialConfig {
    /// Folds this layer over `config`, leaving unnamed fields untouched.
    pub(super) fn apply_to(self, config: &mut Config) {
        if let Some(dir) = self.scan.dir {
            config.scan.dir = dir;
        }
        if let Some(auto_scan) = self.scan.auto_scan {
            config.scan.auto_scan = auto_scan;
        }
        if let Some(interval_secs) = self.scan.interval_secs {
            config.scan.interval_secs = interval_secs;
        }
        if let Some(provider) = self.embedding.provider {
            config.embedding.provider = provider;
        }
        if let Some(url) = self.embedding.url {
            config.embedding.url = url;
        }
        if let Some(model) = self.embedding.model {
            config.embedding.model = model;
        }
        if let Some(path) = self.storage.path {
            config.storage.path = path;
        }
    }
}

/// Partial `[scan]` table.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialScan {
    /// Optional `scan.dir`.
    dir: Option<PathBuf>,
    /// Optional `scan.auto_scan`.
    auto_scan: Option<bool>,
    /// Optional `scan.interval_secs`.
    interval_secs: Option<u64>,
}

/// Partial `[embedding]` table.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialEmbedding {
    /// Optional `embedding.provider`.
    provider: Option<String>,
    /// Optional `embedding.url`.
    url: Option<String>,
    /// Optional `embedding.model`.
    model: Option<String>,
}

/// Partial `[storage]` table.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialStorage {
    /// Optional `storage.path`.
    path: Option<PathBuf>,
}

/// Parses one `SECTION__FIELD` variable into the leaf layer.
pub(super) fn apply_env_leaf(
    key: &str,
    suffix: &str,
    value: &str,
    leaves: &mut PartialConfig,
) -> Result<(), ConfigError> {
    let Some((section, field)) = suffix.split_once("__") else {
        return Err(classify_unrecognized(key, suffix));
    };
    if field.contains("__") {
        return Err(ConfigError::InvalidEnvVar(key.to_owned()));
    }
    match (section, field) {
        ("SCAN", "DIR") => leaves.scan.dir = Some(PathBuf::from(value)),
        ("SCAN", "AUTO_SCAN") => {
            leaves.scan.auto_scan = Some(value.parse().map_err(|_| {
                ConfigError::InvalidEnvValue { key: key.to_owned(), value: value.to_owned() }
            })?);
        }
        ("SCAN", "INTERVAL_SECS") => {
            leaves.scan.interval_secs = Some(value.parse().map_err(|_| {
                ConfigError::InvalidEnvValue { key: key.to_owned(), value: value.to_owned() }
            })?);
        }
        ("EMBEDDING", "PROVIDER") => leaves.embedding.provider = Some(value.to_owned()),
        ("EMBEDDING", "URL") => leaves.embedding.url = Some(value.to_owned()),
        ("EMBEDDING", "MODEL") => leaves.embedding.model = Some(value.to_owned()),
        ("STORAGE", "PATH") => leaves.storage.path = Some(PathBuf::from(value)),
        _ => return Err(ConfigError::InvalidEnvVar(key.to_owned())),
    }
    Ok(())
}

/// Classifies a `BRAGA_`-prefixed variable that is not a leaf and not a meta variable.
///
/// A known section without a field is a malformed leaf override; anything else is outside the
/// recognized namespace.
fn classify_unrecognized(key: &str, suffix: &str) -> ConfigError {
    match suffix {
        "SCAN" | "EMBEDDING" | "STORAGE" => ConfigError::InvalidEnvVar(key.to_owned()),
        _ => ConfigError::UnknownEnvVar(key.to_owned()),
    }
}
