//! Fake-Agent coverage for responsive ACP session orchestration.

use std::time::{Duration, Instant};

use super::*;

const FRAME_LIMIT: usize = 16 * 1024;

#[cfg(unix)]
fn driver(script: &str, workspace: &tempfile::TempDir) -> AcpSessionDriver {
    let mut launch = RuntimeLaunchSpec::new("/bin/sh", workspace.path());
    launch.arguments = vec!["-c".into(), script.into()];
    let mut config = AcpSessionDriverConfig::new(
        launch,
        AcpV1ClientConfig::new("cosh-ng", "0.15.0", FRAME_LIMIT),
        workspace.path(),
    );
    config.initialize_timeout = Duration::from_secs(2);
    config.prompt_timeout = Duration::from_secs(2);
    config.shutdown_grace = Duration::from_millis(50);
    config.command_timeout = Duration::from_secs(3);
    AcpSessionDriver::launch(config).unwrap()
}

fn observation(driver: &AcpSessionDriver) -> AcpV1Observation {
    match driver.receive_timeout(Duration::from_secs(2)).unwrap() {
        AcpSessionEvent::Observation(observation) => observation,
        AcpSessionEvent::Terminal(terminal) => {
            panic!(
                "unexpected terminal: {:?} {:?}",
                terminal.kind, terminal.detail
            )
        }
    }
}

#[cfg(unix)]
#[test]
fn driver_streams_one_prompt_and_settles_once() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-1","result":{"protocolVersion":1,"agentCapabilities":{}}}' ;;
        2) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-2","result":{"sessionId":"session-1"}}' ;;
        3)
           printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}'
           printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-3","result":{"stopReason":"end_turn"}}'
           ;;
    esac
done
"#;
    let driver = driver(script, &workspace);

    driver.initialize().unwrap();
    assert!(matches!(
        observation(&driver),
        AcpV1Observation::Initialized { .. }
    ));
    driver.open_session().unwrap();
    assert!(matches!(
        observation(&driver),
        AcpV1Observation::SessionOpened { .. }
    ));
    driver.prompt("hello").unwrap();
    assert!(matches!(
        observation(&driver),
        AcpV1Observation::SessionUpdate { .. }
    ));
    assert!(matches!(
        observation(&driver),
        AcpV1Observation::PromptFinished { .. }
    ));
    driver.shutdown().unwrap();
    let AcpSessionEvent::Terminal(terminal) =
        driver.receive_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected terminal")
    };
    assert_eq!(terminal.kind, AcpSessionTerminalKind::Shutdown);
    assert!(terminal.process.is_some());
    assert!(matches!(
        driver.receive_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout)
    ));
}

#[cfg(unix)]
#[test]
fn independent_cancel_reaps_silent_agent() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-1","result":{"protocolVersion":1,"agentCapabilities":{}}}' ;;
        2) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-2","result":{"sessionId":"session-1"}}' ;;
        3) while :; do sleep 1; done ;;
    esac
done
"#;
    let driver = driver(script, &workspace);
    driver.initialize().unwrap();
    observation(&driver);
    driver.open_session().unwrap();
    observation(&driver);
    driver.prompt("wait").unwrap();

    let started = Instant::now();
    driver.control().cancel().unwrap();
    let AcpSessionEvent::Terminal(terminal) =
        driver.receive_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected terminal")
    };
    assert_eq!(terminal.kind, AcpSessionTerminalKind::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(terminal.process.is_some());
}

#[cfg(unix)]
#[test]
fn cancel_settles_pending_permission_before_reap() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-1","result":{"protocolVersion":1,"agentCapabilities":{}}}' ;;
        2) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-2","result":{"sessionId":"session-1"}}' ;;
        3) printf '%s\n' '{"jsonrpc":"2.0","id":41,"method":"session/request_permission","params":{"sessionId":"session-1","toolCall":{"toolCallId":"tool-1","title":"Run"},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]}}' ;;
    esac
done
"#;
    let driver = driver(script, &workspace);
    driver.initialize().unwrap();
    observation(&driver);
    driver.open_session().unwrap();
    observation(&driver);
    driver.prompt("permission").unwrap();
    let AcpV1Observation::PermissionRequested(request) = observation(&driver) else {
        panic!("expected permission request")
    };
    assert_eq!(request.request_id, AcpV1RequestId::Number(41));

    driver.control().cancel().unwrap();
    let AcpSessionEvent::Terminal(terminal) =
        driver.receive_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected terminal")
    };
    assert_eq!(terminal.kind, AcpSessionTerminalKind::Cancelled);
    assert!(
        terminal.detail.is_none(),
        "cancel frames should encode cleanly"
    );
}

#[cfg(unix)]
#[test]
fn unsupported_callback_is_rejected_by_the_actor() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-1","result":{"protocolVersion":1,"agentCapabilities":{}}}' ;;
        2) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-2","result":{"sessionId":"session-1"}}' ;;
        3) printf '%s\n' '{"jsonrpc":"2.0","id":77,"method":"fs/read_text_file","params":{"sessionId":"session-1","path":"/etc/passwd"}}' ;;
        4)
           printf '%s\n' "$line" | grep -q '"code":-32601' || exit 9
           printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-3","result":{"stopReason":"end_turn"}}'
           ;;
    esac
done
"#;
    let driver = driver(script, &workspace);
    driver.initialize().unwrap();
    observation(&driver);
    driver.open_session().unwrap();
    observation(&driver);
    driver.prompt("unsupported").unwrap();
    assert!(matches!(
        observation(&driver),
        AcpV1Observation::UnsupportedClientRequest { .. }
    ));
    assert!(matches!(
        observation(&driver),
        AcpV1Observation::PromptFinished { .. }
    ));
    driver.shutdown().unwrap();
}

#[cfg(unix)]
#[test]
fn malformed_initialize_fails_closed_with_one_terminal() {
    let workspace = tempfile::tempdir().unwrap();
    let driver = driver(
        "read -r line; printf '%s\\n' 'not-json'; sleep 60",
        &workspace,
    );

    assert!(matches!(
        driver.initialize(),
        Err(AcpSessionDriverError::Bridge(_))
    ));
    let AcpSessionEvent::Terminal(terminal) =
        driver.receive_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected terminal")
    };
    assert_eq!(terminal.kind, AcpSessionTerminalKind::Failed);
    assert!(terminal.detail.is_some());
    assert!(terminal.process.is_some());
}

#[cfg(unix)]
#[test]
fn permission_decision_is_single_use() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-1","result":{"protocolVersion":1,"agentCapabilities":{}}}' ;;
        2) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-2","result":{"sessionId":"session-1"}}' ;;
        3) printf '%s\n' '{"jsonrpc":"2.0","id":41,"method":"session/request_permission","params":{"sessionId":"session-1","toolCall":{"toolCallId":"tool-1","title":"Run"},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]}}' ;;
        4) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-3","result":{"stopReason":"end_turn"}}' ;;
    esac
done
"#;
    let driver = driver(script, &workspace);
    driver.initialize().unwrap();
    observation(&driver);
    driver.open_session().unwrap();
    observation(&driver);
    driver.prompt("permission").unwrap();
    let AcpV1Observation::PermissionRequested(request) = observation(&driver) else {
        panic!("expected permission request")
    };
    driver
        .answer_permission(
            request.request_id.clone(),
            AcpV1PermissionDecision::Selected {
                option_id: "allow".to_owned(),
            },
        )
        .unwrap();
    assert!(driver
        .answer_permission(request.request_id, AcpV1PermissionDecision::Cancelled)
        .is_err());
    driver.shutdown().unwrap();
}

#[cfg(unix)]
#[test]
fn terminal_is_delivered_after_buffered_observations() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-1","result":{"protocolVersion":1,"agentCapabilities":{}}}' ;;
        2) printf '%s\n' '{"jsonrpc":"2.0","id":"cosh-acp-2","result":{"sessionId":"session-1"}}' ;;
        3)
           i=0
           while [ "$i" -lt 40 ]; do
             printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"chunk"}}}}'
             i=$((i + 1))
           done
           ;;
    esac
done
"#;
    let driver = driver(script, &workspace);
    driver.initialize().unwrap();
    observation(&driver);
    driver.open_session().unwrap();
    observation(&driver);
    driver.prompt("overflow").unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let mut observations = 0;
    loop {
        match driver.receive_timeout(Duration::from_secs(2)).unwrap() {
            AcpSessionEvent::Observation(_) => observations += 1,
            AcpSessionEvent::Terminal(terminal) => {
                assert_eq!(terminal.kind, AcpSessionTerminalKind::Failed);
                break;
            }
        }
    }
    assert_eq!(observations, EVENT_CAPACITY);
    assert!(driver.receive_timeout(Duration::from_millis(20)).is_err());
}
