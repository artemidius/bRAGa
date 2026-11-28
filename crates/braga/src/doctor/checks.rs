// SPDX-License-Identifier: Apache-2.0

//! The `doctor` driver and the non-destructive checks.

use braga_embedder::Embedder;
use braga_storage::{Key, Store};

use crate::config::Config;
use crate::embedding::build_embedder;

use super::scratch::scratch_checks;
use super::{
    CONFIG_VALID, Check, DoctorError, DoctorReport, EMBED_LIVE, LIVE_ENV, LIVE_PROBE, LIVE_SKIPPED,
    STORE_OPEN,
};

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
