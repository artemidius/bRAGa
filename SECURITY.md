# Security Policy

> **Status: early development. Not audited.** bRAGa is not audited and provides no security
> guarantees. Until a third-party audit has happened, do not rely on it as the only control for
> data you cannot afford to lose or disclose.

This policy covers the `braga` Rust workspace in this repository. The technical detail behind it —
including the exact non-goals — lives in [`docs/threat-model.md`](docs/threat-model.md).

## Supported versions

| Version | Supported |
|---|---|
| 0.1.x (latest release) | Yes — security fixes land here |
| 0.1.x (older releases) | No |
| < 0.1 | No |

bRAGa is pre-1.0. Security patches are made on the latest published 0.1.x release; older releases
do not receive backports. Building from `main` is supported for testing, but `main` is not a
release and carries no stability or security promise.

## Reporting a vulnerability

A dedicated private reporting channel is not in place yet. Until it is:

1. If this repository offers GitHub private vulnerability reporting, use **Security → Report a
   vulnerability** to open a private advisory.
2. Otherwise, open a minimal public GitHub issue that says only that you have a security report
   and asks a maintainer for a private channel. Do **not** include exploit details, proof-of-concept
   code, keys, index files, or document contents in that issue.

Please give us a chance to fix the issue before public disclosure. This project cannot currently
offer a bug bounty or a response-time guarantee.

## Encryption boundary

The encryption claim is narrow and deliberate: **base mode protects the index at rest.**

- **What it protects.** The SQLite index at `storage.path` (default `~/.braga/index.db`) is
  encrypted at the SQLCipher page/codec level, not as a whole file. Chunk text, file paths, content
  hashes, embeddings, and the FTS5/`sqlite-vec` indexes live in encrypted pages. The `-wal` and
  `-shm` sidecars, checkpointed pages, and `VACUUM INTO` backups contain no plaintext chunk content,
  file paths, or embeddings. `braga doctor` check `encryption.at_rest` verifies a canary never
  appears in plaintext in the main database, `-wal`, or `-shm`; the storage `single_file_invariant`
  test extends that assertion to a checkpoint and a `VACUUM INTO` backup. The configured index opens
  only with its 32-byte key, and a different key is rejected.
- **The source folder is unencrypted.** bRAGa encrypts a derived index, not your documents. The
  files under `scan.dir` stay in plaintext wherever they are. Encrypting the index does not protect
  the corpus it was built from.
- **Live process memory is out of scope.** A running query engine decrypts pages to compute
  scores. `ptrace`, a debugger, a core dump, or a same-user memory dump can recover plaintext.
- **A compromised host defeats the model.** Code execution as your user or as root can read the
  key file and the index.
- **Remote embedder traffic is plaintext to the provider.** `braga-embedder` sends chunk text as
  JSON to the configured endpoint. TLS protects the wire, not the provider's ability to read the
  text; bRAGa has no local-only gate. Treat the endpoint as part of your trust boundary.
- **The key file is plaintext on disk.** `~/.braga/key` is 32 raw bytes protected only by file
  permissions (`0600` file, `0700` parent on Unix; default ACL on Windows, with no automatic
  hardening). It is not passphrase-derived and not held in an OS keychain. Losing it loses the
  index; copying it with the index defeats the encryption.

`braga-extra` is not shipped. There is no per-chunk or per-embedding AEAD in this repository, no
`ChunkEncryptor`/`KeyProvider` seam, no external-key mode, and no blind-index keyword search. Those
are a planned future mode, and no security claim is made about them. See the threat model for the
full non-goals.

## Licensing and the `braga-extra` seam

This repository and everything it ships are **Apache-2.0 only**. There is no AGPL code, no
AGPL-licensed dependency, and no feature-gated AGPL code path inside the Apache-2.0 artifact.

A future `braga-extra` is planned as a **separate crate** under AGPL-3.0 (with a commercial
option). It would depend on `braga`, never the reverse, and would not be a Cargo feature, an
optional module, or any other feature-gated path inside the Apache-2.0 distribution. That one-way
dependency is the seam that keeps AGPL code out of the Apache-2.0 artifact. Nothing in this
repository currently implements that crate or its per-record encryption.
