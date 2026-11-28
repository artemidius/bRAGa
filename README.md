# bRAGa

**A free, open-source RAG library — out of the box, cross-platform, written in Rust.**

Point it at a folder. bRAGa indexes your files, embeds them, and stores everything in one encrypted
database. Ask a question; get the most relevant chunks back, with the source file and line numbers.

No server. No plumbing. No rebuilding RAG for every project.

## Quick start

```sh
braga init     # create config + encryption key
braga scan     # index your folder
braga query "how do I reset my password?"
```

Sensitive data? Turn on the [`braga-extra`](#extra-security) layer.

## Install

<details>
<summary><b>CLI (Rust)</b></summary>

Requires Rust 1.89+.

```sh
git clone https://github.com/artemidius/bRAGa.git
cd bRAGa
cargo install --path crates/braga-cli
```

</details>

<details>
<summary><b>Python</b></summary>

Requires Python 3.10+ and Rust 1.89+.

```sh
git clone https://github.com/artemidius/bRAGa.git
cd bRAGa
cargo build --release -p braga-ffi
pip install ./python
```

</details>

<details>
<summary><b>Rust library (from source)</b></summary>

```toml
# Cargo.toml
[dependencies]
braga = { path = "crates/braga" }
```

</details>

## Bring your own model

Works with any **OpenAI-compatible** API or **Ollama** — local models welcome. The chunks that get
embedded never leave your machine unless you point it at a remote provider.

## Encrypted by default

The index is encrypted at rest; the key lives in `~/.braga/key` with `chmod 600`. Nothing phones
home, and file contents or keys are never logged. Details: [`SECURITY.md`](SECURITY.md) ·
[threat model](docs/threat-model.md).

### Extra security

For sensitive data, **`braga-extra`** adds per-chunk **XChaCha20-Poly1305** encryption with keys held
*outside* the database, in-memory search, and an opt-in keyword mode with a published leakage model.

## Platforms

- **Core + CLI:** macOS, Linux, Windows.
- **Wrappers:** Python available now; JVM, .NET, Go, and Swift on the roadmap — all on one frozen C
  API.

## License

- `braga` — **Apache-2.0**. Free to use, including in closed-source products.
- `braga-extra` — **AGPL-3.0 + commercial license**. Free for everyone; paid only if you embed it in
  a closed-source product.
