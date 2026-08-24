//! Content taxonomy and deterministic detection (roadmap §4.2).
//!
//! Detection is a pure function of the content with bounded work: only the
//! first [`MAX_SCAN_BYTES`] bytes / [`MAX_SCAN_LINES`] lines are inspected
//! (plus, for the JSON sniff alone, at most [`MAX_SCAN_BYTES`] trailing
//! bytes), and no format is fully parsed — expensive parsing belongs inside the
//! selected compressor, not in a detector that runs on every input. When no
//! cheap signal is decisive the detector prefers the more general class, and
//! ultimately [`ContentType::PlainText`] or [`ContentType::Unknown`]; the
//! pipeline routes those to passthrough until a conservative fallback
//! compressor exists.

mod build_log;
mod diff;
mod html;
mod json;
mod search_results;
mod source_code;
mod stack_trace;
mod tabular;

/// The first content taxonomy (roadmap §4.2).
///
/// Wire values (see [`ContentType::wire_str`]) are the stable strings the
/// protocol's `content_type` response field carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentType {
    /// JSON documents, typically arrays of records or nested objects.
    JsonRecords,
    /// grep/ripgrep-style `path:line[:col]:text` match listings.
    SearchResults,
    /// Compiler, package-manager, or test-runner terminal output.
    BuildLog,
    /// A stack trace or panic report, starting as one.
    StackTrace,
    /// Unified diff / patch output.
    Diff,
    /// A complete HTML document. Ambiguous fragments are not classified.
    Html,
    /// Delimiter-consistent tables (CSV, TSV, Markdown tables).
    Tabular,
    /// Program source code. Detected only on strong signals.
    SourceCode,
    /// Readable text with no more specific classification.
    PlainText,
    /// Empty, binary-like, or otherwise unclassifiable content.
    Unknown,
}

impl ContentType {
    /// The stable wire value used by the protocol's `content_type` field.
    #[must_use]
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::JsonRecords => "json_records",
            Self::SearchResults => "search_results",
            Self::BuildLog => "build_log",
            Self::StackTrace => "stack_trace",
            Self::Diff => "diff",
            Self::Html => "html",
            Self::Tabular => "tabular",
            Self::SourceCode => "source_code",
            Self::PlainText => "plain_text",
            Self::Unknown => "unknown",
        }
    }
}

/// Detection inspects at most this many leading bytes.
pub const MAX_SCAN_BYTES: usize = 64 * 1024;
/// Detection inspects at most this many leading lines.
pub const MAX_SCAN_LINES: usize = 200;

/// Classifies content into the taxonomy. Deterministic: identical input
/// always produces the identical class.
///
/// Checks run from the most distinctive shape to the most general one, so a
/// build log that merely *contains* a traceback stays [`ContentType::BuildLog`]
/// while content that *starts as* a traceback is [`ContentType::StackTrace`].
#[must_use]
pub fn detect(content: &str) -> ContentType {
    let scan = scan_prefix(content);
    if scan.trim().is_empty() {
        return ContentType::Unknown;
    }
    if !mostly_text(scan) {
        return ContentType::Unknown;
    }
    if diff::is_diff(scan) {
        return ContentType::Diff;
    }
    if stack_trace::is_stack_trace(scan) {
        return ContentType::StackTrace;
    }
    // Bracket sniff on the content's head and bounded tail: the JSON
    // compressor is the authority — it parses, and non-record JSON passes
    // through there.
    if json::is_json_like(content) {
        return ContentType::JsonRecords;
    }
    if html::is_html_document(scan) {
        return ContentType::Html;
    }
    if search_results::is_search_results(scan) {
        return ContentType::SearchResults;
    }
    if tabular::is_tabular(scan) {
        return ContentType::Tabular;
    }
    if build_log::is_build_log(scan) {
        return ContentType::BuildLog;
    }
    if source_code::is_source_code(scan) {
        return ContentType::SourceCode;
    }
    ContentType::PlainText
}

/// The bounded slice all line-based checks operate on, cut at a char
/// boundary at most [`MAX_SCAN_BYTES`] into the content.
fn scan_prefix(content: &str) -> &str {
    if content.len() <= MAX_SCAN_BYTES {
        return content;
    }
    let mut end = MAX_SCAN_BYTES;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    &content[..end]
}

fn scan_lines(scan: &str) -> impl Iterator<Item = &str> {
    scan.lines().take(MAX_SCAN_LINES)
}

fn non_empty_lines(scan: &str) -> impl Iterator<Item = &str> {
    scan_lines(scan).filter(|l| !l.trim().is_empty())
}

/// Rejects binary-like input: more than 5% of the leading bytes are control
/// characters other than the whitespace family and ESC (ANSI sequences are
/// legitimate in terminal output).
fn mostly_text(scan: &str) -> bool {
    let sample = &scan.as_bytes()[..scan.len().min(4096)];
    let control = sample
        .iter()
        .filter(|b| b.is_ascii_control() && !matches!(b, b'\n' | b'\r' | b'\t' | 0x1b))
        .count();
    control * 20 <= sample.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/content_tests.rs");
}
