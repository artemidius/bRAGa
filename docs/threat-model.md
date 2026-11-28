# bRAGa threat model

> **Status: early development. Not audited.** bRAGa is not audited and provides no security
> guarantees. This document describes what the shipped code does and what it deliberately does
> not do; it is not a security guarantee.

This is the product-level statement that every wrapper and downstream integration should link to.
The encryption-at-rest claims below are written so that the storage test suite
(`crates/braga-storage/tests/roundtrip.rs::single_file_invariant`) or the `braga doctor` check
`encryption.at_rest` can verify them. The source-folder, key-file custody, and live-memory
non-goals are also `doctor`'s documented non-goals.

## Adversary and scope

**In scope** — an attacker who obtains the files but not your running user account:

- theft of a laptop or its disk, or a raw disk image taken while the machine is off;
- an offsite or cloud backup that captured `~/.braga/index.db` and its sidecars;
- casual local file access by another user or process that cannot read your home directory or the
  key file.

**Out of scope** — an attacker with code execution, memory access, or control of the host:

- a compromised host, root/admin access, malware, or a malicious process running as your user;
- a hostile local process, `ptrace`, a debugger, a core dump, or a memory dump;
- the source documents themselves, which bRAGa never encrypts;
- the remote embedding provider and the network path to it;
- physical attacks such as cold-boot memory recovery or DMA.

## What base mode protects

bRAGa's base mode encrypts **the index at rest**. The index is a single SQLite database at
`storage.path` (default `~/.braga/index.db`). `braga-storage` uses `rusqlite` with the
`bundled-sqlcipher` feature, which builds SQLCipher 4.20.0 (SQLite 3.54.1, libtomcrypt 1.18.2)
from `sqlcipher-src`. Encryption is at the SQLCipher **page/codec level**, not whole-file
encryption: the raw 32-byte key is applied with `Connection::key` before any statement runs, and
every page SQLCipher writes or reads is encrypted or decrypted through that codec.

Concretely:

- The main database file is a sequence of encrypted pages. Chunk text, file paths, content hashes,
  embeddings, the FTS5 index, and the `sqlite-vec` index all live in those pages.
- `PRAGMA journal_mode=WAL` is set. WAL frames are written through the same codec, so the `-wal`
  sidecar carries encrypted page images rather than plaintext rows.
- The `-shm` sidecar is a transient shared-memory index over the WAL. It is not a store of table
  content, and the at-rest test asserts the canary does not appear in it either.
- Checkpointing copies pages through the pager, so the checkpointed main database stays encrypted;
  the WAL is truncated after a checkpoint.
- `VACUUM INTO` produces a backup database through the same pager codec, so a backup taken with
  `Store::vacuum_into` is encrypted with the same key. The backup reopens with that key and fails
  with `StoreError::WrongKey` under a different key.

What this means in practice: someone who copies `index.db` and `index.db-wal` without also
obtaining the key sees encrypted page data. The `-shm` sidecar is a transient index over those
pages and exposes no chunk content, paths, or embeddings. The index file's own name, size, and
timestamps remain visible; the file paths stored inside it do not.

Evidence, per claim:

| Claim | Verified by |
|---|---|
| The configured index opens only with the configured key | `braga doctor` check `store.open`; `crypto.roundtrip` also proves a different key is rejected |
| A canary written to the database does not appear in plaintext in the main database, `-wal`, or `-shm` | `braga doctor` check `encryption.at_rest` |
| The same holds across a checkpoint and a `VACUUM INTO` backup | `single_file_invariant` in `crates/braga-storage/tests/roundtrip.rs` |
| The backup is encrypted and key-bound | `single_file_invariant`: backup reopens with the key, fails with `WrongKey` otherwise |

`encryption.at_rest` runs against a fresh scratch database created in a temporary directory, not
against your existing index. It proves the key/codec path that the shipped code uses; it does not
audit the bytes of an index you already have. `store.open` is the check that touches your
configured path. There is no claim here that `doctor` scans an arbitrary pre-existing index for
plaintext.

## What it does NOT protect

These are explicit non-goals. Each is a property `braga doctor` also says it does not check.

### The source folder is unencrypted

bRAGa encrypts a **derived index**, not your documents. The directory named by `scan.dir` (default
`.`) keeps the original files in plaintext, in place. bRAGa does not encrypt, move, or rewrite them.
Anyone who can read the source folder reads the documents without needing the key or the index.
Encrypting the index protects the derived artifact; it does not protect the corpus it was built
from.

### Live process memory is out of scope

Any query engine must decrypt pages to compute scores. While a `braga` command, the library, or a
wrapper is running, chunk text, paths, and vectors can exist in plaintext in process memory. A
debugger attached to the process (`ptrace`), a core dump, a same-user memory dump, or a malicious
library in the process can recover that plaintext. bRAGa does not lock pages into memory, does not
guarantee zeroization of the SQLite page cache, and does not claim resistance to memory inspection.

### A compromised host defeats the model

If an attacker can execute code as your user or as root, they can read `~/.braga/key` and the index,
or wait until the next command unlocks it. Base-mode encryption protects files at rest when the
attacker has the files but not the account; it is not protection against a compromised machine.

### Remote embedder traffic is plaintext to the provider

`braga-embedder` sends chunk text to the configured endpoint as JSON over `ureq`. If
`embedding.provider` points at a remote OpenAI-compatible or Ollama service, that text leaves the
machine in plaintext from bRAGa's point of view: bRAGa does not encrypt the payload before sending,
does not add a local-only gate, and cannot control what the provider retains or logs. HTTPS protects
the wire between the machine and the provider; it does not make the provider unable to read the
chunks. For sensitive corpora, point the embedder at a provider you control, such as a local Ollama
instance, and treat the endpoint as part of your trust boundary.

### The key file is plaintext on disk

The key file holds the raw 32 bytes. Its only protection is filesystem permissions. It is not
derived from a passphrase, not sealed in an OS keychain or TPM, and not escrowed by bRAGa. An
attacker who copies both the index and the key file can decrypt the index. Backups must protect the
key separately from the index; backing up both together defeats the encryption.

## Key residency

- **Material:** exactly 32 raw bytes generated from the operating system CSPRNG via `getrandom`.
  There is no passphrase, password-based key derivation, key wrapping, or key rotation in base mode.
- **Default location:** `~/.braga/key`.
- **Resolution order:** `BRAGA_KEY_FILE`, then an explicit path passed by the caller (the CLI's
  `--key-file`), then `~/.braga/key`. `BRAGA_KEY_FILE` wins over `--key-file`.
- **Permissions:** on Unix the file is created with mode `0o600` and a missing parent directory with
  mode `0o700`. On Windows the file is created with the default ACL; there is no automatic
  hardening, so the caller is responsible for tightening it.
- **No clobbering:** writing a key uses `create_new`, so an existing key file is never overwritten.
  Losing the key means losing the index; bRAGa keeps no recovery copy.
- **Redaction:** `Key`'s `Debug` output is `Key([REDACTED])`, there is no `Display`, and key bytes
  never appear in errors or logs. Errors carry paths and fixed reasons only.

## `braga-extra` is not shipped

**`braga-extra` is not shipped.** There is no `braga-extra` crate in this repository, no per-chunk
or per-embedding AEAD, no `ChunkEncryptor` or `KeyProvider` trait, no caller-supplied external-key
mode, and no blind-index keyword search. These are a **planned future mode**, not a present
capability. No security claim is made about them.

If that mode is built, it is planned as a separate AGPL-3.0/commercial crate that depends on
`braga`, never the reverse, and never as a feature-gated path inside the Apache-2.0 artifact. Base
mode would remain exactly what this document describes.

## What `doctor` checks

`braga doctor` reports six stable check names:
`config.valid`, `store.open`, `embed.roundtrip`, `crypto.roundtrip`, `encryption.at_rest`, and the
opt-in `embed.live` (enabled with `BRAGA_DOCTOR_LIVE_EMBED`). It claims nothing beyond them. In
particular, it does not check the source folder, key-file custody, live process memory, the
provider's handling of embeddings, or a pre-existing index's bytes.
