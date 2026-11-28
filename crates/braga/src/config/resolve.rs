// SPDX-License-Identifier: Apache-2.0

//! Layered configuration resolution: `TOML` files, inline documents, and environment leaves.

use std::path::Path;

use super::{
    Config, ConfigError,
    env::{ENV_PREFIX, PartialConfig, apply_env_leaf},
};

impl Config {
    /// Resolves the effective configuration from every available layer.
    ///
    /// Precedence, highest first:
    ///
    /// 1. `explicit` — the path passed by the caller,
    /// 2. `BRAGA_CONFIG` — an inline `TOML` document,
    /// 3. `BRAGA_TOML` — the path of a `TOML` file,
    /// 4. `BRAGA_*__*` leaf overrides,
    /// 5. the built-in defaults.
    ///
    /// A layer contributes only the fields it names, so a higher layer that omits a field leaves
    /// the lower layers' value in place. No configuration file is discovered implicitly: a file
    /// joins the pipeline only through `explicit` or `BRAGA_TOML`.
    ///
    /// # Environment namespace
    ///
    /// Exactly these names are recognized; every other `BRAGA_`-prefixed variable is an error.
    ///
    /// | Variable | Field |
    /// |---|---|
    /// | `BRAGA_TOML` | path to a `braga.toml` file |
    /// | `BRAGA_CONFIG` | inline `TOML` document |
    /// | `BRAGA_SCAN__DIR` | `scan.dir` |
    /// | `BRAGA_SCAN__AUTO_SCAN` | `scan.auto_scan` |
    /// | `BRAGA_SCAN__INTERVAL_SECS` | `scan.interval_secs` |
    /// | `BRAGA_EMBEDDING__PROVIDER` | `embedding.provider` |
    /// | `BRAGA_EMBEDDING__URL` | `embedding.url` |
    /// | `BRAGA_EMBEDDING__MODEL` | `embedding.model` |
    /// | `BRAGA_STORAGE__PATH` | `storage.path` |
    ///
    /// Leaf names are split on the literal `__` into `SECTION__FIELD`; matching is
    /// case-sensitive and exactly two segments are accepted. A malformed name that clearly aims at
    /// a leaf — it contains `__`, or it names a known section without a field — is
    /// [`ConfigError::InvalidEnvVar`]; any other unrecognized `BRAGA_*` name is
    /// [`ConfigError::UnknownEnvVar`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if a referenced file cannot be read, [`ConfigError::Toml`] if a
    /// `TOML` layer is invalid or names an unknown key, and [`ConfigError::UnknownEnvVar`] or
    /// [`ConfigError::InvalidEnvVar`] for unrecognized environment variables.
    pub fn resolve(explicit: Option<&Path>) -> Result<Self, ConfigError> {
        resolve_from_env(explicit, std::env::vars())
    }

    /// Renders the built-in defaults as the `TOML` document that `braga init` will write.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Serialize`] if the defaults cannot be serialized, which would be a
    /// bug rather than a configuration problem.
    pub fn default_toml() -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(&Self::default())?)
    }

    /// Parses configuration from a single `TOML` string, filling unnamed fields from the defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Toml`] if the string is not valid configuration, including when it
    /// names an unknown key at any nesting level.
    pub fn from_toml_str(contents: &str) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        merge_toml(&mut config, contents)?;
        Ok(config)
    }

    /// Loads configuration from the `TOML` file at `path`, filling unnamed fields from the
    /// defaults.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if the file cannot be read, and [`ConfigError::Toml`] if the
    /// contents are not valid configuration (including unknown keys).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        merge_file(&mut config, path.as_ref())?;
        Ok(config)
    }
}

/// The pure resolution pipeline behind [`Config::resolve`].
///
/// The environment is injected so that precedence can be tested without mutating — and racing
/// on — the real process environment.
pub(super) fn resolve_from_env(
    explicit: Option<&Path>,
    env: impl Iterator<Item = (String, String)>,
) -> Result<Config, ConfigError> {
    let mut toml_path = None;
    let mut inline = None;
    let mut leaves = PartialConfig::default();

    for (key, value) in env {
        let Some(suffix) = key.strip_prefix(ENV_PREFIX) else {
            continue;
        };
        match suffix {
            "TOML" => toml_path = Some(value),
            "CONFIG" => inline = Some(value),
            _ => apply_env_leaf(&key, suffix, &value, &mut leaves)?,
        }
    }

    let mut config = Config::default();
    leaves.apply_to(&mut config);
    if let Some(path) = toml_path {
        merge_file(&mut config, Path::new(&path))?;
    }
    if let Some(contents) = inline {
        merge_toml(&mut config, &contents)?;
    }
    if let Some(path) = explicit {
        merge_file(&mut config, path)?;
    }
    Ok(config)
}

/// Parses one `TOML` layer and folds it over `config`.
fn merge_toml(config: &mut Config, contents: &str) -> Result<(), ConfigError> {
    let layer: PartialConfig = toml::from_str(contents)?;
    layer.apply_to(config);
    Ok(())
}

/// Reads one `TOML` file layer and folds it over `config`.
fn merge_file(config: &mut Config, path: &Path) -> Result<(), ConfigError> {
    let contents = std::fs::read_to_string(path)?;
    merge_toml(config, &contents)
}
