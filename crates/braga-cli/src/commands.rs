// SPDX-License-Identifier: Apache-2.0

//! The four `braga` subcommand implementations.
//!
//! Each command resolves its inputs, runs the library entry point, and writes results to stdout.
//! Telemetry stays on stderr; these functions never log file content, file paths, query text, or
//! key material.

use std::{
    error::Error,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use braga::{
    config::{Config, Embedding},
    doctor,
    embedding::{EmbeddingError, EmbeddingPipeline, Metric, build_embedder},
    retrieval,
    scanner::Scanner,
};
use braga_chunker::{ContentHash, content_hash};
use braga_storage::{Key, KeyFileError, Store, StoreError};
use tracing::field;

use crate::telemetry::{InstrumentedEmbedder, elapsed_ms};

/// Default configuration file written by `init`.
const CONFIG_FILE: &str = "braga.toml";

/// Environment variable that overrides the key file location.
const KEY_FILE_ENV: &str = "BRAGA_KEY_FILE";

/// Writes the default configuration and a fresh key file.
///
/// Both writes are `create_new`: an existing file fails with an "already exists" error instead of
/// being overwritten.
///
/// # Errors
///
/// Returns an error when the configuration cannot be rendered, either target already exists, or
/// a write fails.
pub(crate) fn init(
    config_path: Option<&Path>,
    key_file: Option<&Path>,
) -> Result<ExitCode, Box<dyn Error>> {
    let config_path = config_path.unwrap_or_else(|| Path::new(CONFIG_FILE));
    let key_path = resolve_init_key_path(key_file)?;

    let rendered = Config::default_toml()?;
    write_new(config_path, rendered.as_bytes())?;
    Key::generate().to_file(&key_path)?;

    let config_display = config_path.display();
    let key_display = key_path.display();
    println!("config: {config_display}");
    println!("key: {key_display}");
    tracing::info!("init complete");
    Ok(ExitCode::SUCCESS)
}

/// Reconciles the scan directory and embeds every chunk that lacks an embedding.
///
/// Reconcile and embed are separate passes over the same store: the scanner owns its store while it
/// reconciles, so the command reopens the database to read the chunks and run the pipeline. The
/// pipeline's cache filter makes a second scan a full cache hit.
///
/// # Errors
///
/// Returns an error when the configuration or key cannot be loaded, the scan directory cannot be
/// read, the store cannot be opened, or the embedding configuration is unusable or the provider
/// fails.
pub(crate) fn scan(
    config_path: Option<&Path>,
    key_file: Option<&Path>,
) -> Result<ExitCode, Box<dyn Error>> {
    let config = Config::resolve(config_path)?;
    let key = Key::load(key_file)?;

    let span = tracing::info_span!(
        "scan",
        added = field::Empty,
        modified = field::Empty,
        unchanged = field::Empty,
        deleted = field::Empty,
        skipped = field::Empty,
        elapsed_ms = field::Empty,
    );
    let _guard = span.enter();
    let started = Instant::now();

    let scanned = {
        let store = open_store(&config.storage.path, &key)?;
        let scanner = Scanner::new(config.scan.dir.clone(), store);
        scanner.reconcile()?
    };

    let store = open_store(&config.storage.path, &key)?;
    let chunks = store.all_chunks()?;
    let pairs: Vec<(ContentHash, &str)> = chunks
        .iter()
        .map(|(_, content)| (content_hash(content.as_bytes()), content.as_str()))
        .collect();

    let embedder = instrumented_embedder(&config.embedding)?;
    let embedding_outcome =
        EmbeddingPipeline::new(Metric::Cosine).run(&store, &embedder, &pairs)?;

    span.record("added", scanned.added);
    span.record("modified", scanned.modified);
    span.record("unchanged", scanned.unchanged);
    span.record("deleted", scanned.deleted);
    span.record("skipped", scanned.skipped);
    span.record("elapsed_ms", elapsed_ms(started));
    tracing::info!("scan complete");

    let braga::scanner::ScanOutcome { added, modified, unchanged, deleted, skipped } = scanned;
    let braga::embedding::EmbeddingOutcome { cache_hits, embedded: embedded_count, model, dim } =
        embedding_outcome;
    println!("added: {added}");
    println!("modified: {modified}");
    println!("unchanged: {unchanged}");
    println!("deleted: {deleted}");
    println!("skipped: {skipped}");
    println!("embedded: {embedded_count}");
    println!("cache_hits: {cache_hits}");
    println!("model: {model}");
    println!("dim: {dim}");
    Ok(ExitCode::SUCCESS)
}

/// Queries the index and prints each hit as `path:line_start-line_end <score> <content>`.
///
/// # Errors
///
/// Returns an error when the configuration or key cannot be loaded, the embedding configuration is
/// unusable, the provider fails, or the store cannot be read.
pub(crate) fn query(
    config_path: Option<&Path>,
    key_file: Option<&Path>,
    text: &str,
    top_k: usize,
) -> Result<ExitCode, Box<dyn Error>> {
    let config = Config::resolve(config_path)?;
    let key = Key::load(key_file)?;
    let embedder = instrumented_embedder(&config.embedding)?;

    let span =
        tracing::info_span!("query", top_k = top_k, hits = field::Empty, elapsed_ms = field::Empty);
    let _guard = span.enter();
    let started = Instant::now();

    let store = open_store(&config.storage.path, &key)?;
    let hits = retrieval::query(&store, &embedder, text, top_k)?;

    span.record("hits", hits.len());
    span.record("elapsed_ms", elapsed_ms(started));
    tracing::info!("query complete");

    for hit in &hits {
        let path = &hit.path;
        let line_start = hit.line_start;
        let line_end = hit.line_end;
        let fused_score = hit.score;
        let content = &hit.content;
        println!("{path}:{line_start}-{line_end} {fused_score:.4} {content}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Runs the claims verifier, prints its JSON report, and exits non-zero unless every check passed.
///
/// # Errors
///
/// Returns an error when the configuration or key cannot be loaded, the embedding configuration is
/// unusable, or the report cannot be serialized.
pub(crate) fn doctor(
    config_path: Option<&Path>,
    key_file: Option<&Path>,
) -> Result<ExitCode, Box<dyn Error>> {
    let config = Config::resolve(config_path)?;
    let key = Key::load(key_file)?;
    let embedder = instrumented_embedder(&config.embedding)?;

    let report = doctor::doctor(&config, &key, &embedder)?;
    let json = report.to_json()?;
    println!("{json}");

    Ok(if report.ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// Builds the configured adapter and wraps it in the embed-call instrumentation.
fn instrumented_embedder(embedding: &Embedding) -> Result<InstrumentedEmbedder, EmbeddingError> {
    Ok(InstrumentedEmbedder::new(build_embedder(embedding)?))
}

/// Opens the encrypted store and emits the `store.open` telemetry span.
fn open_store(path: &Path, key: &Key) -> Result<Store, StoreError> {
    let span = tracing::info_span!("store.open", elapsed_ms = field::Empty, status = field::Empty);
    let _guard = span.enter();
    let started = Instant::now();
    let result = Store::open(path, key);
    span.record("elapsed_ms", elapsed_ms(started));
    if result.is_ok() {
        span.record("status", "ok");
        tracing::info!("store opened");
    } else {
        span.record("status", "error");
        tracing::info!("store open failed");
    }
    result
}

/// Writes `bytes` to `path`, refusing to replace an existing file.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(path).map_err(|source| {
        if source.kind() == io::ErrorKind::AlreadyExists {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("config file {}: already exists", path.display()),
            )
        } else {
            source
        }
    })?;
    file.write_all(bytes)
}

/// Resolves the key file path `init` writes: `--key-file`, then `BRAGA_KEY_FILE`, then
/// `~/.braga/key`.
fn resolve_init_key_path(explicit: Option<&Path>) -> Result<PathBuf, KeyFileError> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os(KEY_FILE_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty());
    home.map_or(Err(KeyFileError::NoHomeDirectory), |home| {
        Ok(PathBuf::from(home).join(".braga").join("key"))
    })
}
