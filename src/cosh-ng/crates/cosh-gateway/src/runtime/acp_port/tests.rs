//! Fake-ACP coverage for identity, permission, and terminal mapping.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cosh_gateway_contracts::{
    capability::{CapabilityRequest, CapabilityScope, OperationDescriptor},
    common::{
        ActorKind, AuthAssurance, BoundedName, BoundedOpaque, BoundedText, ContentPart, Digest,
        TargetRef, WorkspaceRef,
    },
    ids::{
        ActorId, AgentSessionId, InstallationId, PermitId, RequestId, RunId, RuntimeBindingId,
        RuntimeInstanceId, TaskId,
    },
    runtime::{AgentRuntimeCommand, AgentRuntimeEvent, RunOutcome, RuntimePermissionDecision},
};
use serde_json::json;

use super::*;
use crate::runtime::{
    AcpSessionTerminal, AcpV1ClientConfig, AcpV1PermissionOption, RuntimeLaunchSpec,
};

#[derive(Default)]
struct FakeState {
    events: VecDeque<AcpSessionEvent>,
    answers: Vec<(AcpV1RequestId, AcpV1PermissionDecision)>,
    cancelled: bool,
    shutdown: bool,
}

struct FakeBackend(Arc<Mutex<FakeState>>);

impl AcpSessionBackend for FakeBackend {
    fn initialize(&self) -> Result<(), AcpSessionDriverError> {
        Ok(())
    }
    fn open_session(&self) -> Result<(), AcpSessionDriverError> {
        Ok(())
    }
    fn prompt(&self, _text: String) -> Result<(), AcpSessionDriverError> {
        Ok(())
    }
    fn answer_permission(
        &self,
        request_id: AcpV1RequestId,
        decision: AcpV1PermissionDecision,
    ) -> Result<(), AcpSessionDriverError> {
        self.0.lock().unwrap().answers.push((request_id, decision));
        Ok(())
    }
    fn receive_timeout(
        &self,
        _timeout: Duration,
    ) -> Result<AcpSessionEvent, std::sync::mpsc::RecvTimeoutError> {
        self.0
            .lock()
            .unwrap()
            .events
            .pop_front()
            .ok_or(std::sync::mpsc::RecvTimeoutError::Timeout)
    }
    fn cancel(&self) -> Result<(), AcpSessionDriverError> {
        let mut state = self.0.lock().unwrap();
        state.cancelled = true;
        state
            .events
            .push_back(AcpSessionEvent::Terminal(AcpSessionTerminal {
                kind: AcpSessionTerminalKind::Cancelled,
                detail: None,
                process: None,
            }));
        Ok(())
    }
    fn shutdown(&self) -> Result<(), AcpSessionDriverError> {
        self.0.lock().unwrap().shutdown = true;
        Ok(())
    }
}

struct Normalizer {
    request_id: RequestId,
    mismatch: bool,
}

impl AcpPermissionNormalizer for Normalizer {
    fn normalize(
        &mut self,
        _request: &AcpV1PermissionRequest,
        context: &AcpPermissionContext,
    ) -> Result<CapabilityRequest, AgentRuntimePortError> {
        Ok(CapabilityRequest {
            request_id: self.request_id.clone(),
            task_id: if self.mismatch {
                TaskId::new()
            } else {
                context.task_id.clone()
            },
            run_id: context.run_id.clone(),
            actor: context.actor.clone(),
            target: TargetRef {
                kind: BoundedName::new("local").unwrap(),
                authority: BoundedName::new("cosh").unwrap(),
                identifier: BoundedOpaque::new("workspace").unwrap(),
            },
            operation: OperationDescriptor {
                namespace: BoundedName::new("process").unwrap(),
                name: BoundedName::new("spawn").unwrap(),
                arguments_digest: digest('2'),
            },
            operation_digest: digest('3'),
            requested_scope: CapabilityScope {
                resource: BoundedName::new("process").unwrap(),
                access: BoundedName::new("execute").unwrap(),
            },
            input_digest: digest('4'),
            expires_at_ms: u64::MAX,
        })
    }
}

fn digest(character: char) -> Digest {
    Digest::parse(character.to_string().repeat(64)).unwrap()
}

fn workspace() -> WorkspaceRef {
    WorkspaceRef {
        scope_digest: digest('0'),
        display_name: Some(BoundedText::new("workspace").unwrap()),
    }
}

fn test_port(
    events: Vec<AcpSessionEvent>,
    normalizer: Normalizer,
) -> (
    AcpAgentRuntime,
    Arc<Mutex<FakeState>>,
    AcpAgentRuntimeIdentity,
) {
    let actor = ActorRef {
        actor_id: ActorId::new(),
        actor_kind: ActorKind::Human,
        issuer: BoundedName::new("local-os").unwrap(),
        assurance: AuthAssurance::LocalOs,
    };
    let identity = AcpAgentRuntimeIdentity {
        installation_id: InstallationId::new(),
        actor,
        task_id: TaskId::new(),
        run_id: RunId::new(),
        agent_session_id: AgentSessionId::new(),
        binding_id: RuntimeBindingId::new(),
        runtime_instance_id: RuntimeInstanceId::new(),
        runtime_generation: 9,
        adapter_authority: BoundedName::new("codex-acp").unwrap(),
        connection_scope_digest: digest('1'),
    };
    let mut launch = RuntimeLaunchSpec::new("/bin/false", Path::new("/"));
    launch.stdout_line_limit = 64 * 1024;
    let session = AcpSessionDriverConfig::new(
        launch,
        AcpV1ClientConfig::new("test", "1", 64 * 1024),
        "/workspace",
    );
    let config = AcpAgentRuntimeConfig {
        session,
        workspace: workspace(),
        identity: identity.clone(),
    };
    let state = Arc::new(Mutex::new(FakeState {
        events: events.into(),
        ..FakeState::default()
    }));
    let port = AcpAgentRuntime::with_backend(
        config,
        Box::new(normalizer),
        Box::new(FakeBackend(state.clone())),
    );
    (port, state, identity)
}

fn open(port: &mut AcpAgentRuntime, identity: &AcpAgentRuntimeIdentity) {
    port.dispatch(
        AgentRuntimeCommand::OpenSession {
            task_id: identity.task_id.clone(),
            run_id: identity.run_id.clone(),
            workspace: workspace(),
        },
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
}

fn prompt(port: &mut AcpAgentRuntime, identity: &AcpAgentRuntimeIdentity) {
    port.dispatch(
        AgentRuntimeCommand::Prompt {
            run_id: identity.run_id.clone(),
            input: vec![ContentPart::Text {
                text: BoundedText::new("inspect").unwrap(),
            }],
        },
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
}

#[test]
fn maps_bounded_text_and_exactly_one_terminal_without_provider_ids() {
    let events = vec![
        AcpSessionEvent::Observation(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::SessionOpened {
            session_id: "provider-secret-session".into(),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::SessionUpdate {
            session_id: "provider-secret-session".into(),
            update: json!({"sessionUpdate":"agent_message_chunk","messageId":"provider-message","content":{"type":"text","text":"hello"}}),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::PromptFinished {
            session_id: "provider-secret-session".into(),
            stop_reason: AcpV1StopReason::EndTurn,
        }),
    ];
    let (mut port, state, identity) = test_port(
        events,
        Normalizer {
            request_id: RequestId::new(),
            mismatch: false,
        },
    );
    open(&mut port, &identity);
    let opened = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(opened.sequence, 1);
    prompt(&mut port, &identity);
    let chunk = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(
        matches!(chunk.event, AgentRuntimeEvent::MessageChunk { content: ContentPart::Text { ref text }, .. } if text.as_str() == "hello")
    );
    let encoded = serde_json::to_string(&chunk).unwrap();
    assert!(!encoded.contains("provider-secret-session"));
    assert!(!encoded.contains("provider-message"));
    let terminal = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        terminal.event,
        AgentRuntimeEvent::Completed {
            outcome: RunOutcome::Succeeded
        }
    ));
    assert!(state.lock().unwrap().shutdown);
    assert_eq!(
        port.next_event(Instant::now() + Duration::from_millis(1)),
        Err(AgentRuntimePortError::Terminal)
    );
}

#[test]
fn correlates_broker_permit_only_to_offered_allow_once() {
    let request_id = RequestId::new();
    let permission = AcpV1PermissionRequest {
        request_id: AcpV1RequestId::String("acp-request".into()),
        session_id: "session".into(),
        tool_call: json!({"toolCallId":"provider-tool","title":"run"}),
        options: vec![
            AcpV1PermissionOption {
                option_id: "allow".into(),
                name: "Allow once".into(),
                kind: AcpV1PermissionOptionKind::AllowOnce,
            },
            AcpV1PermissionOption {
                option_id: "always".into(),
                name: "Always".into(),
                kind: AcpV1PermissionOptionKind::AllowAlways,
            },
        ],
    };
    let events = vec![
        AcpSessionEvent::Observation(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::PermissionRequested(permission)),
    ];
    let (mut port, state, identity) = test_port(
        events,
        Normalizer {
            request_id: request_id.clone(),
            mismatch: false,
        },
    );
    open(&mut port, &identity);
    port.next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    prompt(&mut port, &identity);
    let event = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(
        matches!(event.event, AgentRuntimeEvent::PermissionRequested { ref request } if request.request_id == request_id)
    );
    port.dispatch(
        AgentRuntimeCommand::ResolvePermission {
            request_id,
            decision: RuntimePermissionDecision::Permit {
                permit_id: PermitId::new(),
            },
        },
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(
        state.lock().unwrap().answers,
        vec![(
            AcpV1RequestId::String("acp-request".into()),
            AcpV1PermissionDecision::Selected {
                option_id: "allow".into()
            }
        )]
    );
}

#[test]
fn rejects_normalizer_identity_substitution_and_settles_transport() {
    let permission = AcpV1PermissionRequest {
        request_id: AcpV1RequestId::Number(7),
        session_id: "session".into(),
        tool_call: json!({"toolCallId":"tool"}),
        options: vec![AcpV1PermissionOption {
            option_id: "reject".into(),
            name: "Reject once".into(),
            kind: AcpV1PermissionOptionKind::RejectOnce,
        }],
    };
    let events = vec![
        AcpSessionEvent::Observation(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::PermissionRequested(permission)),
    ];
    let (mut port, state, identity) = test_port(
        events,
        Normalizer {
            request_id: RequestId::new(),
            mismatch: true,
        },
    );
    open(&mut port, &identity);
    port.next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    prompt(&mut port, &identity);
    let event = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        event.event,
        AgentRuntimeEvent::TransportFailed { .. }
    ));
    assert!(state.lock().unwrap().cancelled);
}

#[test]
fn missing_allow_once_is_cancelled_without_selecting_durable_permission() {
    let request_id = RequestId::new();
    let permission = AcpV1PermissionRequest {
        request_id: AcpV1RequestId::String("permission".into()),
        session_id: "session".into(),
        tool_call: json!({"toolCallId":"tool"}),
        options: vec![AcpV1PermissionOption {
            option_id: "always".into(),
            name: "Always".into(),
            kind: AcpV1PermissionOptionKind::AllowAlways,
        }],
    };
    let events = vec![
        AcpSessionEvent::Observation(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::PermissionRequested(permission)),
    ];
    let (mut port, state, identity) = test_port(
        events,
        Normalizer {
            request_id: request_id.clone(),
            mismatch: false,
        },
    );
    open(&mut port, &identity);
    port.next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    prompt(&mut port, &identity);
    port.next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();

    let result = port.dispatch(
        AgentRuntimeCommand::ResolvePermission {
            request_id,
            decision: RuntimePermissionDecision::Permit {
                permit_id: PermitId::new(),
            },
        },
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(
        result,
        Err(AgentRuntimePortError::Unsupported {
            operation: "one-shot permission option"
        })
    );
    assert_eq!(
        state.lock().unwrap().answers,
        vec![(
            AcpV1RequestId::String("permission".into()),
            AcpV1PermissionDecision::Cancelled
        )]
    );
}

#[test]
fn cancellation_waits_for_terminal_before_public_completion() {
    let events = vec![
        AcpSessionEvent::Observation(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        AcpSessionEvent::Observation(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
    ];
    let (mut port, state, identity) = test_port(
        events,
        Normalizer {
            request_id: RequestId::new(),
            mismatch: false,
        },
    );
    open(&mut port, &identity);
    port.next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    prompt(&mut port, &identity);

    port.dispatch(
        AgentRuntimeCommand::Cancel {
            run_id: identity.run_id,
            cause: cosh_gateway_contracts::task::CancelReason::UserRequested,
        },
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    assert!(state.lock().unwrap().cancelled);
    let terminal = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        terminal.event,
        AgentRuntimeEvent::Completed {
            outcome: RunOutcome::Cancelled
        }
    ));
    assert_eq!(
        port.next_event(Instant::now() + Duration::from_millis(1)),
        Err(AgentRuntimePortError::Terminal)
    );
}
