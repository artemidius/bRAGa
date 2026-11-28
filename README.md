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

From Python (after `pip install`):

```python
import braga

with braga.open("braga.toml") as db:          # key defaults to ~/.braga/key
    db.update()                                # scan + embed missing chunks
    for hit in db.query("how do I reset my password?", top_k=5):
        print(f"{hit.path}:{hit.line_start}-{hit.line_end}  {hit.score:.3f}")
        print(hit.content[:200])
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

<details>
<summary><b>Agentic install (paste into your AI agent)</b></summary>

```
Install and set up bRAGa in this environment:

1. Install the platform prerequisites (detect the OS):
   - macOS: `xcode-select --install`, then Rust via `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`.
   - Linux (Debian/Ubuntu): `sudo apt-get install -y build-essential pkg-config libssl-dev`, then Rust via the same rustup command.
   - Windows: install Rust via rustup-init.exe and the Visual Studio Build Tools ("Desktop development with C++").
   (Rust must be 1.89+.)
2. Clone https://github.com/artemidius/bRAGa.git and `cd` into it.
3. Install the CLI: `cargo install --path crates/braga-cli`.
4. Python package (if you'll use it from Python): `cargo build --release -p braga-ffi && pip install ./python`.
5. Initialize: `braga init` (creates `braga.toml` and an encryption key at `~/.braga/key`).
6. In `braga.toml`, set `[scan] dir` to the folder to index and `[embedding]` to a working
   provider (`openai-compatible` with a base URL + model, or `ollama`).
7. Run `braga scan`, then verify with `braga doctor` (exit 0 = all checks pass).
8. Try `braga query "<a question about the folder>"`.

Stop and report any step that fails; do not invent config values or skip the `doctor` check.
```

</details>

## Platform setup

<details>
<summary><b>macOS</b></summary>

```sh
xcode-select --install   # C compiler (clang) for the SQLCipher build
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

macOS uses the built-in CommonCrypto — nothing else to install.

</details>

<details>
<summary><b>Linux (Debian / Ubuntu)</b></summary>

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

</details>

<details>
<summary><b>Windows</b></summary>

- Rust: [rustup-init.exe](https://rustup.rs)
- C compiler + linker: Visual Studio Build Tools ("Desktop development with C++")

</details>

## Bring your own model

Embeddings are pluggable — pick what fits:
- **Remote API:** any OpenAI-compatible endpoint.
- **Local server:** Ollama.
- **In-process:** embed on your own machine with no server, by implementing the `Embedder` trait
  (e.g. a `candle` model — experimental).

Chunks only leave your machine when you point them at a remote provider.

## Encrypted by default

The index is encrypted at rest; the key lives in `~/.braga/key` with `chmod 600`. Nothing phones
home, and file contents or keys are never logged. Details: [`SECURITY.md`](SECURITY.md) ·
[threat model](docs/threat-model.md).

### Extra security

For sensitive data, **`braga-extra`** adds per-chunk **XChaCha20-Poly1305** encryption with keys held
*outside* the database, in-memory search, and an opt-in keyword mode with a published leakage model.

## Platforms

- **Core + CLI:** macOS, Linux, Windows.
- **Wrappers:** Python, Go, JVM, .NET, and Swift (macOS) available now — all on one frozen C API.

## License

- `braga` — **Apache-2.0**. Free to use, including in closed-source products.
- `braga-extra` — **AGPL-3.0 + commercial license**. Free for everyone; paid only if you embed it in
  a closed-source product.
