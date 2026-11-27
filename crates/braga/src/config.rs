// SPDX-License-Identifier: Apache-2.0

//! Configuration model for `bRAGa`.
//!
//! Configuration is resolved from up to five layers, lowest precedence first:
//!
//! 1. the built-in defaults ([`Config::default`]), which are also what [`Config::default_toml`]
//!    emits and what the future `braga init` will write,
//! 2. `BRAGA_*__*` leaf overrides over those defaults,
//! 3. the file named by `BRAGA_TOML`,
//! 4. the inline `TOML` document in `BRAGA_CONFIG`,
//! 5. the explicit path passed to [`Config::resolve`].
//!
//! Layers merge per field: a layer only contributes the fields it names, so a `BRAGA_TOML` file
//! that sets only `[embedding]` keeps `scan` and `storage` from the lower layers. Unknown `TOML`
//! keys and unknown or malformed `BRAGA_*` variables are errors; nothing is silently ignored.
//! [`Config::resolve`] documents the environment namespace in full.

use std::path::{Path, PathBuf};

/// Prefix of every environment variable that participates in configuration resolution.
const ENV_PREFIX: &str = "BRAGA_";

/// Top-level `bRAGa` configuration.
///
/// See the [module documentation](self) for how the layers are merged.
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
fn resolve_from_env(
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

/// File-scanning settings (`[scan]`).
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scan {
    /// Directory that is scanned for documents.
    pub dir: PathBuf,
}

impl Default for Scan {
    fn default() -> Self {
        Self { dir: PathBuf::from(".") }
    }
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
fn default_storage_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty());
    match home {
        Some(home) => PathBuf::from(home).join(".braga").join("index.db"),
        None => PathBuf::from("braga.sqlite3"),
    }
}

/// A configuration layer with every field optional, so only the named fields are applied.
///
/// Every struct rejects unknown keys, so a typo in any layer fails closed just like it does in the
/// fully typed schema.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PartialConfig {
    /// Partial `[scan]` table.
    scan: PartialScan,
    /// Partial `[embedding]` table.
    embedding: PartialEmbedding,
    /// Partial `[storage]` table.
    storage: PartialStorage,
}

impl PartialConfig {
    /// Folds this layer over `config`, leaving unnamed fields untouched.
    fn apply_to(self, config: &mut Config) {
        if let Some(dir) = self.scan.dir {
            config.scan.dir = dir;
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

/// Parses one `SECTION__FIELD` variable into the leaf layer.
fn apply_env_leaf(
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
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    /// A complete, valid configuration document.
    const FULL: &str = r#"
        [scan]
        dir = "/tmp/docs"

        [embedding]
        provider = "ollama"
        url = "http://localhost:11434"
        model = "nomic-embed-text"

        [storage]
        path = "/tmp/braga.sqlite3"
    "#;

    /// Resolves through the pure pipeline with a synthetic, borrow-friendly environment.
    fn resolve_env(env: &[(&str, &str)], explicit: Option<&Path>) -> Result<Config, ConfigError> {
        resolve_from_env(
            explicit,
            env.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        )
    }

    /// Writes `contents` to a unique file under the system temp directory.
    fn write_temp_config(name: &str, contents: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock").as_nanos();
        let path = std::env::temp_dir()
            .join(format!("braga-config-{name}-{}-{nanos}.toml", std::process::id()));
        std::fs::write(&path, contents).expect("temp config should be writable");
        path
    }

    #[test]
    fn defaults_are_the_documented_values() {
        let config = Config::default();
        assert_eq!(config.scan, Scan { dir: PathBuf::from(".") });
        assert_eq!(config.embedding, Embedding::default());
        assert_eq!(config.embedding.provider, "");
        assert_eq!(config.embedding.url, "");
        assert_eq!(config.embedding.model, "");
        assert_eq!(config.storage.path, default_storage_path());

        let storage = default_storage_path();
        assert!(
            storage.ends_with(".braga/index.db") || storage == Path::new("braga.sqlite3"),
            "unexpected default storage path: {storage:?}"
        );
    }

    /// One row of the environment table: the injected environment and the mutation it expects.
    struct EnvCase {
        /// Row name used in assertion messages.
        name: &'static str,
        /// Synthetic environment for the row.
        env: &'static [(&'static str, &'static str)],
        /// Expected delta over [`Config::default`].
        mutate: fn(&mut Config),
    }

    #[test]
    fn defaults_and_leaf_overrides() {
        let cases = [
            EnvCase { name: "built-in defaults", env: &[], mutate: |_| {} },
            EnvCase {
                name: "non-BRAGA variables are ignored",
                env: &[("PATH", "/usr/bin"), ("RUST_LOG", "debug")],
                mutate: |_| {},
            },
            EnvCase {
                name: "scan.dir",
                env: &[("BRAGA_SCAN__DIR", "/srv/docs")],
                mutate: |config| config.scan.dir = PathBuf::from("/srv/docs"),
            },
            EnvCase {
                name: "embedding.provider",
                env: &[("BRAGA_EMBEDDING__PROVIDER", "ollama")],
                mutate: |config| config.embedding.provider = "ollama".to_owned(),
            },
            EnvCase {
                name: "embedding.url",
                env: &[("BRAGA_EMBEDDING__URL", "http://localhost:11434")],
                mutate: |config| config.embedding.url = "http://localhost:11434".to_owned(),
            },
            EnvCase {
                name: "embedding.model",
                env: &[("BRAGA_EMBEDDING__MODEL", "nomic-embed-text")],
                mutate: |config| config.embedding.model = "nomic-embed-text".to_owned(),
            },
            EnvCase {
                name: "storage.path",
                env: &[("BRAGA_STORAGE__PATH", "/srv/braga.db")],
                mutate: |config| config.storage.path = PathBuf::from("/srv/braga.db"),
            },
        ];

        let defaults = Config::default();
        for EnvCase { name, env, mutate } in cases {
            let mut expected = defaults.clone();
            mutate(&mut expected);
            let resolved = resolve_env(env, None).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(resolved, expected, "{name}");
        }
    }

    #[test]
    fn full_precedence_cross_product() {
        // Each field pins one precedence edge; no field is named by every layer.
        let file = write_temp_config(
            "precedence-file",
            r#"
                [scan]
                dir = "/file-dir"

                [embedding]
                provider = "file-provider"
                model = "file-model"
            "#,
        );
        let explicit =
            write_temp_config("precedence-explicit", "[embedding]\nmodel = \"explicit-model\"\n");
        let inline = "[embedding]\nprovider = \"inline-provider\"\n";
        let env = vec![
            ("BRAGA_SCAN__DIR".to_owned(), "/leaf-dir".to_owned()),
            ("BRAGA_EMBEDDING__PROVIDER".to_owned(), "leaf-provider".to_owned()),
            ("BRAGA_EMBEDDING__URL".to_owned(), "leaf-url".to_owned()),
            ("BRAGA_TOML".to_owned(), file.display().to_string()),
            ("BRAGA_CONFIG".to_owned(), inline.to_owned()),
        ];

        let resolved = resolve_from_env(Some(&explicit), env.into_iter());
        for path in [&file, &explicit] {
            std::fs::remove_file(path).expect("temp config should be removable");
        }

        let mut expected = Config::default();
        expected.scan.dir = PathBuf::from("/file-dir"); // BRAGA_TOML > leaf > default
        expected.embedding.provider = "inline-provider".to_owned(); // BRAGA_CONFIG > file > leaf
        expected.embedding.url = "leaf-url".to_owned(); // leaf wins; higher layers are silent
        expected.embedding.model = "explicit-model".to_owned(); // explicit > file > default
        assert_eq!(resolved.expect("layers should resolve"), expected);
    }

    #[test]
    fn explicit_path_beats_inline_config() {
        let explicit =
            write_temp_config("explicit-wins", "[embedding]\nmodel = \"explicit-model\"\n");
        let env =
            vec![("BRAGA_CONFIG".to_owned(), "[embedding]\nmodel = \"inline-model\"\n".to_owned())];

        let resolved = resolve_from_env(Some(&explicit), env.into_iter());
        std::fs::remove_file(&explicit).expect("temp config should be removable");

        let mut expected = Config::default();
        expected.embedding.model = "explicit-model".to_owned();
        assert_eq!(resolved.expect("layers should resolve"), expected);
    }

    #[test]
    fn toml_layers_merge_per_field() {
        let file = write_temp_config(
            "merge",
            "[scan]\ndir = \"/from-file\"\n\n[embedding]\nprovider = \"file-provider\"\n",
        );
        let inline = "[embedding]\nurl = \"http://inline:11434\"\n";
        let env = vec![
            ("BRAGA_SCAN__DIR".to_owned(), "/from-leaf".to_owned()),
            ("BRAGA_EMBEDDING__MODEL".to_owned(), "leaf-model".to_owned()),
            ("BRAGA_TOML".to_owned(), file.display().to_string()),
            ("BRAGA_CONFIG".to_owned(), inline.to_owned()),
        ];

        let resolved = resolve_from_env(None, env.into_iter());
        std::fs::remove_file(&file).expect("temp config should be removable");

        let mut expected = Config::default();
        expected.scan.dir = PathBuf::from("/from-file"); // the file beats the leaf
        expected.embedding.provider = "file-provider".to_owned();
        expected.embedding.url = "http://inline:11434".to_owned(); // inline beats the file
        expected.embedding.model = "leaf-model".to_owned(); // falls through the higher layers
        assert_eq!(resolved.expect("layers should resolve"), expected);
    }

    #[test]
    fn parses_full_config() {
        let config = Config::from_toml_str(FULL).expect("full config should parse");
        assert_eq!(config.scan.dir, PathBuf::from("/tmp/docs"));
        assert_eq!(config.embedding.provider, "ollama");
        assert_eq!(config.embedding.url, "http://localhost:11434");
        assert_eq!(config.embedding.model, "nomic-embed-text");
        assert_eq!(config.storage.path, PathBuf::from("/tmp/braga.sqlite3"));
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config =
            Config::from_toml_str("[scan]\ndir = \"/tmp/docs\"").expect("partial config parses");
        assert_eq!(config.scan.dir, PathBuf::from("/tmp/docs"));
        assert_eq!(config.embedding, Embedding::default());
        assert_eq!(config.storage, Storage::default());
    }

    #[test]
    fn unknown_keys_fail_closed_in_every_layer() {
        let cases = [
            ("top level", "bogus = true"),
            ("scan", "[scan]\nbogus = true"),
            ("embedding", "[embedding]\nbogus = true"),
            ("storage", "[storage]\nbogus = true"),
        ];

        for (level, contents) in cases {
            let parsed = Config::from_toml_str(contents);
            assert!(
                matches!(parsed, Err(ConfigError::Toml(_))),
                "from_toml_str must reject an unknown key in {level}"
            );

            let inline = resolve_env(&[("BRAGA_CONFIG", contents)], None);
            assert!(
                matches!(inline, Err(ConfigError::Toml(_))),
                "BRAGA_CONFIG must reject an unknown key in {level}"
            );

            let path = write_temp_config(level, contents);
            let file_value = path.display().to_string();
            let from_file = resolve_env(&[("BRAGA_TOML", &file_value)], None);
            std::fs::remove_file(&path).expect("temp config should be removable");
            assert!(
                matches!(from_file, Err(ConfigError::Toml(_))),
                "BRAGA_TOML must reject an unknown key in {level}"
            );
        }
    }

    #[test]
    fn unrecognized_environment_variables_fail_closed() {
        type Matcher = fn(&ConfigError) -> bool;
        let cases: [(&str, Matcher); 6] = [
            ("BRAGA_BOGUS", |error| matches!(error, ConfigError::UnknownEnvVar(_))),
            ("BRAGA_", |error| matches!(error, ConfigError::UnknownEnvVar(_))),
            ("BRAGA_EMBEDDING", |error| matches!(error, ConfigError::InvalidEnvVar(_))),
            ("BRAGA_A__B__C", |error| matches!(error, ConfigError::InvalidEnvVar(_))),
            ("BRAGA_EMBEDDING__NOPE", |error| matches!(error, ConfigError::InvalidEnvVar(_))),
            ("BRAGA_scan__dir", |error| matches!(error, ConfigError::InvalidEnvVar(_))),
        ];

        for (key, is_expected) in cases {
            let result = resolve_env(&[(key, "value")], None);
            let Err(error) = result else {
                panic!("{key} must fail closed");
            };
            assert!(is_expected(&error), "{key}: unexpected error: {error}");
        }
    }

    #[test]
    fn default_toml_round_trips() {
        let rendered = Config::default_toml().expect("defaults should serialize");
        for table in ["[scan]", "[embedding]", "[storage]"] {
            assert!(rendered.contains(table), "default TOML is missing {table}:\n{rendered}");
        }
        assert!(rendered.contains("dir = \".\""), "scan.dir should serialize as '.':\n{rendered}");
        assert_eq!(
            Config::from_toml_str(&rendered).expect("default TOML should parse"),
            Config::default()
        );
    }

    #[test]
    fn load_round_trips_a_file() {
        let path = write_temp_config("load", FULL);
        let loaded = Config::load(&path);
        std::fs::remove_file(&path).expect("temp config should be removable");

        let expected = Config::from_toml_str(FULL).expect("full config should parse");
        assert_eq!(loaded.expect("config should load"), expected);
    }
}
