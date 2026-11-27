// SPDX-License-Identifier: Apache-2.0

//! Token-window chunking over a decoded UTF-8 document.

use crate::hash::{ContentHash, content_hash};
use crate::tokenizer::{ByteTokenizer, Tokenizer};

/// A contiguous window of a decoded document.
///
/// The fields mirror the `chunks` table columns one-for-one, except `file_id`, which the scanner
/// assigns. See the [crate documentation](crate) for the mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// `SHA-256` of [`content`](Self::content) bytes (`chunks.id`).
    pub id: ContentHash,
    /// Byte offset of `content` in the source document (`chunks.byte_offset`).
    pub byte_offset: u64,
    /// 1-based, inclusive first line of the chunk (`chunks.line_start`).
    pub line_start: u32,
    /// 1-based, inclusive last line of the chunk (`chunks.line_end`).
    pub line_end: u32,
    /// Exact UTF-8 chunk text; a leading BOM and CRLF pairs are preserved.
    pub content: String,
}

/// Sizing policy for [`Chunker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkerConfig {
    /// Tokens a window accumulates before it is emitted.
    pub target_tokens: usize,
    /// Tokens shared with the next window; must be smaller than `target_tokens`.
    pub overlap_tokens: usize,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self { target_tokens: 800, overlap_tokens: 100 }
    }
}

/// Chunks documents into overlapping token windows.
#[derive(Debug, Clone)]
pub struct Chunker<T: Tokenizer> {
    tokenizer: T,
    config: ChunkerConfig,
}

impl Chunker<ByteTokenizer> {
    /// Builds a chunker with [`ChunkerConfig::default`] and the [`ByteTokenizer`] fallback.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(ChunkerConfig::default())
    }

    /// Builds a chunker with an explicit configuration and the [`ByteTokenizer`] fallback.
    ///
    /// # Panics
    ///
    /// Panics if `target_tokens` is zero or `overlap_tokens` is not smaller than
    /// `target_tokens`; either would prevent the window walk from advancing.
    #[must_use]
    pub fn with_config(config: ChunkerConfig) -> Self {
        assert!(config.target_tokens > 0, "target_tokens must be non-zero");
        assert!(
            config.overlap_tokens < config.target_tokens,
            "overlap_tokens must be smaller than target_tokens"
        );
        Self { tokenizer: ByteTokenizer::new(), config }
    }
}

impl Default for Chunker<ByteTokenizer> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Tokenizer> Chunker<T> {
    /// Splits `text` into chunks of roughly [`ChunkerConfig::target_tokens`] tokens.
    ///
    /// Returns no chunks for empty text. A document shorter than the target becomes exactly one
    /// chunk. A final window is emitted only when it contributes at least one token beyond the
    /// previous window, so a zero-gain tail is never produced.
    ///
    /// `text` must be the raw decode of the file bytes (BOM and CRLF intact); byte offsets are
    /// file byte offsets and chunk content reproduces the source bytes exactly.
    ///
    /// # Panics
    ///
    /// Panics only if the document is larger than `u64::MAX` bytes or has more than `u32::MAX`
    /// lines, which the `&str` API cannot reach on supported targets.
    #[must_use]
    pub fn chunk(&self, text: &str) -> Vec<Chunk> {
        let spans = self.tokenizer.token_spans(text);
        if spans.is_empty() {
            return Vec::new();
        }

        let line_starts = line_starts(text);
        let mut chunks = Vec::new();
        let mut first_token = 0;
        loop {
            let end_token = (first_token + self.config.target_tokens).min(spans.len());
            let byte_start = spans[first_token].0;
            let byte_end = spans[end_token - 1].1;
            let content = &text[byte_start..byte_end];

            chunks.push(Chunk {
                id: content_hash(content.as_bytes()),
                byte_offset: u64::try_from(byte_start).expect("byte offset exceeds u64"),
                line_start: line_number(&line_starts, byte_start),
                line_end: line_number(&line_starts, byte_end - 1),
                content: content.to_owned(),
            });

            if end_token == spans.len() {
                break;
            }
            first_token = end_token - self.config.overlap_tokens;
        }
        chunks
    }
}

/// Starting byte offset of every line, plus the position after a trailing `\n`.
///
/// Only `\n` terminates a line: `\r` is an ordinary byte, so CRLF is a single break.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(text.len() / 32 + 1);
    starts.push(0);
    for (offset, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(offset + 1);
        }
    }
    starts
}

/// 1-based number of the line containing `offset`, found by binary search.
fn line_number(starts: &[usize], offset: usize) -> u32 {
    let line = starts.partition_point(|&start| start <= offset);
    u32::try_from(line).expect("line count exceeds u32")
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::{Chunk, Chunker, ChunkerConfig};
    use crate::hash::content_hash;
    use crate::tokenizer::ByteTokenizer;

    fn ascii_doc(tokens: usize) -> String {
        "a".repeat(tokens * 4)
    }

    fn custom(target_tokens: usize, overlap_tokens: usize) -> Chunker<ByteTokenizer> {
        Chunker::with_config(ChunkerConfig { target_tokens, overlap_tokens })
    }

    fn offset(chunk: &Chunk) -> usize {
        usize::try_from(chunk.byte_offset).expect("offset fits usize")
    }

    /// Recomputes the line range by counting newlines, independently of the chunker's index.
    fn expected_lines(text: &str, chunk: &Chunk) -> (u32, u32) {
        let start = offset(chunk);
        let end = start + chunk.content.len();
        let first = 1 + u32::try_from(text[..start].matches('\n').count()).expect("line count");
        let last = 1 + u32::try_from(text[..end - 1].matches('\n').count()).expect("line count");
        (first, last)
    }

    #[test]
    fn empty_text_yields_no_chunks() {
        assert_eq!(Chunker::new().chunk(""), Vec::new());
    }

    #[test]
    fn short_text_yields_one_whole_text_chunk() {
        let text = "hello\nworld";
        let chunks = Chunker::new().chunk(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, text);
        assert_eq!(chunks[0].byte_offset, 0);
        assert_eq!((chunks[0].line_start, chunks[0].line_end), (1, 2));
        assert_eq!(chunks[0].id, content_hash(text.as_bytes()));
    }

    #[test]
    fn defaults_are_800_tokens_over_100() {
        let config = ChunkerConfig::default();
        assert_eq!((config.target_tokens, config.overlap_tokens), (800, 100));
    }

    #[test]
    fn exactly_one_target_yields_one_chunk_and_no_zero_gain_tail() {
        assert_eq!(Chunker::new().chunk(&ascii_doc(800)).len(), 1);
    }

    #[test]
    fn longer_documents_split_into_overlapping_windows() {
        let text = ascii_doc(1000);
        let chunks = Chunker::new().chunk(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(offset(&chunks[0]), 0);
        assert_eq!(chunks[0].content.len(), 3200);
        assert_eq!(offset(&chunks[1]), 2800);
        assert_eq!(chunks[1].content.len(), text.len() - 2800);
        // 100 tokens of 4 bytes are shared between the two windows.
        assert_eq!(&chunks[0].content[2800..], &chunks[1].content[..400]);
    }

    #[test]
    fn every_chunk_id_is_the_sha256_of_its_content() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(200);
        let chunks = Chunker::new().chunk(&text);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert_eq!(chunk.id, content_hash(chunk.content.as_bytes()));
        }
    }

    #[test]
    fn no_two_chunks_in_one_document_share_an_id() {
        let mut text = String::new();
        for index in 0..500 {
            write!(text, "{index:04}").expect("writing to a String cannot fail");
        }
        let chunks = custom(50, 10).chunk(&text);
        assert!(chunks.len() > 1);
        let mut ids: Vec<_> = chunks.iter().map(|chunk| chunk.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), chunks.len());
    }

    #[test]
    fn chunks_reconstruct_the_source_bytes() {
        let text = "αβγ\r\n日本語 🙂 and more text\nwith lines\n";
        let chunks = custom(2, 1).chunk(text);
        let mut covered = vec![false; text.len()];
        for chunk in &chunks {
            let start = offset(chunk);
            let end = start + chunk.content.len();
            assert_eq!(&text[start..end], chunk.content);
            covered[start..end].fill(true);
        }
        assert!(covered.iter().all(|&byte| byte));
    }

    #[test]
    fn line_ranges_match_the_source_across_crlf() {
        let text = "alpha\r\nbeta\r\ngamma\r\ndelta\r\nepsilon\r\nzeta\r\neta\r\ntheta\r\n";
        let chunks = custom(2, 1).chunk(text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert_eq!((chunk.line_start, chunk.line_end), expected_lines(text, chunk));
        }
        for chunk in &chunks[1..] {
            let start = offset(chunk);
            assert!(!(text.as_bytes()[start - 1] == b'\r' && text.as_bytes()[start] == b'\n'));
        }
    }

    #[test]
    fn a_trailing_line_without_newline_still_counts() {
        let chunks = Chunker::new().chunk("first\nsecond");
        assert_eq!((chunks[0].line_start, chunks[0].line_end), (1, 2));

        let chunks = Chunker::new().chunk("first\nsecond\n");
        assert_eq!((chunks[0].line_start, chunks[0].line_end), (1, 2));
    }

    #[test]
    fn a_leading_bom_is_preserved_at_offset_zero() {
        let text = format!("\u{feff}{}", ascii_doc(400));
        let chunks = Chunker::new().chunk(&text);
        assert_ne!(chunks.len(), 0);
        assert_eq!(chunks[0].byte_offset, 0);
        assert_eq!(chunks[0].line_start, 1);
        assert!(chunks[0].content.starts_with('\u{feff}'));
        for chunk in &chunks {
            let start = offset(chunk);
            let end = start + chunk.content.len();
            assert_eq!(&text[start..end], chunk.content);
        }
    }

    #[test]
    fn multibyte_documents_never_split_a_codepoint() {
        let text = "日本語のテキストをチャンクに分割します。🙂🙃🎉 ".repeat(20);
        let chunks = custom(3, 1).chunk(&text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            let start = offset(chunk);
            let end = start + chunk.content.len();
            assert!(text.is_char_boundary(start));
            assert!(text.is_char_boundary(end));
            assert_eq!(&text[start..end], chunk.content);
        }
    }
}
