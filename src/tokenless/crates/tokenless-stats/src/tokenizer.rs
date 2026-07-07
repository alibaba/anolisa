//! Tokenizer for estimating token counts.

/// Estimate token count from text using character-based heuristic.
/// CJK characters count as ~1 token each; other characters use ~4 per token.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut cjk = 0usize;
    let mut other = 0usize;
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4)
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{20000}'..='\u{2A6DF}'
        | '\u{2A700}'..='\u{2B73F}'
        | '\u{2B740}'..='\u{2B81F}'
        | '\u{2B820}'..='\u{2CEAF}'
        | '\u{2CEB0}'..='\u{2EBEF}'
        | '\u{30000}'..='\u{3134F}'
        | '\u{3100}'..='\u{312F}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{3040}'..='\u{309F}'
        | '\u{30A0}'..='\u{30FF}'
    )
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
    include!("tests/tokenizer_tests.rs");
}
