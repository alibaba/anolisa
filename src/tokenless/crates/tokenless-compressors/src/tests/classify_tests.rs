#[test]
fn error_warning_and_summary_lines_are_signal() {
    for line in [
        "error[E0308]: mismatched types",
        "warning: unused variable: `x`",
        "FAILED tests/test_x.py::test_y - AssertionError",
        "thread 'main' panicked at src/main.rs:5:5:",
        "npm ERR! code ELIFECYCLE",
        "test result: FAILED. 3 passed; 1 failed; 0 ignored",
        "Process exited with exit code 1",
        "collect2: fatal error: ld returned 1 exit status",
        "✗ lint",
        "Traceback (most recent call last):",
    ] {
        assert_eq!(analyze(line).class, LineClass::Signal, "line: {line}");
    }
}

/// Failures that name no error: the crash, the kernel, the shell, or the
/// network reporting it. Each one is the whole story of a failed run.
#[test]
fn keywordless_failures_are_signal() {
    for line in [
        "Segmentation fault (core dumped)",
        "Aborted (core dumped)",
        "Killed",
        "OOMKilled",
        "signal: killed",
        "out of memory: Kill process 8123 (rustc) score 901",
        "./build.sh: line 42: gcc: command not found",
        "bash: ./deploy.sh: Permission denied",
        "cp: cannot stat 'build/app': No such file or directory",
        "ld.so: libssl.so.1.1: cannot open shared object file",
        "Connection refused",
        "curl: (28) Operation timed out after 30001 milliseconds",
        "TIMEOUT after 600s",
        "yarn install exited with 1",
        "413 Request Entity Too Large",
    ] {
        assert_eq!(analyze(line).class, LineClass::Signal, "line: {line}");
    }
}

#[test]
fn routine_progress_lines_are_neutral() {
    for line in [
        "   Compiling serde v1.0.190",
        "Downloading 47 crates",
        "npm http fetch GET 200 https://registry.npmjs.org/foo 12ms",
        "get https://proxy.golang.org/golang.org/x/sys/@v/list",
        "  Installing collected packages: pytest",
    ] {
        assert_eq!(analyze(line).class, LineClass::Neutral, "line: {line}");
    }
}

#[test]
fn go_diagnostics_are_signal_but_goroutine_frames_are_not() {
    assert_eq!(analyze("./main.go:10:2: undefined: fooBar").class, LineClass::Signal);
    assert_eq!(analyze("    main_test.go:10: unexpected value").class, LineClass::Signal);
    // A goroutine dump frame's line number is followed by an offset.
    assert_eq!(analyze("\t/work/main.go:10 +0x24").class, LineClass::Neutral);
}

#[test]
fn routine_looking_line_reporting_a_failure_is_signal() {
    assert_eq!(analyze("Downloading foo v1.2 failed").class, LineClass::Signal);
}

#[test]
fn other_lines_are_neutral() {
    for line in ["running 5 tests", "   Doc-tests tokenless", "", "some plain output"] {
        assert_eq!(analyze(line).class, LineClass::Neutral, "line: {line}");
    }
}

#[test]
fn level_buckets() {
    assert_eq!(analyze("INFO: starting daemon").level, LogLevel::Info);
    assert_eq!(analyze("[debug] cache miss").level, LogLevel::Debug);
    assert_eq!(analyze("notice: config reloaded").level, LogLevel::Notice);
    assert_eq!(analyze("TRACE spans flushed").level, LogLevel::Trace);
    assert_eq!(analyze("plain output").level, LogLevel::Other);
}
