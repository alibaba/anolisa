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

/// Length of a stash hash in hex characters (see `key.rs`).
const HASH_LEN: usize = 24;

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

/// Extract the first **valid** marker's hash from arbitrary text. Useful
/// when the LLM quotes a whole truncation line such as
/// `<... 12 items truncated, retrieve with <<tokenless:abcd…>>`.
///
/// Scans past malformed markers (wrong-length or non-hex content between a
/// prefix/suffix pair) so a hallucinated or partial marker earlier in the
/// text does not prevent retrieval of a valid marker that follows it.
///
/// The scan stays linear on untrusted input: at each prefix the fixed
/// `24-hex + suffix` window directly after it is validated in place instead
/// of searching the rest of the text for the next suffix, so a rejected
/// prefix costs O(1) and no tail is ever rescanned.
pub fn extract_hash(text: &str) -> Option<&str> {
    let mut search_from = 0;
    while let Some(start) = text[search_from..].find(MARKER_PREFIX) {
        let hash_start = search_from + start + MARKER_PREFIX.len();
        // A valid marker is exactly prefix + 24 hex chars + suffix, so only
        // the fixed window right after the prefix can complete it. When too
        // few bytes remain, `get` yields None and this prefix is rejected.
        if let Some(hash) = text.get(hash_start..hash_start + HASH_LEN)
            && validate_hash(hash).is_some()
            && text[hash_start + HASH_LEN..].starts_with(MARKER_SUFFIX)
        {
            return Some(hash);
        }
        // MARKER_PREFIX cannot overlap itself, so no marker can start inside
        // the prefix just rejected; resume right after it.
        search_from = hash_start;
    }
    None
}

/// Whether `hash` is a valid stash key: exactly 24 ASCII hex characters
/// (case-insensitive — keys are stored lowercase, lookups normalize). Public
/// so callers can validate a bare hash before a DB round-trip and surface a
/// clear format error to the user.
pub fn is_valid_hash(hash: &str) -> bool {
    hash.len() == HASH_LEN && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A valid stash key is exactly 24 ASCII hex characters.
fn validate_hash(hash: &str) -> Option<()> {
    if is_valid_hash(hash) { Some(()) } else { None }
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
    fn is_valid_hash_accepts_24_hex_case_insensitive() {
        assert!(is_valid_hash("0123456789abcdef01234567"));
        assert!(is_valid_hash("ABCDEF0123456789ABCDEF01")); // uppercase ok
    }

    #[test]
    fn is_valid_hash_rejects_malformed() {
        assert!(!is_valid_hash("0123456789abcdef0123456")); // 23 chars
        assert!(!is_valid_hash("0123456789abcdef0123456789")); // 26 chars
        assert!(!is_valid_hash("ZZZZZZZZZZZZZZZZZZZZZZZZ")); // non-hex
        assert!(!is_valid_hash(""));
        assert!(!is_valid_hash("/some/path"));
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

    #[test]
    fn extract_hash_skips_malformed_marker_before_valid() {
        // A partial/hallucinated marker (3 hex chars) precedes a valid one.
        let text = "see <<tokenless:abc>> then <<tokenless:0123456789abcdef01234567>>";
        assert_eq!(extract_hash(text), Some("0123456789abcdef01234567"));
    }

    #[test]
    fn extract_hash_skips_non_hex_marker_before_valid() {
        let text =
            "<<tokenless:ZZZZZZZZZZZZZZZZZZZZZZZZ>> and <<tokenless:abcdef0123456789abcdef01>>";
        assert_eq!(extract_hash(text), Some("abcdef0123456789abcdef01"));
    }

    #[test]
    fn extract_hash_all_malformed_returns_none() {
        assert_eq!(
            extract_hash("<<tokenless:abc>> and <<tokenless:def>>"),
            None,
        );
    }

    #[test]
    fn extract_hash_skips_nested_malformed_prefix_before_valid() {
        // An unclosed/partial prefix is followed immediately by a valid marker.
        // The first prefix pairs with the valid marker's suffix and produces
        // an invalid hash; scanning must resume inside that rejected span to
        // find the real marker.
        let text = "<<tokenless:abc <<tokenless:0123456789abcdef01234567>>";
        assert_eq!(extract_hash(text), Some("0123456789abcdef01234567"));
    }

    #[test]
    fn extract_hash_many_malformed_prefixes_returns_none() {
        // Pathological untrusted input for the old rescanning scanner: N
        // repeated prefixes sharing one trailing suffix. The fixed-window
        // scan rejects each prefix in O(1), keeping this linear.
        let text = format!("{}>>", "<<tokenless:".repeat(20_000));
        assert_eq!(extract_hash(&text), None);
    }

    #[test]
    fn extract_hash_finds_valid_marker_after_many_malformed_prefixes() {
        let mut text = String::new();
        for _ in 0..1_000 {
            text.push_str("<<tokenless:not-a-hash ");
        }
        text.push_str("<<tokenless:0123456789abcdef01234567>>");
        assert_eq!(extract_hash(&text), Some("0123456789abcdef01234567"));
    }
}
