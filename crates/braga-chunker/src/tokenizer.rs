// SPDX-License-Identifier: Apache-2.0

//! Tokenizer strategy used to size chunk windows.

/// Splits `text` into token byte spans.
///
/// Each span is `[start, end)` within `text`, in order, non-overlapping, and aligned to UTF-8
/// codepoint boundaries. Token counts only size windows; chunk identity comes from
/// [`content_hash`](crate::content_hash), so an approximate tokenizer never affects correctness.
pub trait Tokenizer: Send + Sync {
    /// Returns the token spans of `text`, in order, covering it without gaps or overlaps.
    #[must_use]
    fn token_spans(&self, text: &str) -> Vec<(usize, usize)>;
}

/// Dependency-free byte/char fallback tokenizer.
///
/// It budgets four bytes per token, close to `cl100k_base`'s ~3.7 bytes/token on English prose,
/// and rounds each span up to a codepoint boundary. A `\r\n` pair is kept inside one span so a
/// chunk boundary can never split it. The token count is an approximation for providers this crate
/// cannot identify yet; it is documented drift, not a hidden assumption.
#[derive(Debug, Clone, Copy)]
pub struct ByteTokenizer {
    bytes_per_token: usize,
}

impl ByteTokenizer {
    /// Creates the fallback tokenizer with its four-bytes-per-token budget.
    #[must_use]
    pub const fn new() -> Self {
        Self { bytes_per_token: 4 }
    }
}

impl Default for ByteTokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Tokenizer for ByteTokenizer {
    fn token_spans(&self, text: &str) -> Vec<(usize, usize)> {
        let bytes = text.as_bytes();
        let mut spans = Vec::with_capacity(bytes.len() / self.bytes_per_token + 1);
        let mut start = 0;
        while start < bytes.len() {
            let mut end = (start + self.bytes_per_token).min(bytes.len());
            while end < bytes.len() && !text.is_char_boundary(end) {
                end += 1;
            }
            if end < bytes.len() && bytes[end - 1] == b'\r' && bytes[end] == b'\n' {
                end += 1;
            }
            spans.push((start, end));
            start = end;
        }
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteTokenizer, Tokenizer};

    fn spans(text: &str, bytes_per_token: usize) -> Vec<(usize, usize)> {
        ByteTokenizer { bytes_per_token }.token_spans(text)
    }

    fn assert_well_formed(text: &str, spans: &[(usize, usize)]) {
        let mut expected_start = 0;
        for &(start, end) in spans {
            assert_eq!(start, expected_start, "spans must be ordered and contiguous");
            assert!(start < end, "spans must be non-empty");
            assert!(end <= text.len());
            assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
            assert!(
                !(start > 0
                    && text.as_bytes()[start - 1] == b'\r'
                    && text.as_bytes()[start] == b'\n'),
                "a span must not start between CR and LF"
            );
            expected_start = end;
        }
        assert_eq!(expected_start, text.len(), "spans must cover the whole text");
    }

    #[test]
    fn empty_text_has_no_spans() {
        assert_eq!(spans("", 4), Vec::new());
    }

    #[test]
    fn spans_are_ordered_non_overlapping_and_char_aligned() {
        let fixtures =
            ["hello world", "héllo wörld", "日本語のテキストです", "🙂🙃🙂", "a\r\nb\r\nc", "a\rb"];
        for text in fixtures {
            for bytes_per_token in [1, 2, 3, 4, 5, 7] {
                assert_well_formed(text, &spans(text, bytes_per_token));
            }
        }
    }

    #[test]
    fn four_byte_budget_produces_four_byte_spans() {
        assert_eq!(spans("abcdefghijkl", 4), vec![(0, 4), (4, 8), (8, 12)]);
    }

    #[test]
    fn crlf_stays_in_one_span() {
        let spans = spans("a\r\nb\r\nc", 2);
        assert_eq!(spans, vec![(0, 3), (3, 6), (6, 7)]);
    }

    #[test]
    fn multibyte_codepoints_are_not_split() {
        // "é" occupies two bytes, so a two-byte budget rounds up past it.
        assert_eq!(spans("aé", 2), vec![(0, 3)]);
        assert_eq!(spans("aébc", 2), vec![(0, 3), (3, 5)]);
    }
}
