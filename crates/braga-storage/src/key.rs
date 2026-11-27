// SPDX-License-Identifier: Apache-2.0

//! Raw 256-bit database keys and their on-disk files.

use std::{
    fmt, fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use thiserror::Error;

/// Environment variable that overrides the key file location.
const KEY_FILE_ENV: &str = "BRAGA_KEY_FILE";

/// Errors from reading, writing, or locating a [`Key`] file.
///
/// Messages carry a path and a fixed reason only: no key byte is ever formatted.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KeyFileError {
    /// The key file or its parent directory could not be accessed.
    #[error("key file {}: {source}", .path.display())]
    Io {
        /// Path of the offending file or directory.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// The key file did not contain exactly 32 bytes.
    #[error("key file {}: expected exactly 32 bytes, found {found}", .path.display())]
    InvalidLength {
        /// Path of the offending file.
        path: PathBuf,
        /// Number of bytes found.
        found: usize,
    },
    /// A key file already exists at the target path.
    #[error("key file {}: already exists", .path.display())]
    AlreadyExists {
        /// Path of the existing file.
        path: PathBuf,
    },
    /// No home directory could be determined for the default key location.
    #[error("cannot locate the default key file: neither HOME nor USERPROFILE is set")]
    NoHomeDirectory,
}

/// A raw 256-bit `SQLCipher` key.
///
/// `Debug` deliberately redacts the bytes so a key can never leak through logging. There is no
/// `Display` implementation, and none should be added.
#[derive(Clone, PartialEq, Eq)]
pub struct Key([u8; 32]);

impl Key {
    /// Creates a key from exactly 32 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Generates a fresh 256-bit key from the operating system `CSPRNG`.
    ///
    /// # Panics
    ///
    /// Panics if the operating system `CSPRNG` is unavailable. On the desktop platforms this crate
    /// targets, that indicates a broken system rather than a recoverable condition.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).expect("operating system CSPRNG must be available");
        Self(bytes)
    }

    /// Borrows the raw key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Reads a key from `path`, which must contain exactly 32 bytes.
    ///
    /// # Errors
    ///
    /// Returns [`KeyFileError::Io`] if the file cannot be read and
    /// [`KeyFileError::InvalidLength`] for any length other than exactly 32 bytes. Neither error
    /// includes the file's contents.
    pub fn from_file(path: &Path) -> Result<Self, KeyFileError> {
        let contents = fs::read(path)
            .map_err(|source| KeyFileError::Io { path: path.to_path_buf(), source })?;
        let found = contents.len();
        let bytes: [u8; 32] = contents
            .try_into()
            .map_err(|_| KeyFileError::InvalidLength { path: path.to_path_buf(), found })?;
        Ok(Self(bytes))
    }

    /// Writes this key to `path` without ever clobbering an existing file.
    ///
    /// On Unix the file is created with mode `0o600` and a missing parent directory is created
    /// with mode `0o700`. On Windows the restrictive ACL work is deferred and documented as
    /// best-effort: the file is created with the default ACL, so the caller is responsible for
    /// tightening it.
    ///
    /// # Errors
    ///
    /// Returns [`KeyFileError::AlreadyExists`] if `path` already exists, and [`KeyFileError::Io`]
    /// if the parent directory or the file cannot be created or written.
    pub fn to_file(&self, path: &Path) -> Result<(), KeyFileError> {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            create_parent_dir(parent)?;
        }

        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }

        let mut file = options.open(path).map_err(|source| open_error(path, source))?;
        file.write_all(self.as_bytes())
            .map_err(|source| KeyFileError::Io { path: path.to_path_buf(), source })?;
        Ok(())
    }

    /// Loads the unattended key, highest precedence first, from `BRAGA_KEY_FILE`, the
    /// caller-supplied `override_path`, then `~/.braga/key`.
    ///
    /// # Errors
    ///
    /// Returns [`KeyFileError::NoHomeDirectory`] when no path is configured and neither `HOME` nor
    /// `USERPROFILE` is set, and the [`Key::from_file`] errors otherwise.
    pub fn load(override_path: Option<&Path>) -> Result<Self, KeyFileError> {
        let path = resolve_key_path(override_path, std::env::vars())?;
        Self::from_file(&path)
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key([REDACTED])")
    }
}

/// The pure path-resolution pipeline behind [`Key::load`].
///
/// The environment is injected so that precedence can be tested without mutating — and racing
/// on — the real process environment.
fn resolve_key_path(
    explicit: Option<&Path>,
    env: impl Iterator<Item = (String, String)>,
) -> Result<PathBuf, KeyFileError> {
    let mut override_path = None;
    let mut home = None;
    for (name, value) in env {
        match name.as_str() {
            KEY_FILE_ENV => override_path = Some(PathBuf::from(value)),
            "HOME" => {
                home.get_or_insert_with(|| PathBuf::from(value));
            }
            #[cfg(windows)]
            "USERPROFILE" => {
                home.get_or_insert_with(|| PathBuf::from(value));
            }
            _ => {}
        }
    }

    if let Some(path) = override_path {
        return Ok(path);
    }
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    home.map_or(Err(KeyFileError::NoHomeDirectory), |home| Ok(home.join(".braga").join("key")))
}

/// Creates `path` as a directory that is private to the current user on Unix.
fn create_parent_dir(path: &Path) -> Result<(), KeyFileError> {
    #[cfg(unix)]
    let result = {
        use std::os::unix::fs::DirBuilderExt as _;

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path)
    };
    #[cfg(not(unix))]
    let result = fs::create_dir_all(path);

    result.map_err(|source| KeyFileError::Io { path: path.to_path_buf(), source })
}

/// Maps an `OpenOptions::open` failure, preserving the "exists" reason for [`KeyFileError`].
fn open_error(path: &Path, source: io::Error) -> KeyFileError {
    if source.kind() == io::ErrorKind::AlreadyExists {
        KeyFileError::AlreadyExists { path: path.to_path_buf() }
    } else {
        KeyFileError::Io { path: path.to_path_buf(), source }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{KeyFileError, resolve_key_path};

    fn env(pairs: &[(&str, &str)]) -> impl Iterator<Item = (String, String)> {
        let owned: Vec<(String, String)> =
            pairs.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect();
        owned.into_iter()
    }

    #[test]
    fn braga_key_file_has_highest_precedence() {
        let path = resolve_key_path(
            Some(Path::new("/tmp/explicit")),
            env(&[("BRAGA_KEY_FILE", "/tmp/from-env"), ("HOME", "/home/user")]),
        )
        .expect("resolve key path");
        assert_eq!(path, PathBuf::from("/tmp/from-env"));
    }

    #[test]
    fn explicit_path_beats_the_home_directory() {
        let path =
            resolve_key_path(Some(Path::new("/tmp/explicit")), env(&[("HOME", "/home/user")]))
                .expect("resolve key path");
        assert_eq!(path, PathBuf::from("/tmp/explicit"));
    }

    #[test]
    fn falls_back_to_the_home_directory() {
        let path = resolve_key_path(None, env(&[("HOME", "/home/user")])).expect("resolve");
        assert_eq!(path, PathBuf::from("/home/user/.braga/key"));
    }

    #[test]
    fn missing_home_directory_is_an_error() {
        let error = resolve_key_path(None, env(&[])).expect_err("no home directory");
        assert!(matches!(error, KeyFileError::NoHomeDirectory));
    }
}
