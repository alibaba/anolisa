//! Tokenizer for estimating token counts.

/// Estimate token count from text using character-based heuristic.
/// Uses ~4 characters per token for English text as a rough approximation.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.chars().count().div_ceil(4)
}

/// Estimate token count from a character count when text is unavailable.
/// Same heuristic: ~4 characters per token.
pub fn estimate_tokens_from_chars(chars: usize) -> usize {
    if chars == 0 {
        return 0;
    }
    chars.div_ceil(4)
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
