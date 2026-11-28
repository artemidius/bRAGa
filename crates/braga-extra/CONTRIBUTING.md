<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Contributing to braga-extra

`braga-extra` is the AGPL-3.0-or-later tier of bRAGa. The core `braga`, `braga-chunker`,
`braga-embedder`, and `braga-storage` crates are Apache-2.0 and live in the same workspace; the
license boundary is the crate, not the repository.

## Sign the CLA first

Every contributor must accept [`CLA.md`](CLA.md) before their first pull request is merged. This
applies to code, tests, documentation, and configuration. Without it, the contribution cannot be
relicensed under the commercial terms and the pull request will stay unmerged.

The CLA exists so the project can continue to offer `braga-extra` under both AGPL-3.0-or-later and
a commercial license. The maintainer is the single copyright holder for the original code; the
project does not accept code it cannot license both ways.

## License rules

- Every file under `crates/braga-extra/` carries
  `SPDX-License-Identifier: AGPL-3.0-or-later` (use `<!-- ... -->` for Markdown).
- Never put AGPL code, AGPL feature flags, or AGPL-only paths in an Apache-2.0 crate. An Apache
  crate must not depend on `braga-extra`.
- A new dependency needs a reason in the pull request, a compatible license, and a lockfile update.
  Prefer RustCrypto crates and the standard library.
- Do not describe the crate as providing search over ciphertext or approximate nearest-neighbour
  search over ciphertext. It does not. The honest boundary is decrypt-then-score vector search plus
  the opt-in blind index, whose leakage table lives in [`LEAKAGE.md`](LEAKAGE.md).

## Build and checks

From the repository root, with Rust 1.89 or newer:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
cargo doc --no-deps
```

Run `reuse lint` when it is installed. Otherwise check that each new file has an SPDX header and
that `.reuse/dep5` covers the file type.

## Code conventions

- Edition 2024, `max_width = 100`, `use_small_heuristics = "Max"`, and crate-level imports.
- Public items need rustdoc; `missing_docs` is a warning that CI treats as a failure.
- Keep a module around 250 lines of code or less. Move large test suites to `tests.rs` or `tests/`.
- Keep diffs minimal and match the surrounding style. One responsibility per module.

## Security reports

Do not open a public issue for a vulnerability. Follow the repository's [`SECURITY.md`](../../SECURITY.md).
