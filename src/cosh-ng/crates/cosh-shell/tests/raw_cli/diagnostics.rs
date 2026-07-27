use super::*;

#[test]
fn diagnostics_export_collects_sources_without_leaking_secrets() {
    let home = temp_shell_home("diagnostics-export");
    let state = home.join(".copilot-shell");
    let logs = state.join("logs");
    fs::create_dir_all(&logs).expect("create log directory");
    fs::write(
        logs.join("cosh-shell.log.current"),
        "request token=diagnostic-secret-token\n",
    )
    .expect("write diagnostic log");
    fs::write(
        state.join("audit-events.jsonl"),
        "{\"authorization\":\"Bearer diagnostic-secret-auth\"}\n",
    )
    .expect("write diagnostic events");
    fs::write(
        state.join("last-crash.log"),
        "password: diagnostic-secret-password\n",
    )
    .expect("write crash summary");
    let output = home.join("diagnostic.json");

    let command_output = Command::new(env!("CARGO_BIN_EXE_cosh-shell"))
        .args(["diagnostics", "export", "--output"])
        .arg(&output)
        .env("HOME", &home)
        .env("COSH_SHELL_HEALTH_SCAN", "fixture:linux-healthy")
        .output()
        .expect("run diagnostics export");
    assert!(
        command_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&command_output.stderr)
    );

    let content = fs::read_to_string(&output).expect("read diagnostic bundle");
    let bundle: serde_json::Value =
        serde_json::from_str(&content).expect("parse diagnostic bundle");
    assert_eq!(bundle["format"], "cosh-diagnostic-bundle");
    assert_eq!(bundle["version"], 1);
    assert_eq!(bundle["sources"]["health"]["overall_severity"], "ok");
    assert!(bundle["sources"]["health"]["findings"].is_array());
    assert!(bundle["sources"]["health"]["unavailable"].is_array());
    assert!(bundle["sources"]["health"]["try_items"].is_array());
    assert!(!content.contains("diagnostic-secret-token"));
    assert!(!content.contains("diagnostic-secret-auth"));
    assert!(!content.contains("diagnostic-secret-password"));
    assert!(content.contains("<redacted>"));
    assert_eq!(
        fs::metadata(&output)
            .expect("diagnostic metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let _ = fs::remove_dir_all(home);
}
