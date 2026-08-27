const BUILD_LOG_MARKERS: &[&str] = &[
    "Compiling ",
    "Finished ",
    "Downloading ",
    "Installing ",
    "warning:",
    "error:",
    "error[",
    "FAILED",
    " passed",
    " failed",
    "npm ",
    "npm ERR!",
    "make[",
    "make: ",
    "Entering directory",
    "Leaving directory",
    "cc -",
    "gcc ",
    "clang ",
    "ld: ",
    "collect2",
    "go build",
    "go: downloading",
    "test result:",
    "$ ",
];

/// Terminal build/test output: ANSI sequences, progress redraws, or at least
/// two distinct marker kinds.
pub(super) fn is_build_log(scan: &str) -> bool {
    let mut score = 0usize;
    if scan.contains('\u{1b}') {
        score += 1;
    }
    if has_bare_carriage_return(scan) {
        score += 1;
    }
    score += BUILD_LOG_MARKERS
        .iter()
        .filter(|m| scan.contains(**m))
        .count();
    score >= 2
}

/// A `\r` not followed by `\n` is a progress-bar redraw, not a Windows line
/// ending.
fn has_bare_carriage_return(scan: &str) -> bool {
    let bytes = scan.as_bytes();
    bytes
        .iter()
        .enumerate()
        .any(|(i, b)| *b == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
}
