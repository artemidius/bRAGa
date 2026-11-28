// SPDX-License-Identifier: Apache-2.0

//! `braga_init`: write the default `braga.toml` and a fresh key file.
//!
//! Both writes use `create_new`, mirroring the CLI's `init`: an existing file is an error rather
//! than a silent overwrite. The key path resolves like the CLI's `--key-file`, then
//! `BRAGA_KEY_FILE`, then `~/.braga/key`; the configuration defaults to `braga.toml` in the current
//! directory.

use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use braga::config::Config;
use braga_storage::Key;

use crate::{
    abi::BragaResult,
    guard::{self, Failure},
    string,
};

/// The default configuration file written by `braga_init`.
const CONFIG_FILE: &str = "braga.toml";

/// Environment variable that overrides the key file location.
const KEY_FILE_ENV: &str = "BRAGA_KEY_FILE";

/// Writes the default configuration and a fresh 32-byte key file.
///
/// A null `config_path` writes `braga.toml` in the current directory; a null `key_path` follows the
/// environment and home-directory rules above. Both paths must be valid UTF-8 and neither target may
/// already exist.
///
/// # Safety
///
/// Each non-null pointer must be valid for reads of its length and remain so for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn braga_init(
    config_path: *const u8,
    config_path_len: usize,
    key_path: *const u8,
    key_path_len: usize,
) -> BragaResult {
    guard::guard(|| {
        // SAFETY: the caller guarantees each non-null pointer is readable for its length.
        let config_path =
            unsafe { string::optional_path(config_path, config_path_len, "config_path") }?;
        // SAFETY: as above.
        let key_path = unsafe { string::optional_path(key_path, key_path_len, "key_path") }?;

        let config_path = config_path.unwrap_or_else(|| Path::new(CONFIG_FILE));
        let key_path = resolve_key_path(key_path)?;
        let rendered = Config::default_toml()?;
        write_new(config_path, rendered.as_bytes())?;
        Key::generate().to_file(&key_path)?;
        Ok(())
    })
}

/// Writes `bytes` to `path`, refusing to replace an existing file.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(path).map_err(|source| {
        if source.kind() == io::ErrorKind::AlreadyExists {
            Failure::new(BragaResult::Io, format!("config file {}: already exists", path.display()))
        } else {
            Failure::from(source)
        }
    })?;
    file.write_all(bytes).map_err(Failure::from)
}

/// Resolves the key file path: explicit, then `BRAGA_KEY_FILE`, then `~/.braga/key`.
fn resolve_key_path(explicit: Option<&Path>) -> Result<PathBuf, Failure> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os(KEY_FILE_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty());
    home.map_or_else(
        || {
            Err(Failure::new(
                BragaResult::Key,
                "cannot locate the default key file: neither HOME nor USERPROFILE is set",
            ))
        },
        |home| Ok(PathBuf::from(home).join(".braga").join("key")),
    )
}
