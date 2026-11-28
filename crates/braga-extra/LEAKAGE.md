<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Blind-index leakage table

The blind index is **opt-in and off by default**. It lets an extra-mode index answer one
keyword-token query without decrypting chunk text, at the cost of publishing a pattern of token
hashes. It is property-preserving-encryption territory: the postings leak equality and
co-occurrence structure, and are subject to the inference attacks described by Naveed, Kamara, and
Wright, "Inference Attacks on Property-Preserving Encrypted Databases" (ACM CCS 2015). Do not
enable it when that leakage is unacceptable.

| What leaks | To whom |
|---|---|
| Token *frequency* per document | anyone with the DB file |
| Token *co-occurrence* within a chunk (postings share a `chunk_id`) | anyone with the DB file |
| *Query* token patterns (which truncated HMACs are looked up, and when) | anyone observing the query path |
| File paths, content hashes, chunk offsets/line numbers, model registry | anyone with the DB file (metadata is not AEAD-encrypted) |
| **Not leaked:** chunk text, embedding vectors, per-chunk key, master key | — |

What each row means:

- **Token frequency per document.** The `blind_index` table holds one row per distinct token hash
  per chunk. Counting the rows that share a `chunk_id` reveals how many distinct words a chunk has;
  a hash that appears in many chunks reveals how common that word is across the corpus.
- **Token co-occurrence.** All postings for one chunk carry the same `chunk_id`, so an observer can
  see which token hashes occur together. Combined with frequency analysis, that is enough to
  reconstruct substantial text statistics.
- **Query token patterns.** A lookup is `SELECT chunk_id FROM blind_index WHERE token = ?1`. An
  observer of the query path (process tracing, query logging, or repeated timing measurements) can
  see which hashes are requested and how often, and correlate that with the stored postings.
- **Metadata.** The AEAD covers chunk text and embedding vectors, not the surrounding rows. File
  paths, content hashes, chunk offsets, line numbers, and the model registry stay readable with the
  database key alone.

What does **not** leak, even with the index enabled:

- chunk text and embedding vectors, which remain sealed under the per-record AEAD keys;
- the per-chunk and per-embedding keys;
- the master key, from which the blind-index key is derived and which is never written to the
  database.

The truncation to 8 bytes keeps the index compact and bounds a single posting, but it also makes
hash collisions possible: two different tokens can map to the same posting, and a query can return
chunks that matched the other token. That is a correctness trade, not a privacy guarantee. HMAC with
a secret key prevents an observer without the master key from testing candidate tokens offline, but
it does not hide the patterns in the table above.

The index does not change the retrieval boundary: vector search still decrypts and scores in
memory, and the host process that performs a query can see plaintext. A compromise of the process
(ptrace or a memory dump) is outside the protection this crate offers.
