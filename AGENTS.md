# AGENTS.md

Instructions for agents working in this repository.

## Commands

```sh
cargo build
cargo test
cargo fmt --check   # cargo fmt to apply
cargo clippy --all-targets -- -D warnings
cargo doc --no-deps
```

Rust is installed via rustup; run `source ~/.cargo/env` first if `cargo` is not on `PATH`.

## Conventions

- Edition 2024; minimum supported Rust version is 1.89.
- rustfmt: `max_width = 100`, `use_small_heuristics = "Max"`, `imports_granularity = "Crate"`.
- Every source and config file carries an `SPDX-License-Identifier: Apache-2.0` header.
- Apache-2.0 only. Never add AGPL code or feature-gated AGPL paths.
- Public API carries rustdoc; `missing_docs` is a lint failure.
- Keep dependencies minimal and justify each new crate.
- CI runs fmt, clippy (`-D warnings`), tests, and docs on every push and pull request.
