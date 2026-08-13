//! Fake-Core coverage for public Runtime mapping and settlement.

use std::time::{Duration, Instant};

use cosh_gateway_contracts::{
    common::{BoundedName, BoundedText, ContentPart, Digest, WorkspaceRef},
    external::ExternalRefKind,
    ids::{
        AgentSessionId, InstallationId, RunId, RuntimeBindingId, RuntimeInstanceId,
        RuntimeMessageId, TaskId, ToolUseId,
    },
    runtime::{AgentRuntimeCommand, AgentRuntimeEvent, RunOutcome},
    task::CancelReason,
};

use super::*;

fn workspace_ref() -> WorkspaceRef {
    WorkspaceRef {
        scope_digest: Digest::parse("0".repeat(64)).unwrap(),
        display_name: Some(BoundedText::new("test workspace").unwrap()),
    }
}

#[cfg(unix)]
fn bridge(script: &str, workspace: &tempfile::TempDir) -> (CoshCoreBridge, CoshCoreBridgeIdentity) {
    let identity = CoshCoreBridgeIdentity {
        installation_id: InstallationId::new(),
        actor_id: None,
        task_id: TaskId::new(),
        run_id: RunId::new(),
        agent_session_id: AgentSessionId::new(),
        binding_id: RuntimeBindingId::new(),
        runtime_instance_id: RuntimeInstanceId::new(),
        runtime_generation: 7,
        provider_authority: BoundedName::new("cosh-core").unwrap(),
        provider_scope_digest: Digest::parse("1".repeat(64)).unwrap(),
    };
    let initialize_request_id = format!("init-{}", identity.runtime_instance_id);
    let script = script.replace("__INIT_REQUEST_ID__", &initialize_request_id);
    let mut launch = RuntimeLaunchSpec::new("/bin/sh", workspace.path());
    launch.arguments = vec!["-c".into(), script.into()];
    let mut config = CoshCoreBridgeConfig::new(launch, workspace_ref(), identity.clone());
    config.prompt_timeout = Duration::from_secs(2);
    config.shutdown_grace = Duration::from_millis(50);
    (CoshCoreBridge::launch(config).unwrap(), identity)
}

fn open(bridge: &mut CoshCoreBridge, identity: &CoshCoreBridgeIdentity) {
    bridge
        .dispatch(
            AgentRuntimeCommand::OpenSession {
                task_id: identity.task_id.clone(),
                run_id: identity.run_id.clone(),
                workspace: workspace_ref(),
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
}

#[cfg(unix)]
#[test]
fn core_bridge_maps_identity_stream_and_terminal_once() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
step=0
while IFS= read -r line; do
    step=$((step + 1))
    case "$step" in
        1)
            printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"__INIT_REQUEST_ID__","response":{"subtype":"initialize","protocol_version":1,"capabilities":{}}}}'
            printf '%s\n' '{"type":"system","subtype":"init","session_id":"provider-session","model":"test","tools":[]}'
            ;;
        2)
            printf '%s\n' '{"type":"stream_event","event":{"type":"message_start"}}'
            printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}'
            printf '%s\n' '{"type":"stream_event","event":{"type":"message_stop"}}'
            printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"private result must not leak","session_id":"provider-session"}'
            ;;
    esac
done
"#;
    let (mut bridge, identity) = bridge(script, &workspace);
    open(&mut bridge, &identity);

    let opened = bridge
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(opened.sequence, 1);
    assert_eq!(opened.binding_id, identity.binding_id);
    assert_eq!(
        opened.header.correlation.task_id,
        Some(identity.task_id.clone())
    );
    assert_eq!(
        opened.header.correlation.run_id,
        Some(identity.run_id.clone())
    );
    let AgentRuntimeEvent::SessionOpened { binding } = opened.event else {
        panic!("expected session binding")
    };
    assert_eq!(binding.agent_session_id, identity.agent_session_id);
    assert_eq!(binding.runtime_instance_id, identity.runtime_instance_id);
    assert_eq!(binding.runtime_generation, 7);
    assert_eq!(
        binding.external_session.kind,
        ExternalRefKind::ProviderSession
    );
    assert_eq!(binding.external_session.value.as_str(), "provider-session");

    bridge
        .dispatch(
            AgentRuntimeCommand::Prompt {
                run_id: identity.run_id.clone(),
                input: vec![ContentPart::Text {
                    text: BoundedText::new("diagnose").unwrap(),
                }],
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
    let chunk = bridge
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(chunk.sequence, 2);
    assert!(matches!(
        chunk.event,
        AgentRuntimeEvent::MessageChunk {
            content: ContentPart::Text { ref text },
            ..
        } if text.as_str() == "hello"
    ));
    let terminal = bridge
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(terminal.sequence, 3);
    assert!(matches!(
        terminal.event,
        AgentRuntimeEvent::Completed {
            outcome: RunOutcome::Succeeded
        }
    ));
    assert_eq!(
        bridge.next_event(Instant::now() + Duration::from_millis(20)),
        Err(AgentRuntimePortError::Terminal)
    );
    assert!(!format!("{terminal:?}").contains("private result must not leak"));
}

#[cfg(unix)]
#[test]
fn cancellation_reaps_core_and_emits_one_cancelled_terminal() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
IFS= read -r line
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"__INIT_REQUEST_ID__","response":{"subtype":"initialize","protocol_version":1,"capabilities":{}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"provider-session"}'
IFS= read -r line
while :; do sleep 1; done
"#;
    let (mut bridge, identity) = bridge(script, &workspace);
    open(&mut bridge, &identity);
    bridge
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    bridge
        .dispatch(
            AgentRuntimeCommand::Prompt {
                run_id: identity.run_id.clone(),
                input: vec![ContentPart::Text {
                    text: BoundedText::new("wait").unwrap(),
                }],
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();

    let started = Instant::now();
    bridge
        .dispatch(
            AgentRuntimeCommand::Cancel {
                run_id: identity.run_id,
                cause: CancelReason::UserRequested,
            },
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    let terminal = bridge
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        terminal.event,
        AgentRuntimeEvent::Completed {
            outcome: RunOutcome::Cancelled
        }
    ));
    assert_eq!(
        bridge.next_event(Instant::now() + Duration::from_millis(20)),
        Err(AgentRuntimePortError::Terminal)
    );
}

#[cfg(unix)]
#[test]
fn open_deadline_fails_closed_without_provider_payload() {
    let workspace = tempfile::tempdir().unwrap();
    let (mut bridge, identity) = bridge("read -r line; sleep 60", &workspace);
    let result = bridge.dispatch(
        AgentRuntimeCommand::OpenSession {
            task_id: identity.task_id,
            run_id: identity.run_id,
            workspace: workspace_ref(),
        },
        Instant::now() + Duration::from_millis(40),
    );
    assert!(matches!(
        result,
        Err(AgentRuntimePortError::Deadline {
            operation: "open_session"
        })
    ));
    let terminal = bridge
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    let AgentRuntimeEvent::TransportFailed { error } = terminal.event else {
        panic!("expected transport failure")
    };
    assert_eq!(error.code.as_str(), "core_session_open_failed");
    assert_eq!(
        error.safe_message.as_str(),
        "The Agent runtime transport failed"
    );
}

#[cfg(unix)]
#[test]
fn cross_run_commands_are_rejected_before_private_io() {
    let workspace = tempfile::tempdir().unwrap();
    let (mut bridge, identity) = bridge("sleep 60", &workspace);
    let result = bridge.dispatch(
        AgentRuntimeCommand::OpenSession {
            task_id: identity.task_id,
            run_id: RunId::new(),
            workspace: workspace_ref(),
        },
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(result, Err(AgentRuntimePortError::IdentityMismatch));
}

#[cfg(unix)]
#[test]
fn session_open_must_be_delivered_before_prompt_and_idle_cancel() {
    let workspace = tempfile::tempdir().unwrap();
    let script = r#"
IFS= read -r line
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"__INIT_REQUEST_ID__","response":{"subtype":"initialize","protocol_version":1,"capabilities":{}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"provider-session"}'
IFS= read -r line
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"provider-session"}'
"#;
    let (mut bridge, identity) = bridge(script, &workspace);
    open(&mut bridge, &identity);

    let prompt = || AgentRuntimeCommand::Prompt {
        run_id: identity.run_id.clone(),
        input: vec![ContentPart::Text {
            text: BoundedText::new("continue").unwrap(),
        }],
    };
    assert_eq!(
        bridge.dispatch(prompt(), Instant::now() + Duration::from_secs(1)),
        Err(AgentRuntimePortError::InvalidState {
            operation: "prompt",
            state: "session-opened-pending",
        })
    );

    bridge
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        bridge.dispatch(
            AgentRuntimeCommand::Cancel {
                run_id: identity.run_id.clone(),
                cause: CancelReason::UserRequested,
            },
            Instant::now() + Duration::from_secs(1),
        ),
        Err(AgentRuntimePortError::InvalidState {
            operation: "cancel",
            state: "session-open",
        })
    );

    bridge
        .dispatch(prompt(), Instant::now() + Duration::from_secs(1))
        .unwrap();
    let terminal = bridge
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        terminal.event,
        AgentRuntimeEvent::Completed {
            outcome: RunOutcome::Succeeded
        }
    ));
}

#[test]
fn aggregate_prompt_size_is_bounded() {
    let input = vec![
        ContentPart::Text {
            text: BoundedText::new("abc").unwrap(),
        },
        ContentPart::Text {
            text: BoundedText::new("def").unwrap(),
        },
    ];
    assert_eq!(prompt_text(input.clone(), 7).unwrap(), "abc\ndef");
    assert_eq!(prompt_text(input, 6), Err(AgentRuntimePortError::Protocol));
}

#[cfg(unix)]
#[test]
fn tool_identity_retention_is_bounded() {
    let workspace = tempfile::tempdir().unwrap();
    let (mut bridge, _) = bridge("sleep 60", &workspace);
    bridge.current_message = Some(RuntimeMessageId::new());
    for index in 0..MAX_TOOL_USES_PER_TURN {
        bridge
            .tool_ids
            .insert(format!("tool-{index}"), ToolUseId::new());
    }

    let result = bridge.map_stream(CoshCoreStreamEvent::ContentBlockStart {
        index: 0,
        content_block: CoshCoreContentBlockInfo::ToolUse {
            id: "one-too-many".to_owned(),
            name: "shell".to_owned(),
        },
    });
    assert_eq!(result, Err(AgentRuntimePortError::Protocol));
}
