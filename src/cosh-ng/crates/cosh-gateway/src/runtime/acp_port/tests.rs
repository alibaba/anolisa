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
        ActorId, AgentSessionId, ApprovalId, InputRequestId, InstallationId, RequestId, RunId,
        RuntimeBindingId, RuntimeInstanceId, TaskId, TurnId,
    },
    runtime::{
        AgentRuntimeCommand, AgentRuntimeEvent, BrokeredRequestAcknowledgement, ExecutionAuthority,
        RuntimeInputResponse, RuntimePermissionDecision, ToolInvocationStatus, TurnLimit,
        TurnOutcome,
    },
};
use serde_json::json;

use super::*;
use crate::runtime::{
    AcpSessionObservation, AcpSessionTerminal, AcpV1ClientConfig, AcpV1PermissionOption,
    RuntimeLaunchSpec,
};

#[derive(Default)]
struct FakeState {
    events: VecDeque<AcpSessionEvent>,
    prompts: Vec<String>,
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
    fn prompt(&self, text: String) -> Result<(), AcpSessionDriverError> {
        self.0.lock().unwrap().prompts.push(text);
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

fn observed(observation: AcpV1Observation) -> AcpSessionEvent {
    // Driver-local ordering is intentionally independent from RuntimeEventEnvelope ordering.
    AcpSessionEvent::Observation(AcpSessionObservation::new(99, observation))
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

fn prompt(port: &mut AcpAgentRuntime, identity: &AcpAgentRuntimeIdentity) -> TurnId {
    let turn_id = TurnId::new();
    port.dispatch(
        AgentRuntimeCommand::Prompt {
            run_id: identity.run_id.clone(),
            turn_id: turn_id.clone(),
            input: vec![ContentPart::Text {
                text: BoundedText::new("inspect").unwrap(),
            }],
        },
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    let started = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        started.event,
        AgentRuntimeEvent::TurnStarted { turn_id: observed } if observed == turn_id
    ));
    turn_id
}

#[test]
fn maps_bounded_text_and_exactly_one_terminal_without_provider_ids() {
    let events = vec![
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
            session_id: "provider-secret-session".into(),
        }),
        observed(AcpV1Observation::SessionUpdate {
            session_id: "provider-secret-session".into(),
            update: json!({"sessionUpdate":"agent_message_chunk","messageId":"provider-message","content":{"type":"text","text":"hello"}}),
        }),
        observed(AcpV1Observation::PromptFinished {
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
    let turn_id = prompt(&mut port, &identity);
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
            turn_id: observed,
            outcome: TurnOutcome::Completed
        } if observed == turn_id
    ));
    assert!(!state.lock().unwrap().shutdown);
    assert_eq!(
        port.next_event(Instant::now() + Duration::from_millis(1)),
        Err(AgentRuntimePortError::InvalidState {
            operation: "next_event",
            state: "session-open"
        })
    );
}

#[test]
fn limit_result_keeps_session_open_for_another_turn() {
    let events = vec![
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        observed(AcpV1Observation::PromptFinished {
            session_id: "session".into(),
            stop_reason: AcpV1StopReason::MaxTokens,
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
    let first_turn = prompt(&mut port, &identity);
    let limited = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        limited.event,
        AgentRuntimeEvent::Completed {
            turn_id: observed,
            outcome: TurnOutcome::LimitReached {
                limit: TurnLimit::Tokens
            }
        } if observed == first_turn
    ));
    assert!(!state.lock().unwrap().shutdown);

    state
        .lock()
        .unwrap()
        .events
        .push_back(observed(AcpV1Observation::PromptFinished {
            session_id: "session".into(),
            stop_reason: AcpV1StopReason::MaxTurnRequests,
        }));
    let second_turn = prompt(&mut port, &identity);
    let completed = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        completed.event,
        AgentRuntimeEvent::Completed {
            turn_id: observed,
            outcome: TurnOutcome::LimitReached {
                limit: TurnLimit::Requests
            }
        } if observed == second_turn
    ));
}

#[test]
fn tool_updates_emit_stable_bounded_snapshots() {
    let events = vec![
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        observed(AcpV1Observation::SessionUpdate {
            session_id: "session".into(),
            update: json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "tool-1",
                "title": "Run tests",
                "kind": "execute"
            }),
        }),
        observed(AcpV1Observation::SessionUpdate {
            session_id: "session".into(),
            update: json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tool-1",
                "status": "completed",
                "rawOutput": {"exitCode": 0}
            }),
        }),
    ];
    let (mut port, _, identity) = test_port(
        events,
        Normalizer {
            request_id: RequestId::new(),
            mismatch: false,
        },
    );
    open(&mut port, &identity);
    port.next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    let turn_id = prompt(&mut port, &identity);

    let created = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    let AgentRuntimeEvent::ToolInvocationUpdated { snapshot: created } = created.event else {
        panic!("expected initial tool snapshot");
    };
    assert_eq!(created.turn_id, turn_id);
    assert_eq!(created.revision, 1);
    assert_eq!(created.status, ToolInvocationStatus::Pending);
    assert_eq!(
        created.authority,
        ExecutionAuthority::ProviderNativeObserved
    );

    let updated = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    let AgentRuntimeEvent::ToolInvocationUpdated { snapshot: updated } = updated.event else {
        panic!("expected updated tool snapshot");
    };
    assert_eq!(updated.tool_use_id, created.tool_use_id);
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.status, ToolInvocationStatus::Completed);
}

#[test]
fn correlates_provider_native_approval_only_to_offered_allow_once() {
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
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        observed(AcpV1Observation::PermissionRequested(permission)),
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
    assert!(matches!(
        event.event,
        AgentRuntimeEvent::ExecutionPermissionRequested { ref request, .. }
            if request.request_id == request_id
    ));
    port.dispatch(
        AgentRuntimeCommand::ResolvePermission {
            request_id,
            decision: RuntimePermissionDecision::ProviderNativeAllowOnce,
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
        tool_call: json!({"toolCallId":"tool","title":"run"}),
        options: vec![AcpV1PermissionOption {
            option_id: "reject".into(),
            name: "Reject once".into(),
            kind: AcpV1PermissionOptionKind::RejectOnce,
        }],
    };
    let events = vec![
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        observed(AcpV1Observation::PermissionRequested(permission)),
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
fn brokered_takeover_is_rejected_without_answering_provider() {
    let request_id = RequestId::new();
    let permission = AcpV1PermissionRequest {
        request_id: AcpV1RequestId::String("permission".into()),
        session_id: "session".into(),
        tool_call: json!({"toolCallId":"tool","title":"run"}),
        options: vec![AcpV1PermissionOption {
            option_id: "always".into(),
            name: "Always".into(),
            kind: AcpV1PermissionOptionKind::AllowAlways,
        }],
    };
    let events = vec![
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        observed(AcpV1Observation::PermissionRequested(permission)),
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
        AgentRuntimeCommand::AcknowledgeBrokeredRequest {
            acknowledgement: BrokeredRequestAcknowledgement {
                request_id,
                approval_id: ApprovalId::new(),
            },
        },
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(
        result,
        Err(AgentRuntimePortError::Unsupported {
            operation: "COSH-brokered execution over ACP"
        })
    );
    assert!(state.lock().unwrap().answers.is_empty());
}

#[test]
fn cancellation_waits_for_terminal_before_public_completion() {
    let events = vec![
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
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
    let turn_id = prompt(&mut port, &identity);

    port.dispatch(
        AgentRuntimeCommand::Cancel {
            run_id: identity.run_id,
            turn_id: turn_id.clone(),
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
            turn_id: observed,
            outcome: TurnOutcome::Cancelled
        } if observed == turn_id
    ));
    assert_eq!(
        port.next_event(Instant::now() + Duration::from_millis(1)),
        Err(AgentRuntimePortError::Terminal)
    );
}

#[test]
fn completion_and_cancel_races_settle_once_and_late_permission_cannot_answer() {
    let completed_events = vec![
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        observed(AcpV1Observation::PromptFinished {
            session_id: "session".into(),
            stop_reason: AcpV1StopReason::EndTurn,
        }),
    ];
    let (mut completed, _, completed_identity) = test_port(
        completed_events,
        Normalizer {
            request_id: RequestId::new(),
            mismatch: false,
        },
    );
    open(&mut completed, &completed_identity);
    completed
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    let completed_turn = prompt(&mut completed, &completed_identity);
    let terminal = completed
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        terminal.event,
        AgentRuntimeEvent::Completed {
            turn_id,
            outcome: TurnOutcome::Completed
        } if turn_id == completed_turn
    ));
    assert!(matches!(
        completed.dispatch(
            AgentRuntimeCommand::Cancel {
                run_id: completed_identity.run_id,
                turn_id: completed_turn,
                cause: cosh_gateway_contracts::task::CancelReason::UserRequested,
            },
            Instant::now() + Duration::from_secs(1),
        ),
        Err(AgentRuntimePortError::InvalidState { .. })
    ));

    let request_id = RequestId::new();
    let permission = AcpV1PermissionRequest {
        request_id: AcpV1RequestId::Number(91),
        session_id: "session".into(),
        tool_call: json!({"toolCallId":"tool","title":"run"}),
        options: vec![AcpV1PermissionOption {
            option_id: "allow".into(),
            name: "Allow once".into(),
            kind: AcpV1PermissionOptionKind::AllowOnce,
        }],
    };
    let cancelled_events = vec![
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
            session_id: "session".into(),
        }),
        observed(AcpV1Observation::PermissionRequested(permission)),
    ];
    let (mut cancelled, state, cancelled_identity) = test_port(
        cancelled_events,
        Normalizer {
            request_id: request_id.clone(),
            mismatch: false,
        },
    );
    open(&mut cancelled, &cancelled_identity);
    cancelled
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    let cancelled_turn = prompt(&mut cancelled, &cancelled_identity);
    cancelled
        .dispatch(
            AgentRuntimeCommand::Cancel {
                run_id: cancelled_identity.run_id,
                turn_id: cancelled_turn.clone(),
                cause: cosh_gateway_contracts::task::CancelReason::UserRequested,
            },
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
    assert!(matches!(
        cancelled.dispatch(
            AgentRuntimeCommand::ResolvePermission {
                request_id,
                decision: RuntimePermissionDecision::ProviderNativeAllowOnce,
            },
            Instant::now() + Duration::from_secs(1),
        ),
        Err(AgentRuntimePortError::InvalidState { .. })
    ));
    assert!(state.lock().unwrap().answers.is_empty());
    let terminal = cancelled
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        terminal.event,
        AgentRuntimeEvent::Completed {
            turn_id,
            outcome: TurnOutcome::Cancelled
        } if turn_id == cancelled_turn
    ));
    assert_eq!(
        cancelled.next_event(Instant::now() + Duration::from_millis(1)),
        Err(AgentRuntimePortError::Terminal)
    );
}

#[test]
fn unsupported_resume_rich_content_and_second_session_do_not_reach_backend() {
    let events = vec![
        observed(AcpV1Observation::Initialized {
            agent_info: None,
            capabilities: Default::default(),
        }),
        observed(AcpV1Observation::SessionOpened {
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
    let opened = port
        .next_event(Instant::now() + Duration::from_secs(1))
        .unwrap();
    let AgentRuntimeEvent::SessionOpened { binding } = opened.event else {
        panic!("expected opened session")
    };

    assert_eq!(
        port.dispatch(
            AgentRuntimeCommand::ResumeSession {
                task_id: identity.task_id.clone(),
                run_id: identity.run_id.clone(),
                binding,
            },
            Instant::now() + Duration::from_secs(1),
        ),
        Err(AgentRuntimePortError::Unsupported {
            operation: "resume_session"
        })
    );
    assert!(matches!(
        port.dispatch(
            AgentRuntimeCommand::OpenSession {
                task_id: identity.task_id.clone(),
                run_id: identity.run_id.clone(),
                workspace: workspace(),
            },
            Instant::now() + Duration::from_secs(1),
        ),
        Err(AgentRuntimePortError::InvalidState { .. })
    ));
    assert_eq!(
        port.dispatch(
            AgentRuntimeCommand::ResolveInput {
                request_id: InputRequestId::new(),
                run_id: identity.run_id.clone(),
                turn_id: TurnId::new(),
                response: RuntimeInputResponse::Text {
                    text: BoundedText::new("must stay local").unwrap(),
                },
            },
            Instant::now() + Duration::from_secs(1),
        ),
        Err(AgentRuntimePortError::Unsupported {
            operation: "resolve_input"
        })
    );
    assert_eq!(
        port.dispatch(
            AgentRuntimeCommand::Prompt {
                run_id: identity.run_id,
                turn_id: TurnId::new(),
                input: vec![ContentPart::ResourceLink {
                    uri: BoundedOpaque::new("file:///forbidden").unwrap(),
                    label: None,
                }],
            },
            Instant::now() + Duration::from_secs(1),
        ),
        Err(AgentRuntimePortError::Unsupported {
            operation: "resource prompt"
        })
    );
    assert!(state.lock().unwrap().prompts.is_empty());
}
