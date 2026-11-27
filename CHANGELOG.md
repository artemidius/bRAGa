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
