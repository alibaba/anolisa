use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime};

use asc_daemon_client::{ClientError, DaemonClient};
use asc_daemon_protocol::RevisionParams;
use opentelemetry::Context;
use opentelemetry::trace::{TraceContextExt as _, Tracer as _, TracerProvider as _};
use opentelemetry_sdk::trace::SdkTracerProvider;
use serde_json::{Value, json};

#[test]
fn authenticated_get_is_bounded_and_propagates_w3c_trace_context() {
    let directory = unique_directory("success");
    fs::create_dir(&directory).unwrap();
    let socket = directory.join("daemon.sock");
    let token_file = directory.join("token");
    fs::write(&token_file, "01234567890123456789012345678901").unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut frame = String::new();
        BufReader::new(&mut stream).read_line(&mut frame).unwrap();
        let request: Value = serde_json::from_str(frame.trim()).unwrap();
        let response = json!({
            "request_id": "request-1",
            "ok": true,
            "data": {"policyId": "6efed5ea-47c9-4b14-8e86-888f2ad88fc7", "revision": 1},
            "stdout": "",
            "stderr": "",
            "exit_code": 0
        });
        let mut bytes = serde_json::to_vec(&response).unwrap();
        bytes.push(b'\n');
        stream.write_all(&bytes).unwrap();
        request
    });

    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("asc-daemon-client-test");
    let context = Context::current_with_span(tracer.start("policy.get"));
    let client =
        DaemonClient::from_token_file(&socket, &token_file, Duration::from_secs(1)).unwrap();
    let response = client
        .call(
            "policy.templates.get",
            &RevisionParams {
                id: "6efed5ea-47c9-4b14-8e86-888f2ad88fc7".to_owned(),
                revision: 1,
            },
            &context,
        )
        .unwrap();
    assert_eq!(
        response.data["policyId"],
        "6efed5ea-47c9-4b14-8e86-888f2ad88fc7"
    );

    drop(context);
    provider.shutdown().unwrap();
    let request = server.join().unwrap();
    assert_eq!(request["method"], "policy.templates.get");
    assert_eq!(
        request["params"],
        json!({"id": "6efed5ea-47c9-4b14-8e86-888f2ad88fc7", "revision": 1})
    );
    assert_eq!(request["auth"]["scheme"], "bearer");
    assert_eq!(request["auth"]["token"], "01234567890123456789012345678901");
    assert!(request["traceparent"].as_str().unwrap().starts_with("00-"));

    cleanup(&directory);
}

#[test]
fn missing_socket_is_a_stable_unavailable_error() {
    let directory = unique_directory("unavailable");
    fs::create_dir(&directory).unwrap();
    let token_file = directory.join("token");
    fs::write(&token_file, "01234567890123456789012345678901").unwrap();
    let client = DaemonClient::from_token_file(
        directory.join("missing.sock"),
        &token_file,
        Duration::from_secs(1),
    )
    .unwrap();
    let error = client
        .call(
            "policy.bindings.get",
            &json!({"id": "binding-1"}),
            &Context::new(),
        )
        .unwrap_err();
    assert_eq!(error, ClientError::DaemonUnavailable);
    assert_eq!(error.code(), "daemon_unavailable");
    cleanup(&directory);
}

fn unique_directory(suffix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "asc-daemon-client-{suffix}-{}-{nonce}",
        std::process::id()
    ))
}

fn cleanup(directory: &PathBuf) {
    let _ = fs::remove_file(directory.join("daemon.sock"));
    let _ = fs::remove_file(directory.join("token"));
    let _ = fs::remove_dir(directory);
}
