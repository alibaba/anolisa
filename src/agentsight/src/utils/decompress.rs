//! HTTP body decompression utility.
//!
//! Detects `Content-Encoding: gzip` or `deflate` in response headers
//! and decompresses the raw bytes before they are converted to strings
//! or parsed as JSON. Graceful degradation: if decompression fails,
//! the original bytes are returned unchanged.

use std::io::Read;

/// Decompress an HTTP body based on its `Content-Encoding` header value.
///
/// - `None` or `"identity"` → return body unchanged
/// - `"gzip"` or `"x-gzip"` → decompress with GzDecoder
/// - `"deflate"` → decompress with DeflateDecoder
/// - Unknown encoding → return body unchanged
/// - Decompression failure → return original body (graceful fallback)
///
/// Auto-detection: if `content_encoding` is None/unknown but the body starts
/// with gzip magic bytes `\x1f\x8b`, treat it as gzip. This handles HTTP/2
/// responses where the HPACK stateless decoder can't resolve Content-Encoding
/// from the dynamic table.
pub fn decompress_body(body: &[u8], content_encoding: Option<&str>) -> Vec<u8> {
    let encoding = content_encoding.map(|e| e.trim().to_lowercase());

    // Auto-detect gzip if encoding header wasn't resolved
    let effective_encoding = match encoding.as_deref() {
        Some("gzip") | Some("x-gzip") | Some("deflate") => encoding,
        _ => {
            // No encoding header (or unknown) — check for gzip magic bytes
            if body.len() >= 2 && body[0] == 0x1f && body[1] == 0x8b {
                Some("gzip".to_string())
            } else {
                encoding
            }
        }
    };

    match effective_encoding.as_deref() {
        Some("gzip") | Some("x-gzip") => {
            let mut decoded = Vec::new();
            match flate2::read::GzDecoder::new(body).read_to_end(&mut decoded) {
                Ok(_) => decoded,
                Err(e) => {
                    log::warn!("gzip decompression failed ({:?}), using raw body", e);
                    body.to_vec()
                }
            }
        }
        Some("deflate") => {
            let mut decoded = Vec::new();
            match flate2::read::DeflateDecoder::new(body).read_to_end(&mut decoded) {
                Ok(_) => decoded,
                Err(e) => {
                    log::warn!("deflate decompression failed ({:?}), using raw body", e);
                    body.to_vec()
                }
            }
        }
        _ => body.to_vec(),
    }
}

/// Convenience: decompress and then convert to String.
///
/// Returns `None` if the (decompressed) body is empty or not valid UTF-8.
pub fn decompress_body_to_string(body: &[u8], content_encoding: Option<&str>) -> Option<String> {
    let decompressed = decompress_body(body, content_encoding);
    if decompressed.is_empty() {
        None
    } else {
        String::from_utf8(decompressed).ok()
    }
}
