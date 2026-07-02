//! Marker generation and parsing.
//!
//! A marker is `<<tokenless:HASH>>` where HASH is a 24-hex-char stash key.
//! Compressors embed markers in truncated output so the LLM can quote the
//! marker back to retrieve the original payload.

/// Marker prefix. The `tokenless:` namespace distinguishes these markers from
/// Headroom's `<<ccr:HASH>>` and from any user content.
pub const MARKER_PREFIX: &str = "<<tokenless:";

/// Marker suffix.
pub const MARKER_SUFFIX: &str = ">>";

/// Build a marker string for `hash`.
pub fn marker_for(hash: &str) -> String {
    format!("{MARKER_PREFIX}{hash}{MARKER_SUFFIX}")
}

/// Parse a marker that occupies the entirety of `s`, returning the embedded
/// hash. Returns `None` for malformed input rather than panicking, so callers
/// can pass untrusted LLM output directly.
pub fn parse_marker(s: &str) -> Option<&str> {
    let inner = s.strip_prefix(MARKER_PREFIX)?;
    let inner = inner.strip_suffix(MARKER_SUFFIX)?;
    validate_hash(inner)?;
    Some(inner)
}

/// Extract the first marker's hash from arbitrary text. Useful when the LLM
/// quotes a whole truncation line such as
/// `<... 12 items truncated, retrieve with <<tokenless:abcd…>>`.
pub fn extract_hash(text: &str) -> Option<&str> {
    let start = text.find(MARKER_PREFIX)?;
    let rest = &text[start + MARKER_PREFIX.len()..];
    let end = rest.find(MARKER_SUFFIX)?;
    let hash = &rest[..end];
    validate_hash(hash)?;
    Some(hash)
}

/// A valid stash key is exactly 24 lowercase hex characters.
fn validate_hash(hash: &str) -> Option<()> {
    if hash.len() == 24 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let hash = "0123456789abcdef01234567";
        let marker = marker_for(hash);
        assert_eq!(marker, "<<tokenless:0123456789abcdef01234567>>");
        assert_eq!(parse_marker(&marker), Some(hash));
    }

    #[test]
    fn parse_rejects_non_marker() {
        assert_eq!(parse_marker("not a marker"), None);
        assert_eq!(parse_marker("<<tokenless:abc>>"), None); // too short
        assert_eq!(parse_marker("<<tokenless:ZZZZZZZZZZZZZZZZZZZZZZZZ>>"), None); // non-hex
        assert_eq!(parse_marker(""), None);
    }

    #[test]
    fn parse_rejects_embedded_marker() {
        // parse_marker requires the whole string to be a marker; use
        // extract_hash for embedded forms.
        let line = "<... 12 items truncated, retrieve with <<tokenless:0123456789abcdef01234567>>";
        assert_eq!(parse_marker(line), None);
        assert_eq!(extract_hash(line), Some("0123456789abcdef01234567"));
    }

    #[test]
    fn extract_hash_from_plain_marker() {
        let marker = marker_for("abcdef0123456789abcdef01");
        assert_eq!(extract_hash(&marker), Some("abcdef0123456789abcdef01"));
    }

    #[test]
    fn extract_hash_none_when_absent() {
        assert_eq!(extract_hash("no marker here"), None);
        assert_eq!(extract_hash(""), None);
    }

    #[test]
    fn extract_hash_rejects_malformed() {
        // Prefix present but no closing suffix.
        assert_eq!(extract_hash("<<tokenless:0123456789abcdef01234567"), None);
        // Wrong length inside a well-formed marker pair.
        assert_eq!(extract_hash("<<tokenless:abc>>"), None);
    }

    #[test]
    fn extract_hash_picks_first_of_multiple() {
        let text =
            "<<tokenless:000000000000000000000000>> then <<tokenless:111111111111111111111111>>";
        assert_eq!(extract_hash(text), Some("000000000000000000000000"));
    }
}
