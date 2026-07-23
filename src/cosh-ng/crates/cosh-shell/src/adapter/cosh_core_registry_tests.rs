use serde_json::Value;

use super::cosh_core::CoshCoreAdapter;

fn test_adapter_with_program(program: &str) -> CoshCoreAdapter {
    CoshCoreAdapter {
        program: program.to_string(),
        allow_model_call: false,
        session: Default::default(),
    }
}

fn write_mock_script(label: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let script = std::env::temp_dir().join(format!(
        "cosh-registry-test-{label}-{}-{n}.sh",
        std::process::id()
    ));
    // Remove stale script if it exists (avoid ETXTBSY)
    let _ = std::fs::remove_file(&script);
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).expect("write mock script");
    let mut permissions = std::fs::metadata(&script)
        .expect("mock script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("chmod mock script");
    // Sync to avoid ETXTBSY race
    std::thread::sleep(std::time::Duration::from_millis(10));
    script
}

#[test]
fn registry_query_parses_success_response() {
    let script = write_mock_script(
        "success",
        r#"read REQUEST
printf '%s\n' '{"type":"registry_response","request_id":"reg-test","success":true,"data":["ext-a","ext-b"]}'
"#,
    );

    let adapter = test_adapter_with_program(&script.to_string_lossy());
    let result = adapter.registry_query("extensions", "list", Value::Null);
    let _ = std::fs::remove_file(&script);

    let data = result.expect("should parse success response");
    assert!(data.is_array(), "data should be array: {data:?}");
    let arr = data.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0], "ext-a");
    assert_eq!(arr[1], "ext-b");
}

#[test]
fn registry_query_returns_error_on_failure() {
    let script = write_mock_script(
        "failure",
        r#"read REQUEST
printf '%s\n' '{"type":"registry_response","request_id":"reg-test","success":false,"error":"extension not found"}'
"#,
    );

    let adapter = test_adapter_with_program(&script.to_string_lossy());
    let result = adapter.registry_query(
        "extensions",
        "detail",
        serde_json::json!({"name": "nonexistent"}),
    );
    let _ = std::fs::remove_file(&script);

    let err = result.expect_err("should return error");
    assert!(
        err.contains("extension not found"),
        "unexpected error: {err}"
    );
}

#[test]
fn registry_query_handles_spawn_failure() {
    let adapter = test_adapter_with_program("/nonexistent/path/to/cosh-core-fake-binary");
    let result = adapter.registry_query("extensions", "list", Value::Null);

    let err = result.expect_err("should fail on spawn");
    assert!(err.contains("failed to spawn"), "unexpected error: {err}");
}

#[test]
fn registry_query_handles_timeout() {
    let script = write_mock_script(
        "timeout",
        // Read request but never respond — just sleep longer than the 5s timeout
        r#"read REQUEST
sleep 60
"#,
    );

    let adapter = test_adapter_with_program(&script.to_string_lossy());
    let result = adapter.registry_query("extensions", "list", Value::Null);
    let _ = std::fs::remove_file(&script);

    let err = result.expect_err("should timeout");
    assert!(
        err.contains("timed out") || err.contains("no response"),
        "unexpected error: {err}"
    );
}

#[test]
fn registry_query_handles_empty_stdout() {
    let script = write_mock_script(
        "empty",
        // Read request then exit immediately without writing anything
        r#"read REQUEST
"#,
    );

    let adapter = test_adapter_with_program(&script.to_string_lossy());
    let result = adapter.registry_query("extensions", "list", Value::Null);
    let _ = std::fs::remove_file(&script);

    let err = result.expect_err("should fail on empty stdout");
    assert!(
        err.contains("no response") || err.contains("EOF"),
        "unexpected error: {err}"
    );
}

#[test]
fn registry_query_handles_invalid_json() {
    let script = write_mock_script(
        "invalid-json",
        r#"read REQUEST
printf '%s\n' 'this is not valid json'
"#,
    );

    let adapter = test_adapter_with_program(&script.to_string_lossy());
    let result = adapter.registry_query("extensions", "list", Value::Null);
    let _ = std::fs::remove_file(&script);

    let err = result.expect_err("should fail on invalid json");
    assert!(err.contains("parse error"), "unexpected error: {err}");
}
