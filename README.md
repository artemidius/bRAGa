# bRAGa

Local-first, encrypted **retrieval toolkit for RAG pipelines**. `bRAGa` builds, maintains, and
queries a retrieval index over your files; it never generates text.

> **Status: early development. Not audited.** See [`SECURITY.md`](SECURITY.md) for the security
> policy and [`docs/threat-model.md`](docs/threat-model.md) for what base-mode encryption does and
> does not protect.

## Build and test

Requires Rust 1.89 or newer (edition 2024).

```sh
cargo build
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo doc --no-deps
```
