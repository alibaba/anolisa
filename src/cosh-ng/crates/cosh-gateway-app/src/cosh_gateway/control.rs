//! Local Task and administration command handlers.

use super::*;

pub(super) fn admin(args: AdminArgs, reporter: &Reporter) -> Result<u8, CliError> {
    match args.command {
        AdminCommand::Inspect(command) => {
            let report = inspect_task_store(command.database)
                .map_err(|error| CliError::StoreInspection(error.to_string()))?;
            let exit = if report.outcome == StoreInspectionOutcome::Healthy {
                0
            } else {
                EXIT_STORE_INSPECTION
            };
            reporter.event(
                "store_inspection",
                serde_json::to_value(report)
                    .map_err(|error| CliError::StoreInspection(error.to_string()))?,
            )?;
            Ok(exit)
        }
    }
}

pub(super) fn task(args: TaskArgs, reporter: &Reporter) -> Result<u8, CliError> {
    let socket = daemon_socket_path(args.socket.as_ref())?;
    let client = LocalGatewayClient::new(socket);
    let result = match args.command {
        TaskCommand::Submit(command) => {
            let request = SubmitTask {
                request_id: RequestId::new(),
                idempotency_key: IdempotencyKey::new(command.idempotency_key)
                    .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                intent: BoundedText::new(read_intent(command.intent_file.as_ref())?)
                    .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                target: target_for_runtime_profile(&command.runtime_profile)?,
                runtime: RuntimeSelector {
                    runtime: bounded_name(command.runtime)?,
                    profile: Some(bounded_name(command.runtime_profile)?),
                },
            };
            client.submit(request)
        }
        TaskCommand::List(command) => client.list(RequestId::new(), command.limit),
        TaskCommand::Get(command) => client.get(RequestId::new(), parse_task(&command.task_id)?),
        TaskCommand::Events(command) => client.events(
            RequestId::new(),
            parse_task(&command.task_id)?,
            (command.after != 0).then_some(command.after),
            command.limit,
        ),
        TaskCommand::Cancel(command) => client.cancel(CancelTask {
            request_id: RequestId::new(),
            idempotency_key: IdempotencyKey::new(command.idempotency_key)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            task_id: parse_task(&command.task_id)?,
            run_id: RunId::parse(&command.run_id)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            expected_revision: command.expected_revision,
        }),
        TaskCommand::Retry(command) => client.retry(RetryTask {
            request_id: RequestId::new(),
            idempotency_key: IdempotencyKey::new(command.idempotency_key)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            task_id: parse_task(&command.task_id)?,
            previous_run_id: RunId::parse(&command.previous_run_id)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            expected_revision: command.expected_revision,
        }),
        TaskCommand::ResolveApproval(command) => client.resolve_approval(ResolveApproval {
            request_id: RequestId::new(),
            idempotency_key: IdempotencyKey::new(command.idempotency_key)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            approval_id: ApprovalId::parse(&command.approval_id)
                .map_err(|error| CliError::InvalidInput(error.to_string()))?,
            decision: command.decision.into(),
        }),
        TaskCommand::Append(command) => {
            let response = if command.selections.is_empty() {
                RuntimeInputResponse::Text {
                    text: BoundedText::new(read_intent(command.input_file.as_ref())?)
                        .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                }
            } else {
                RuntimeInputResponse::Options {
                    selections: RuntimeInputSelections::new(command.selections)
                        .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                }
            };
            client.append_input(AppendTaskInput {
                request_id: RequestId::new(),
                idempotency_key: IdempotencyKey::new(command.idempotency_key)
                    .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                task_id: parse_task(&command.task_id)?,
                input_request_id: InputRequestId::parse(&command.input_request_id)
                    .map_err(|error| CliError::InvalidInput(error.to_string()))?,
                response,
                expected_revision: command.expected_revision,
            })
        }
    }
    .map_err(|error| CliError::Daemon(error.to_string()))?;
    report_gateway_result(reporter, result)?;
    Ok(0)
}

fn report_gateway_result(reporter: &Reporter, result: GatewayResult) -> Result<(), CliError> {
    match result {
        GatewayResult::Pong => reporter.event("daemon_pong", json!({})),
        GatewayResult::Task(task) => reporter.event(
            "task",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::Tasks(tasks) => reporter.event(
            "tasks",
            serde_json::to_value(tasks).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::Events(events) => reporter.event(
            "task_events",
            serde_json::to_value(events).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::Cancelled(task) => reporter.event(
            "task_cancelled",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::Retried(task) => reporter.event(
            "task_retried",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::InputAppended(task) => reporter.event(
            "task_input_appended",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
        GatewayResult::ApprovalResolved(task) => reporter.event(
            "approval_resolved",
            serde_json::to_value(task).map_err(|error| CliError::Daemon(error.to_string()))?,
        ),
    }
}

fn bounded_name(value: String) -> Result<BoundedName, CliError> {
    BoundedName::new(value).map_err(|error| CliError::InvalidInput(error.to_string()))
}

pub(super) fn task_only_target() -> TargetRef {
    GatewayCapabilityProfile::task_only_v1().governed_target()
}

pub(super) fn target_for_runtime_profile(profile: &str) -> Result<TargetRef, CliError> {
    match profile {
        GATEWAY_BROKERED_CORE_RUNTIME_PROFILE => Ok(task_only_target()),
        GATEWAY_CHECKPOINT_CORE_RUNTIME_PROFILE => {
            Ok(GatewayCapabilityProfile::workspace_checkpoint_v1().governed_target())
        }
        "codex" | "claude-code" => {
            Ok(GatewayCapabilityProfile::delegated_acp_v1().governed_target())
        }
        _ => Err(CliError::Profile(
            "unsupported durable Task Runtime profile".to_owned(),
        )),
    }
}

fn parse_task(value: &str) -> Result<TaskId, CliError> {
    TaskId::parse(value).map_err(|error| CliError::InvalidInput(error.to_string()))
}
