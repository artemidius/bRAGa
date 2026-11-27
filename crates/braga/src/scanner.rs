// SPDX-License-Identifier: Apache-2.0

//! The synchronous reconcile pipeline: walk, hash, chunk, and replace-and-GC per file.
//!
//! [`Scanner::reconcile`] makes the encrypted index match the configured scan directory. It walks
//! the tree without following symlinks, hashes every regular file with
//! [`braga_chunker::content_hash`], re-chunks the files whose hash changed, and removes the rows of
//! the files that are gone. Each file is one [`Store`] transaction, so a failure part-way through
//! leaves a consistent index that the next run converges.
//!
//! # Skip policy
//!
//! A file is indexable iff its path is valid `UTF-8` and its bytes are valid `UTF-8`. Non-`UTF-8`
//! paths, non-`UTF-8` (binary) content, symlinks, and sockets/FIFOs/device nodes are skipped and
//! counted in [`ScanOutcome::skipped`]: they are policy, not errors. Because of the `UTF-8` rule, a
//! file that was indexed as text and later becomes binary is garbage-collected like a deletion,
//! while a valid-`UTF-8` file containing NUL bytes is indexed (a documented v1 limitation).
//!
//! # Fail-closed I/O
//!
//! An unreadable root, an unreadable subdirectory, or a failed [`std::fs::read`] returns
//! [`ScanError`]: the scanner never claims to have seen content it could not read. A file that
//! vanishes between `read_dir` and `read` surfaces as [`ScanError::Io`], and the next idempotent
//! run converges.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use braga_chunker::{ByteTokenizer, Chunk, Chunker, content_hash};
use braga_storage::{FileRow, NewChunk, Store, StoreError};
use thiserror::Error;

/// What one [`Scanner::reconcile`] changed.
///
/// On a no-change rescan every stored hash matches the file on disk, so nothing is written and
/// every counter but [`unchanged`](Self::unchanged) is zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanOutcome {
    /// New files indexed.
    pub added: usize,
    /// Files whose content hash changed; their chunks were replaced and the old ones GC'd.
    pub modified: usize,
    /// Files whose content hash matched; nothing was written for them.
    pub unchanged: usize,
    /// Stored files that are gone, or no longer indexable; their chunks were GC'd.
    pub deleted: usize,
    /// Entries skipped by policy: binary content, symlink, or non-`UTF-8` path.
    pub skipped: usize,
}

/// Error returned by [`Scanner::reconcile`].
///
/// It carries no file content and no key material: I/O errors are [`std::io::Error`] and storage
/// errors are the numeric-code-only [`StoreError`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ScanError {
    /// A directory or file could not be read; reconcile fails closed rather than claiming success.
    #[error("scan I/O error: {0}")]
    Io(#[from] io::Error),
    /// A storage operation failed.
    #[error("storage: {0}")]
    Store(#[from] StoreError),
}

/// Reconciles one directory tree into an encrypted [`Store`].
///
/// The scanner owns the store and a default [`Chunker`] (800 target tokens, 100 overlap,
/// [`ByteTokenizer`]). Chunker sizing is not configurable yet; a later stage introduces
/// provider-driven sizing behind the same constructor.
pub struct Scanner {
    dir: PathBuf,
    store: Store,
    chunker: Chunker<ByteTokenizer>,
}

impl Scanner {
    /// Builds a scanner that reconciles `dir` into `store`.
    #[must_use]
    pub fn new(dir: PathBuf, store: Store) -> Self {
        Self { dir, store, chunker: Chunker::new() }
    }

    /// Brings `store` in line with the directory tree under the scanner's root.
    ///
    /// Reconcile is content-driven and idempotent: when nothing changed, neither
    /// [`Store::sync_file`] nor [`Store::delete_file`] is called and the database is untouched.
    /// It is deliberately not one transaction; each add, modify, and delete commits separately,
    /// so the index stays consistent at every intermediate point.
    ///
    /// # Errors
    ///
    /// Returns [`ScanError::Io`] when the tree cannot be read and [`ScanError::Store`] when a
    /// storage operation fails.
    pub fn reconcile(&self) -> Result<ScanOutcome, ScanError> {
        let rows = self.store.list_files()?;
        let stored: HashMap<String, FileRow> =
            rows.into_iter().map(|row| (row.path.clone(), row)).collect();
        let mut seen = HashSet::new();
        let mut outcome = ScanOutcome::default();

        self.walk(&self.dir, &stored, &mut seen, &mut outcome)?;

        // Every stored path absent from the indexable set is gone: deleted, moved, renamed, or now
        // binary. GC it exactly like a deletion.
        for (path, row) in &stored {
            if !seen.contains(path.as_str()) {
                self.store.delete_file(row.id)?;
                outcome.deleted += 1;
            }
        }
        Ok(outcome)
    }

    /// Reads `dir` and recurses, skipping symlinks and non-regular entries.
    fn walk(
        &self,
        dir: &Path,
        stored: &HashMap<String, FileRow>,
        seen: &mut HashSet<String>,
        outcome: &mut ScanOutcome,
    ) -> Result<(), ScanError> {
        // Sorted per directory only for deterministic logs and tests; reconcile is content-driven,
        // so ordering is not a correctness input.
        let mut entries: Vec<fs::DirEntry> =
            fs::read_dir(dir)?.collect::<Result<_, io::Error>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);

        for entry in entries {
            // `file_type` does not follow the link, so a symlink is skipped here rather than
            // recursed into. That makes symlink cycles and re-visits structurally impossible.
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                outcome.skipped += 1;
            } else if file_type.is_dir() {
                self.walk(&entry.path(), stored, seen, outcome)?;
            } else if file_type.is_file() {
                self.reconcile_file(&entry.path(), stored, seen, outcome)?;
            } else {
                outcome.skipped += 1;
            }
        }
        Ok(())
    }

    /// Reconciles one regular file: hash, compare, and re-chunk on add or change.
    fn reconcile_file(
        &self,
        path: &Path,
        stored: &HashMap<String, FileRow>,
        seen: &mut HashSet<String>,
        outcome: &mut ScanOutcome,
    ) -> Result<(), ScanError> {
        let Some(path) = path.to_str() else {
            outcome.skipped += 1;
            return Ok(());
        };
        let bytes = fs::read(path)?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            outcome.skipped += 1;
            return Ok(());
        };
        seen.insert(path.to_owned());

        let hash = content_hash(&bytes);
        let hash_bytes: &[u8] = hash.as_bytes();
        let existing = stored.get(path);
        if existing.is_some_and(|row| row.content_hash.as_slice() == hash_bytes) {
            outcome.unchanged += 1;
            return Ok(());
        }

        let chunks = self.chunker.chunk(text);
        let new_chunks: Vec<NewChunk<'_>> = chunks.iter().map(new_chunk).collect();
        self.store.sync_file(path, hash_bytes, &new_chunks)?;
        if existing.is_some() {
            outcome.modified += 1;
        } else {
            outcome.added += 1;
        }
        Ok(())
    }
}

/// Maps a chunker [`Chunk`] onto the storage layer's borrowed row shape.
fn new_chunk(chunk: &Chunk) -> NewChunk<'_> {
    NewChunk {
        id: chunk.id.as_bytes().as_slice(),
        byte_offset: i64::try_from(chunk.byte_offset).expect("chunk byte offset exceeds i64"),
        line_start: i64::from(chunk.line_start),
        line_end: i64::from(chunk.line_end),
        content: chunk.content.as_str(),
    }
}
