//! HTTP body decompression utility.
//!
//! Detects the `Content-Encoding` response header and decompresses the raw
//! bytes before they are converted to strings or parsed as JSON / SSE.
//! Supported codecs: `gzip` (`x-gzip`), `deflate`, `zstd`, `br` (brotli).
//! Graceful degradation: if decompression fails, the original bytes are
//! returned unchanged.

use std::io::Read;

/// Decompress an HTTP body based on its `Content-Encoding` header value.
///
/// - `None` or `"identity"` → return body unchanged
/// - `"gzip"` or `"x-gzip"` → decompress with GzDecoder
/// - `"deflate"` → decompress with DeflateDecoder
/// - `"zstd"` → decompress with the zstd decoder
/// - `"br"` → decompress with the brotli decoder
/// - Unknown encoding → return body unchanged
/// - Decompression failure → return original body (graceful fallback)
///
/// Auto-detection: if `content_encoding` is None/unknown but the body starts
/// with a known magic prefix, the codec is inferred from the bytes. This
/// handles HTTP/2 responses where the HPACK stateless decoder can't resolve
/// `Content-Encoding` from the dynamic table. Detected prefixes:
/// - gzip: `1f 8b`
/// - zstd: `28 b5 2f fd`
///
/// (Brotli has no reliable magic prefix, so it is only used when the header
/// explicitly says `br`.)
pub fn decompress_body(body: &[u8], content_encoding: Option<&str>) -> Vec<u8> {
    let encoding = content_encoding.map(|e| e.trim().to_lowercase());

    // Resolve the effective encoding: trust the header for known codecs,
    // otherwise fall back to magic-byte sniffing.
    let effective_encoding = match encoding.as_deref() {
        Some("gzip") | Some("x-gzip") | Some("deflate") | Some("zstd") | Some("br") => encoding,
        _ => {
            if body.len() >= 2 && body[0] == 0x1f && body[1] == 0x8b {
                Some("gzip".to_string())
            } else if body.len() >= 4
                && body[0] == 0x28
                && body[1] == 0xb5
                && body[2] == 0x2f
                && body[3] == 0xfd
            {
                // zstd magic number (little-endian 0xFD2FB528)
                Some("zstd".to_string())
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
                    log::warn!("gzip decompression failed ({e:?}), using raw body");
                    body.to_vec()
                }
            }
        }
        Some("deflate") => {
            let mut decoded = Vec::new();
            match flate2::read::DeflateDecoder::new(body).read_to_end(&mut decoded) {
                Ok(_) => decoded,
                Err(e) => {
                    log::warn!("deflate decompression failed ({e:?}), using raw body");
                    body.to_vec()
                }
            }
        }
        Some("zstd") => {
            // `decode_all` handles a stream of one or more concatenated frames,
            // which is what a flushed-per-event zstd SSE stream produces.
            match zstd::decode_all(body) {
                Ok(decoded) => decoded,
                Err(e) => {
                    log::warn!("zstd decompression failed ({e:?}), using raw body");
                    body.to_vec()
                }
            }
        }
        Some("br") => {
            let mut decoded = Vec::new();
            let mut reader = brotli::Decompressor::new(body, 4096);
            match reader.read_to_end(&mut decoded) {
                Ok(_) => decoded,
                Err(e) => {
                    log::warn!("brotli decompression failed ({e:?}), using raw body");
                    body.to_vec()
                }
            }
        }
        _ => body.to_vec(),
    }
}

/// Strip HTTP chunked transfer-encoding framing, returning the concatenated
/// chunk data (binary-safe). Stops at the terminating zero-size chunk. On
/// malformed or incomplete input, returns whatever was decoded so far.
///
/// Needed for compressed SSE streams: the raw bytes look like
/// `<hex>\r\n<data>\r\n…0\r\n\r\n` and the compressed payload must be
/// concatenated *without* the framing before it can be decompressed.
pub fn dechunk_body(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        // The chunk-size line ends at the first CRLF.
        let mut j = i;
        while j + 1 < raw.len() && !(raw[j] == b'\r' && raw[j + 1] == b'\n') {
            j += 1;
        }
        if j + 1 >= raw.len() {
            break; // no CRLF found -> incomplete header
        }
        let size_line = &raw[i..j];
        // Chunk size is hex, up to an optional ';' chunk-extension.
        let hex_end = size_line
            .iter()
            .position(|&b| b == b';')
            .unwrap_or(size_line.len());
        let size = match std::str::from_utf8(&size_line[..hex_end])
            .ok()
            .map(|s| s.trim())
            .and_then(|s| usize::from_str_radix(s, 16).ok())
        {
            Some(s) => s,
            None => break, // malformed size -> stop
        };
        i = j + 2; // skip CRLF after the size line
        if size == 0 {
            break; // terminating zero-size chunk
        }
        if i + size > raw.len() {
            // Incomplete final chunk: salvage what is present.
            out.extend_from_slice(&raw[i..]);
            break;
        }
        out.extend_from_slice(&raw[i..i + size]);
        i += size;
        // Skip the CRLF that follows the chunk data.
        i += 2;
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn zstd_decompresses_by_header() {
        let plain = b"data: {\"type\":\"message_start\"}\n\n";
        let comp = zstd::encode_all(&plain[..], 3).unwrap();
        assert_ne!(comp.as_slice(), &plain[..]);
        assert_eq!(decompress_body(&comp, Some("zstd")), plain);
    }

    #[test]
    fn zstd_decompresses_by_magic_autodetect() {
        // Mirrors the real HTTP/2 case where Content-Encoding can't be resolved
        // from the HPACK dynamic table: rely on the magic-byte sniff.
        let plain = b"event: content_block_delta\ndata: {\"text\":\"hi\"}\n\n";
        let comp = zstd::encode_all(&plain[..], 3).unwrap();
        assert_eq!(&comp[..4], &[0x28, 0xb5, 0x2f, 0xfd]);
        assert_eq!(decompress_body(&comp, None), plain);
    }

    #[test]
    fn brotli_decompresses_by_header() {
        let plain = b"data: {\"type\":\"content_block_delta\"}\n\n";
        let mut comp = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut comp, 4096, 5, 22);
            w.write_all(plain).unwrap();
        }
        assert_eq!(decompress_body(&comp, Some("br")), plain);
    }

    #[test]
    fn identity_and_unknown_pass_through() {
        let body = b"plain body";
        assert_eq!(decompress_body(body, None), body);
        assert_eq!(decompress_body(body, Some("identity")), body);
        assert_eq!(decompress_body(body, Some("weird-codec")), body);
    }

    #[test]
    fn corrupt_compressed_falls_back_to_raw() {
        // Claims zstd but is not a valid zstd stream → graceful raw fallback.
        let body = b"not really zstd";
        assert_eq!(decompress_body(body, Some("zstd")), body);
    }

    #[test]
    fn dechunk_strips_framing() {
        let chunked = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(dechunk_body(chunked), b"hello world");
    }

    #[test]
    fn dechunk_then_zstd_recovers_sse_stream() {
        // The real failure mode: a chunk-framed, zstd-compressed SSE response.
        let sse = b"event: content_block_delta\ndata: {\"text\":\"hi\"}\n\nevent: message_stop\ndata: {}\n\n";
        let comp = zstd::encode_all(&sse[..], 3).unwrap();
        // Frame the compressed bytes across two chunks + the zero terminator.
        let mid = comp.len() / 2;
        let mut chunked = Vec::new();
        chunked.extend_from_slice(format!("{mid:x}\r\n").as_bytes());
        chunked.extend_from_slice(&comp[..mid]);
        chunked.extend_from_slice(b"\r\n");
        chunked.extend_from_slice(format!("{:x}\r\n", comp.len() - mid).as_bytes());
        chunked.extend_from_slice(&comp[mid..]);
        chunked.extend_from_slice(b"\r\n0\r\n\r\n");

        let dechunked = dechunk_body(&chunked);
        assert_eq!(dechunked, comp);
        assert_eq!(decompress_body(&dechunked, Some("zstd")), sse);
    }
}
