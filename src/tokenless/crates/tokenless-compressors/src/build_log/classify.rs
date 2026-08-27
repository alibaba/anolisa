//! Line classification for build-log compression.
//!
//! Binary on purpose: a line is either Signal (never grouped, never
//! omitted) or Neutral (omittable). Template summaries apply to all omitted
//! lines (§6.1 groups repeated templated *lines*, not a special class), so
//! a finer noise taxonomy would drive nothing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineClass {
    /// Errors, warnings, failures, summaries — never grouped, never omitted.
    Signal,
    /// Everything else — omittable.
    Neutral,
}

/// Histogram bucket for an omitted line's log level. Signal lines are never
/// omitted, so warn/error levels need no buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogLevel {
    Info,
    Debug,
    Trace,
    Notice,
    Other,
}

pub(crate) struct LineInfo {
    pub(crate) class: LineClass,
    pub(crate) level: LogLevel,
}

/// Case-insensitive substrings that make a line Signal. Misfires keep a few
/// extra lines ("Compiling error-chain v0.1"), which is the safe direction.
///
/// The second group covers failures that name no error at all — a crash,
/// the kernel, or the shell reporting it. They were found by probing real
/// build/run output against the first group: a segfault, an OOM kill, or a
/// missing interpreter is the whole story of a failed run, and each one
/// otherwise reads as an ordinary Neutral line and gets stashed.
const SIGNAL_SUBSTRINGS: &[&str] = &[
    "error",
    "panic",
    "fatal",
    "fail",
    "warn",
    "exception",
    "assert",
    "undefined reference",
    "unresolved",
    "exit code",
    "exit status",
    "non-zero",
    "test result:",
    "passed",
    "npm err!",
    "traceback",
    // Crashes and kills: "Segmentation fault (core dumped)", "Aborted",
    // "Killed", "OOMKilled", "signal: killed".
    "segmentation fault",
    "core dumped",
    "aborted",
    "killed",
    "out of memory",
    // The shell or loader refusing to run something.
    "command not found",
    "permission denied",
    "no such file",
    "cannot open",
    "cannot stat",
    "cannot find",
    // Network and time limits, and exit-code prose the list above misses.
    "refused",
    "timed out",
    "timeout",
    "exited with",
    "too large",
];

/// Case-sensitive Signal marks that don't fold under lowercasing.
const SIGNAL_MARKS: &[&str] = &["✗"];

/// Go compiler diagnostics carry no keyword at all (`./main.go:10:2:
/// undefined: fooBar`): a `.go:` followed by a bare line number and another
/// colon. Goroutine dump frames (`\t/work/main.go:10 +0x24`) don't match —
/// their line number is followed by an offset, not a colon.
fn is_go_diagnostic(stripped: &str) -> bool {
    let Some(pos) = stripped.find(".go:") else {
        return false;
    };
    match stripped[pos + 4..].split_once(':') {
        Some((line_no, _)) => !line_no.is_empty() && line_no.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// Classify one ANSI-stripped line and extract its level bucket.
pub(crate) fn analyze(stripped: &str) -> LineInfo {
    let lower = stripped.to_lowercase();
    let class = if SIGNAL_SUBSTRINGS.iter().any(|s| lower.contains(s))
        || SIGNAL_MARKS.iter().any(|s| stripped.contains(s))
        || is_go_diagnostic(stripped)
    {
        LineClass::Signal
    } else {
        LineClass::Neutral
    };
    let level = if lower.contains("debug") {
        LogLevel::Debug
    } else if lower.contains("trace") {
        LogLevel::Trace
    } else if lower.contains("info") {
        LogLevel::Info
    } else if lower.contains("notice") {
        LogLevel::Notice
    } else {
        LogLevel::Other
    };
    LineInfo { class, level }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("../tests/classify_tests.rs");
}
