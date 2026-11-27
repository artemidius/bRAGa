// SPDX-License-Identifier: Apache-2.0

//! Schema v1 for the encrypted `bRAGa` database.
//!
//! [`V1`] is a single, ordered batch of DDL statements that the migration runner executes inside
//! one transaction, *after* the `SQLCipher` key has been applied. Every statement therefore runs
//! on the encrypted connection.
//!
//! ## Design decisions
//!
//! * `meta` is a key/value table rather than a scalar, because it must later hold the key-rotation
//!   generation and the crypto format version next to the schema version.
//! * `files` holds one row per scanned source file; `version` is bumped on every detected change.
//! * `chunks.id` is the content hash (content-addressed identity for dedup and the embedding
//!   cache). `chunks` keeps `SQLite`'s implicit integer `rowid` because FTS5 external-content
//!   tables link on an integer rowid and cannot link on the `BLOB` id.
//! * `chunks_fts` is external-content FTS5 over `chunks.content`, so the text is not stored twice.
//! * `model_registry` is the single source of truth for vector shape and metric.
//!
//! ## Why there is no `embeddings` table in v1
//!
//! `sqlite-vec`'s `vec0` fixes the vector width in the virtual-table DDL; it cannot vary per row,
//! and `vec0` is pre-v1. `embeddings` is therefore a *family* of per-dimension virtual tables,
//! created by Stage 6 from `model_registry`; a new dimension is a data migration, not a schema
//! change. The documented template is:
//!
//! ```sql
//! CREATE VIRTUAL TABLE embeddings_d<dim> USING vec0(
//!     embedding FLOAT[<dim>],
//!     +chunk_id BLOB,    -- = chunks.id (content hash)
//!     +model TEXT,
//!     +dim INTEGER
//! );
//! ```
//!
//! The `vec0` implicit integer `rowid` is set equal to `chunks.rowid`, so an embedding joins
//! straight back to its chunk; `+chunk_id` duplicates the content hash for the cache/dedup join.
//! No dimension exists yet, so no table is invented in v1.

/// Version stamped into `meta.schema_version` by the latest migration.
pub(crate) const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Schema version 1, including the `migrations` bookkeeping table the runner records into.
pub(crate) const V1: &str = r"
-- Single-row product metadata. Key/value so it can later hold the key-rotation generation and
-- the crypto format version alongside the schema version.
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

-- Which numbered migration ran, and when.
CREATE TABLE migrations (
    version    INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    applied_at TEXT    NOT NULL
) STRICT;

-- One row per scanned source file.
CREATE TABLE files (
    id           INTEGER PRIMARY KEY,
    path         TEXT    NOT NULL UNIQUE,
    content_hash BLOB    NOT NULL,
    version      INTEGER NOT NULL DEFAULT 1,
    updated_at   INTEGER NOT NULL
) STRICT;

-- Chunk id = content hash. `chunks` keeps SQLite's implicit integer `rowid` because FTS5
-- external-content tables require an integer rowid and cannot link on the BLOB id.
CREATE TABLE chunks (
    id          BLOB    PRIMARY KEY,
    file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    byte_offset INTEGER NOT NULL,
    line_start  INTEGER NOT NULL,
    line_end    INTEGER NOT NULL,
    content     TEXT    NOT NULL
) STRICT;

CREATE INDEX chunks_file ON chunks(file_id);

-- External-content FTS5 over chunk text (no duplicated text column).
CREATE VIRTUAL TABLE chunks_fts USING fts5(
    content,
    content='chunks',
    content_rowid='rowid'
);

-- Source of truth for vector shape. `metric` is the single per-collection metric.
CREATE TABLE model_registry (
    model  TEXT PRIMARY KEY,
    dim    INTEGER NOT NULL CHECK (dim > 0),
    metric TEXT    NOT NULL
) STRICT;

INSERT INTO meta (key, value) VALUES ('schema_version', '1');
";
