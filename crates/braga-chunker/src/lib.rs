// SPDX-License-Identifier: Apache-2.0

//! Tokenizer-aware document chunking for `bRAGa`.
//!
//! [`Chunker`] turns a decoded UTF-8 document into [`Chunk`] windows of roughly
//! [`target_tokens`](ChunkerConfig::target_tokens) tokens with
//! [`overlap_tokens`](ChunkerConfig::overlap_tokens) tokens of overlap. Each chunk carries an exact
//! byte offset, an inclusive 1-based line range, and a content-addressed id.
//!
//! This is a pure leaf crate: no scanning, embedding, querying, or storage I/O. The scanner
//! supplies the text and assigns `file_id`.
//!
//! # Byte fidelity
//!
//! `text` must be `str::from_utf8` of the raw file bytes, never a lossy conversion, so every byte
//! index is a file byte offset. Chunking preserves the bytes it is given:
//!
//! * A leading UTF-8 BOM (`EF BB BF`) is kept, occupies line 1, and stays at `byte_offset == 0`
//!   in the first chunk. Stripping it is a scanner policy: doing it here would shift every
//!   downstream offset by three bytes.
//! * Lines are split only on `\n`. A `\r` before `\n` belongs to the line, so CRLF is one break; a
//!   lone `\r` is not a break. [`ByteTokenizer`] treats a `\r\n` pair as atomic, so a chunk never
//!   separates the two bytes.
//! * Boundaries come from token spans, which are UTF-8 codepoint-aligned, so a multi-byte
//!   codepoint is never split.
//! * Non-UTF-8 files are out of scope; the `&str` API cannot silently mangle bytes.
//!
//! # Tokenizer drift
//!
//! Token counts are only a sizing heuristic. The default [`ByteTokenizer`] is a dependency-free
//! approximation of about four bytes per token; the `OpenAI` `cl100k_base`, `Ollama`, and `TEI`
//! tokenizers all differ, and Stage 7 adds a real `tiktoken-rs` implementation behind the
//! [`Tokenizer`] trait. Chunk identity is `SHA-256` of the exact chunk bytes, so this drift never
//! changes an id.
//!
//! # [`Chunk`] and the `chunks` table
//!
//! | [`Chunk`] field | `chunks` column |
//! |---|---|
//! | `id` | `id` (`BLOB`, `SHA-256`) |
//! | `byte_offset` | `byte_offset` |
//! | `line_start` | `line_start` |
//! | `line_end` | `line_end` |
//! | `content` | `content` |
//!
//! `file_id` is deliberately absent: the scanner owns it.

mod chunker;
mod hash;
mod tokenizer;

pub use chunker::{Chunk, Chunker, ChunkerConfig};
pub use hash::{ContentHash, content_hash};
pub use tokenizer::{ByteTokenizer, Tokenizer};
