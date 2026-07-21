use std::process::{Command, Stdio};

fn run_with_tools(selection: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cosh-core"))
        .args(["--headless", "--bare", "--tools", selection])
        .stdin(Stdio::null())
        .output()
        .expect("run cosh-core")
}

#[test]
fn unknown_tools_exit_nonzero() {
    let output = run_with_tools("missing_tool");

    assert!(!output.status.success(), "status={:?}", output.status);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown tools: missing_tool"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn empty_tools_remain_valid() {
    let output = run_with_tools("");

    assert!(
        output.status.success(),
        "status={:?}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
