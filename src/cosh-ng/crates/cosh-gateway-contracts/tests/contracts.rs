use serde::{de::DeserializeOwned, Serialize};

use cosh_gateway_contracts::{
    capability::{
        ApprovalDecision, CapabilityDecision, CapabilityRequest, CapabilityScope, ExecutionPermit,
        OperationDescriptor,
    },
    common::{
        ActorKind, ActorRef, AuthAssurance, BoundedName, BoundedOpaque, BoundedStringError,
        BoundedText, ContentPart, ContractHeader, ContractSchema, Correlation, Digest,
        IdempotencyKey, TargetRef, CONTRACT_SCHEMA_VERSION, MAX_TEXT_BYTES,
    },
    error::{ContractError, ErrorCategory},
    ids::{
        ActorId, AgentSessionId, ApprovalId, ExecutionId, InstallationId, MessageId, PermitId,
        RequestId, RunId, RuntimeBindingId, ShellSessionId, TaskId, ToolUseId,
    },
    runtime::{
        AgentRuntimeCommand, AgentRuntimeEvent, RunOutcome, RuntimeCommandEnvelope,
        RuntimeEventEnvelope,
    },
    task::{
        GatewayCommandEnvelope, TaskCommand, TaskEvent, TaskEventEnvelope, TaskEventKind, TaskState,
    },
};

fn digest(byte: char) -> Digest {
    Digest::parse(byte.to_string().repeat(64)).expect("test digest is canonical")
}

fn target() -> TargetRef {
    TargetRef {
        kind: BoundedName::new("ecs").expect("test name is bounded"),
        authority: BoundedName::new("local").expect("test name is bounded"),
        identifier: BoundedOpaque::new("instance-1").expect("test ID is bounded"),
    }
}

fn actor() -> ActorRef {
    ActorRef {
        actor_id: ActorId::new(),
        actor_kind: ActorKind::Human,
        issuer: BoundedName::new("local-os").expect("test issuer is bounded"),
        assurance: AuthAssurance::LocalOs,
    }
}

fn header(schema: ContractSchema) -> ContractHeader {
    ContractHeader::new(
        schema,
        MessageId::new(),
        1_700_000_000_000,
        Correlation::new(InstallationId::new()),
    )
}

fn assert_schema_mismatch_rejected<T>(value: &T, wrong_schema: &str)
where
    T: Serialize + DeserializeOwned,
{
    let mut json = serde_json::to_value(value).expect("envelope serializes");
    json["header"]["schema"] = serde_json::json!(wrong_schema);
    assert!(serde_json::from_value::<T>(json).is_err());
}

#[test]
fn internal_id_types_reject_cross_parsing() {
    let task_id = TaskId::new();
    assert!(RunId::parse(task_id.as_str()).is_err());
    assert!(RequestId::parse(task_id.as_str()).is_err());
    assert_eq!(
        TaskId::parse(task_id.as_str()).expect("same ID type parses"),
        task_id
    );

    let agent_session_id = AgentSessionId::new();
    assert!(ShellSessionId::parse(agent_session_id.as_str()).is_err());

    let tool_use_id = ToolUseId::new();
    assert!(ExecutionId::parse(tool_use_id.as_str()).is_err());
    assert!(ApprovalId::parse(tool_use_id.as_str()).is_err());
}

#[test]
fn ids_serialize_as_validated_canonical_strings() {
    let task_id = TaskId::new();
    let json = serde_json::to_string(&task_id).expect("ID serializes");
    let decoded: TaskId = serde_json::from_str(&json).expect("canonical ID deserializes");
    assert_eq!(decoded, task_id);
    assert!(
        serde_json::from_str::<TaskId>("\"run_00000000-0000-0000-0000-000000000000\"").is_err()
    );
}

#[test]
fn task_command_and_event_envelopes_round_trip() {
    let command = GatewayCommandEnvelope {
        header: header(ContractSchema::GatewayCommand),
        actor: actor(),
        idempotency_key: IdempotencyKey::new("channel-message-1").expect("test key is bounded"),
        expected_task_revision: None,
        command: TaskCommand::CreateTask {
            intent: BoundedText::new("inspect disk pressure").expect("test text is bounded"),
            target: target(),
        },
    };
    let command_json = serde_json::to_string(&command).expect("command serializes");
    let command_decoded: GatewayCommandEnvelope =
        serde_json::from_str(&command_json).expect("command deserializes");
    assert_eq!(command_decoded, command);
    assert_schema_mismatch_rejected(&command, "cosh.runtime.command");

    let event = TaskEventEnvelope {
        header: header(ContractSchema::TaskEvent),
        task_id: TaskId::new(),
        revision: 1,
        event: TaskEvent::TaskSubmitted {
            intent_digest: digest('a'),
            target: target(),
        },
    };
    let event_json = serde_json::to_string(&event).expect("event serializes");
    let event_decoded: TaskEventEnvelope =
        serde_json::from_str(&event_json).expect("event deserializes");
    assert_eq!(event_decoded, event);
    assert_eq!(event_decoded.event.kind(), TaskEventKind::TaskSubmitted);
    assert_schema_mismatch_rejected(&event, "cosh.gateway.command");

    assert_eq!(
        serde_json::to_string(&TaskState::WaitingApproval).expect("state serializes"),
        "\"waiting_approval\""
    );
    assert_eq!(
        serde_json::to_string(&TaskState::WaitingInput).expect("state serializes"),
        "\"waiting_input\""
    );
}

#[test]
fn runtime_and_capability_contracts_round_trip() {
    let task_id = TaskId::new();
    let run_id = RunId::new();
    let request_id = RequestId::new();
    let request = CapabilityRequest {
        request_id: request_id.clone(),
        task_id: task_id.clone(),
        run_id: run_id.clone(),
        actor: actor(),
        target: target(),
        operation: OperationDescriptor {
            namespace: BoundedName::new("process").expect("test name is bounded"),
            name: BoundedName::new("spawn").expect("test name is bounded"),
            arguments_digest: digest('b'),
        },
        operation_digest: digest('e'),
        requested_scope: CapabilityScope {
            resource: BoundedName::new("host").expect("test name is bounded"),
            access: BoundedName::new("execute").expect("test name is bounded"),
        },
        input_digest: digest('c'),
        expires_at_ms: 1_700_000_001_000,
    };
    let permit = ExecutionPermit {
        permit_id: PermitId::new(),
        request_id,
        actor_id: request.actor.actor_id.clone(),
        approval_id: Some(ApprovalId::new()),
        task_id,
        run_id: run_id.clone(),
        execution_id: ExecutionId::new(),
        target: target(),
        operation_digest: digest('d'),
        input_digest: request.input_digest.clone(),
        policy_revision: 7,
        valid_until_ms: 1_700_000_001_000,
        single_use: true,
    };
    let decision = CapabilityDecision::Permit { permit };
    let decision_json = serde_json::to_string(&decision).expect("decision serializes");
    let decision_decoded: CapabilityDecision =
        serde_json::from_str(&decision_json).expect("decision deserializes");
    assert_eq!(decision_decoded, decision);

    let runtime = RuntimeCommandEnvelope {
        header: header(ContractSchema::RuntimeCommand),
        command: AgentRuntimeCommand::Prompt {
            run_id,
            input: vec![ContentPart::Text {
                text: BoundedText::new("continue").expect("test text is bounded"),
            }],
        },
    };
    let runtime_json = serde_json::to_string(&runtime).expect("Runtime command serializes");
    let runtime_decoded: RuntimeCommandEnvelope =
        serde_json::from_str(&runtime_json).expect("Runtime command deserializes");
    assert_eq!(runtime_decoded, runtime);
    assert_schema_mismatch_rejected(&runtime, "cosh.runtime.event");

    let runtime_event = RuntimeEventEnvelope {
        header: header(ContractSchema::RuntimeEvent),
        binding_id: RuntimeBindingId::new(),
        sequence: 1,
        event: AgentRuntimeEvent::Completed {
            outcome: RunOutcome::Succeeded,
        },
    };
    let runtime_event_json =
        serde_json::to_string(&runtime_event).expect("Runtime event serializes");
    let runtime_event_decoded: RuntimeEventEnvelope =
        serde_json::from_str(&runtime_event_json).expect("Runtime event deserializes");
    assert_eq!(runtime_event_decoded, runtime_event);
    assert_schema_mismatch_rejected(&runtime_event, "cosh.task.event");

    assert_eq!(ApprovalDecision::Approve, ApprovalDecision::Approve);
    assert_eq!(request.task_id.as_str().split('_').next(), Some("tsk"));
}

#[test]
fn contract_header_version_is_independent_and_fail_closed() {
    let supported = header(ContractSchema::GatewayCommand);
    assert_eq!(supported.schema_version, CONTRACT_SCHEMA_VERSION);

    let mut value = serde_json::to_value(supported).expect("header serializes");
    value["schema_version"] = serde_json::json!(CONTRACT_SCHEMA_VERSION + 1);
    assert!(serde_json::from_value::<ContractHeader>(value).is_err());
}

#[test]
fn contract_errors_are_bounded_during_construction_and_deserialization() {
    let error = ContractError::new(
        "runtime_unavailable",
        ErrorCategory::RuntimeUnavailable,
        true,
        "runtime is temporarily unavailable",
    )
    .expect("test error is bounded");
    let json = serde_json::to_string(&error).expect("error serializes");
    let decoded: ContractError = serde_json::from_str(&json).expect("error deserializes");
    assert_eq!(decoded, error);

    assert_eq!(
        BoundedText::new("x".repeat(MAX_TEXT_BYTES + 1)),
        Err(BoundedStringError::TooLong {
            max_bytes: MAX_TEXT_BYTES
        })
    );

    let mut oversized = serde_json::to_value(error).expect("error serializes");
    oversized["safe_message"] = serde_json::json!("x".repeat(MAX_TEXT_BYTES + 1));
    assert!(serde_json::from_value::<ContractError>(oversized).is_err());
}
