# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Stage 1 bootstrap: Cargo workspace, `braga` crate skeleton, and a `config` module that loads
  TOML and rejects unknown keys.
- CI (fmt, clippy, test, doc), rustfmt/lint configuration, and license hygiene (`NOTICE`,
  `SECURITY.md`, REUSE `dep5`).
- Stage 2 config: layered TOML and environment resolution (`BRAGA_TOML`, `BRAGA_CONFIG`,
  `BRAGA_*__*`) with fail-closed unknown keys.
- Stage 3 storage: SQLCipher-encrypted SQLite store with `sqlite-vec` and FTS5, WAL, and schema
  migrations.
- Stage 4 keys: 32-byte keyfile management (`~/.braga/key`, mode `0600`) and schema v1.
- Stage 5 chunker: line-aware chunking with content hashes.
- Stage 6 scanner/status: directory reconcile and the status engine.
- Stage 7a embedder: `Embedder` trait with OpenAI-compatible and Ollama HTTP adapters.
- Stage 7b retrieval: embedding cache, `vec0` vector search, FTS5 keyword search, and RRF fusion.
- Stage 7c doctor: six-check claims verifier with machine-readable JSON output.
- Stage 8 CLI: `braga init|scan|query|doctor` and off-by-default `tracing` telemetry.
