//! Tokenizer for estimating token counts.

/// Estimate token count from text using character-based heuristic.
/// Uses ~4 characters per token for English text as a rough approximation.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.chars().count().div_ceil(4)
}

/// Estimate token count from byte length when text is unavailable.
/// Uses ~4 bytes per token for ASCII/English text. For UTF-8 multi-byte
/// characters this overestimates (fewer bytes per token); for CJK text
/// (~3 bytes/char, ~1-2 chars/token) it underestimates. Use
/// `estimate_tokens(&str)` when text is available for more accurate results.
pub fn estimate_tokens_from_bytes(bytes: usize) -> usize {
    if bytes == 0 {
        return 0;
    }
    bytes.div_ceil(4)
}

/// Count Unicode characters in text.
pub fn count_chars(text: &str) -> usize {
    text.chars().count()
}

/// Backwards-compatible struct for existing code.
/// Prefer using the free functions `estimate_tokens` and `count_chars` directly.
pub struct Tokenizer;

impl Tokenizer {
    #[doc(hidden)]
    pub fn new() -> Self {
        Self
    }

    #[doc(hidden)]
    pub fn estimate_tokens(&self, text: &str) -> usize {
        estimate_tokens(text)
    }

    #[doc(hidden)]
    pub fn count_chars(&self, text: &str) -> usize {
        count_chars(text)
    }
}

impl Default for Tokenizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens_from_bytes(0), 0);
        assert_eq!(count_chars(""), 0);
    }

    #[test]
    fn ascii_text() {
        // 11 chars / 4 = 3 tokens
        assert_eq!(estimate_tokens("hello world"), 3);
        assert_eq!(count_chars("hello world"), 11);
    }

    #[test]
    fn cjk_text() {
        // 4 CJK chars / 4 = 1 token
        assert_eq!(estimate_tokens("你好世界"), 1);
        assert_eq!(count_chars("你好世界"), 4);
    }

    #[test]
    fn emoji() {
        // 2 emoji chars / 4 = 1 token
        assert_eq!(estimate_tokens("🎉🎊"), 1);
        assert_eq!(count_chars("🎉🎊"), 2);
    }

    #[test]
    fn mixed_text() {
        // ASCII + CJK mixed
        let text = "Hello你好";
        assert_eq!(count_chars(text), 7);
        assert_eq!(estimate_tokens(text), 2);
    }

    #[test]
    fn byte_estimate_vs_char_estimate() {
        // For ASCII, byte and char estimates should match
        let text = "abcdef";
        assert_eq!(
            estimate_tokens(text),
            estimate_tokens_from_bytes(text.len())
        );
    }
}
