fn prompt_text(input: Vec<ContentPart>) -> Result<String, AgentRuntimePortError> {
    let mut output = String::new();
    for part in input {
        match part {
            ContentPart::Text { text } => {
                let separator = usize::from(!output.is_empty());
                let next_len = output
                    .len()
                    .checked_add(separator)
                    .and_then(|length| length.checked_add(text.as_str().len()))
                    .ok_or(AgentRuntimePortError::Protocol)?;
                if next_len > MAX_TEXT_BYTES {
                    return Err(AgentRuntimePortError::Protocol);
                }
                if separator == 1 {
                    output.push('\n');
                }
                output.push_str(text.as_str());
            }
            ContentPart::ResourceLink { .. } => {
                return Err(AgentRuntimePortError::Unsupported {
                    operation: "resource prompt",
                })
            }
        }
    }
    if output.is_empty() {
        Err(AgentRuntimePortError::Protocol)
    } else {
        Ok(output)
    }
}
fn map_driver_error(error: AcpSessionDriverError) -> AgentRuntimePortError {
    match error {
        AcpSessionDriverError::Deadline { operation } => {
            AgentRuntimePortError::Deadline { operation }
        }
        AcpSessionDriverError::InvalidState { operation, state } => {
            AgentRuntimePortError::InvalidState { operation, state }
        }
        AcpSessionDriverError::Bridge(_)
        | AcpSessionDriverError::ActorUnavailable
        | AcpSessionDriverError::CancellationPending
        | AcpSessionDriverError::ObservationBackpressure
        | AcpSessionDriverError::Cancelled => AgentRuntimePortError::Transport,
        AcpSessionDriverError::InvalidDeadlineConfiguration => AgentRuntimePortError::Protocol,
    }
}
fn safe_error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ContractError {
    ContractError::new(code, category, retryable, message)
        .unwrap_or_else(|_| unreachable!("static contract error must remain valid"))
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
