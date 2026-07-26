//! Tool-argument admission and the interactive-question dispatch.
//!
//! Everything here decides whether a provider-issued call may run at all:
//! argument parsing, bounded audit shapes and digests, the rejection path, and
//! the two routes that can reach the user with a question (the
//! `ask_user_question` tool call and the in-band `COSH_QUESTION:` text).
//!
//! It lives beside `core.rs` rather than inside it because the turn loop is
//! already at its size budget, and because these are the checks a reviewer wants
//! to read as one unit: a gap between them is how a malformed call became a
//! valid-looking prompt.

use std::io::Write;
use std::time::Instant;

use tokio::io::AsyncBufReadExt;

use cosh_types::audit::AuditToolData;

use crate::audit::CoreAuditScope;
use crate::protocol::OutputMessage;
use crate::tool::ask_user_question::{
    self, AskUserArgumentError, AskUserQuestionParams, AskUserRejectionDiagnostics,
};
use crate::tool::ToolResult;

use super::{CoshCore, PendingToolCall};

/// Marker introducing an in-band question inside assistant text.
const IN_BAND_MARKER: &str = "COSH_QUESTION:";

/// What the assistant's plain text carries on the in-band question route.
///
/// The marker suppresses ordinary text output, so an invalid payload must be
/// distinguishable from "no question at all" — otherwise the turn would end
/// silently, showing the user neither a question nor a reason.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum InBandQuestion {
    /// No marker in the text: an ordinary assistant reply.
    Absent,
    /// A payload that passed the same validation as the tool call.
    Valid(AskUserQuestionParams),
    /// A payload that must be surfaced as a failure, never as a question.
    Invalid(AskUserArgumentError),
}

/// Classify assistant text that may carry an in-band question.
///
/// Shares [`ask_user_question::validate_value`] with the tool-call route, so
/// neither can produce a question the user is unable to answer.
pub(super) fn parse_in_band_question(text: &str) -> InBandQuestion {
    let Some((_, after_marker)) = text.split_once(IN_BAND_MARKER) else {
        return InBandQuestion::Absent;
    };
    let Some(json_text) = after_marker.trim().lines().next().map(str::trim) else {
        return InBandQuestion::Invalid(AskUserArgumentError::EmptyArguments);
    };
    if json_text.is_empty() {
        return InBandQuestion::Invalid(AskUserArgumentError::EmptyArguments);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json_text) else {
        return InBandQuestion::Invalid(AskUserArgumentError::InvalidJson);
    };
    match ask_user_question::validate_value(&value) {
        Ok(params) => InBandQuestion::Valid(params),
        Err(error) => InBandQuestion::Invalid(error),
    }
}

/// Turn-terminating message for an in-band question that failed validation.
///
/// Carries the stable code and the expected shape only: the payload may hold
/// session content.
pub(super) fn in_band_question_error(error: AskUserArgumentError) -> String {
    format!(
        "Provider emitted an invalid {IN_BAND_MARKER} payload [code={}]: {}. No question was shown.",
        error.code(),
        error.guidance()
    )
}

/// Why a tool call's arguments were refused before execution.
///
/// Both variants carry shapes and codes only — never the payload, which can hold
/// session content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ArgumentError {
    /// The payload was not valid JSON.
    InvalidJson,
    /// The payload parsed, but its root was not the declared object.
    RootNotObject { shape: &'static str },
}

impl ArgumentError {
    /// Stable code for logs, audit reasons, and the tool-result text.
    pub(super) fn code(&self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::RootNotObject { .. } => "arguments_not_object",
        }
    }

    /// JSON parse status recorded for the call, matching the ask-user codes.
    pub(super) fn json_parse_status(&self) -> &'static str {
        match self {
            Self::InvalidJson => ask_user_question::JSON_PARSE_INVALID,
            // The bytes did parse; only the shape was wrong.
            Self::RootNotObject { .. } => ask_user_question::JSON_PARSE_OK,
        }
    }

    /// Audit `input_shape` for a rejected call.
    pub(super) fn audit_shape(&self) -> &'static str {
        match self {
            Self::InvalidJson => "unparsed",
            Self::RootNotObject { shape } => shape,
        }
    }

    /// One clause naming what was wrong, safe to show the model.
    fn summary(&self) -> String {
        match self {
            Self::InvalidJson => "arguments were not valid JSON".to_string(),
            Self::RootNotObject { shape } => {
                format!("arguments were a JSON {shape}, not an object")
            }
        }
    }
}

impl std::fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.summary())
    }
}

/// Parse tool arguments as received from the provider.
///
/// Empty or whitespace-only arguments mean "no arguments" — the convention
/// providers use for zero-parameter tools — and become an empty object. Every
/// tool declares an object root, so a payload that parses to `null`, an array, or
/// a scalar is refused rather than passed through: `null` makes every field look
/// merely absent to the tool implementation, and the other roots would reach an
/// MCP server as arguments it never declared.
///
/// # Errors
///
/// Returns [`ArgumentError`] when non-empty arguments are not valid JSON, or
/// when they parse to anything other than an object.
pub(super) fn parse_tool_arguments(raw: &str) -> Result<serde_json::Value, ArgumentError> {
    if raw.trim().is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| ArgumentError::InvalidJson)?;
    if !value.is_object() {
        return Err(ArgumentError::RootNotObject {
            shape: json_shape(&value),
        });
    }
    Ok(value)
}

/// Tool-result text for a tool call whose arguments were refused.
///
/// Carries no fragment of the rejected payload: malformed arguments can still
/// contain session content.
pub(super) fn invalid_arguments_message(tool_name: &str, error: &ArgumentError) -> String {
    format!(
        "{tool_name} arguments rejected [code={}]: {}. \
         The tool was not executed; re-issue the call with one complete JSON object matching \
         the declared schema.",
        error.code(),
        error.summary()
    )
}

pub(super) fn json_shape(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

pub(super) fn hash_json(value: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    hash_bytes(&bytes)
}

pub(super) fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

impl CoshCore {
    /// Run one `ask_user_question` tool call, or reject its arguments.
    ///
    /// Returns the tool result when a question was actually shown, and `None`
    /// when the call was rejected before execution.
    ///
    /// # Errors
    ///
    /// Propagates audit-recording failures, which abort the turn.
    pub(super) async fn dispatch_ask_user_tool_call<W, R>(
        &mut self,
        scope: CoreAuditScope<'_>,
        call: &PendingToolCall,
        provider_type: &str,
        tool_kind: &str,
        reader: &mut tokio::io::Lines<R>,
        writer: &mut W,
    ) -> Result<Option<ToolResult>, String>
    where
        W: Write,
        R: AsyncBufReadExt + Unpin,
    {
        let report = ask_user_question::inspect_arguments(&call.arguments);
        let tool_data = AuditToolData {
            tool_kind: tool_kind.to_string(),
            input_shape: Some(
                report
                    .root
                    .as_ref()
                    .map(|root| json_shape(root))
                    .unwrap_or(report.json_parse_status)
                    .to_string(),
            ),
            input_hash: Some(match &report.root {
                Some(root) => hash_json(root),
                None => hash_bytes(call.arguments.as_bytes()),
            }),
            ..AuditToolData::default()
        };
        self.audit
            .record_tool_requested(scope, &call.name, &tool_data);

        let params = match report.outcome {
            Ok(params) => params,
            Err(error) => {
                // No control request is emitted: a question the user cannot
                // answer would block the run, and a generic placeholder would
                // hide the real cause.
                ask_user_question::log_rejection(&AskUserRejectionDiagnostics {
                    provider_type,
                    tool_call_id: &call.id,
                    tool_name: &call.name,
                    start_seen: call.start_seen,
                    delta_count: call.delta_count,
                    end_seen: call.end_seen,
                    argument_bytes: report.argument_bytes,
                    json_parse_status: report.json_parse_status,
                    validation_error_code: error.code(),
                    question_shape: report.question_shape,
                });
                self.reject_tool_arguments(
                    scope,
                    &call.name,
                    &call.id,
                    &tool_data,
                    error.tool_error_message(),
                );
                return Ok(None);
            }
        };

        self.audit
            .record_tool_execution_started(scope, &call.name, &tool_data)?;
        let tool_start = Instant::now();
        let result = self.handle_ask_user(&params, reader, writer).await;
        let duration_ms = tool_start.elapsed().as_millis() as u64;
        self.audit.record_tool_terminal(
            scope,
            &call.name,
            &tool_data,
            result.is_error,
            duration_ms,
            result.output.len() as u64,
        );
        // Counted like any other tool call so a single rejection cannot make the
        // question tool look like it always fails.
        self.note_tool_call_metrics(result.is_error, duration_ms);
        Ok(Some(result))
    }

    /// Record one completed tool call in the per-turn metrics.
    pub(super) fn note_tool_call_metrics(&mut self, is_error: bool, duration_ms: u64) {
        self.metrics.tool_calls_total += 1;
        self.metrics.tool_calls_duration_ms += duration_ms;
        if is_error {
            self.metrics.tool_calls_fail += 1;
        } else {
            self.metrics.tool_calls_success += 1;
        }
    }

    /// Fail a tool call whose arguments were rejected before execution.
    ///
    /// Audits the call as failed without a `tool.execution.started` record and
    /// appends an error tool result, so the model can re-issue a valid call
    /// instead of the run stalling on an unusable one.
    pub(super) fn reject_tool_arguments(
        &mut self,
        scope: CoreAuditScope<'_>,
        tool_name: &str,
        tool_call_id: &str,
        tool_data: &AuditToolData,
        message: String,
    ) {
        self.note_tool_call_metrics(true, 0);
        let result = ToolResult::error(message);
        self.audit.record_tool_terminal(
            scope,
            tool_name,
            tool_data,
            result.is_error,
            0,
            result.output.len() as u64,
        );
        self.messages.push(crate::provider::Message::tool_result(
            tool_call_id,
            &result.output,
            result.is_error,
        ));
    }

    /// Emit an interactive question and wait for the user's answer.
    ///
    /// Takes already-validated params: every fallback that could invent question
    /// text lives in `tool::ask_user_question`, so a malformed tool call can
    /// never reach the user as a generic prompt.
    pub(super) async fn handle_ask_user<W, R>(
        &self,
        params: &AskUserQuestionParams,
        reader: &mut tokio::io::Lines<R>,
        writer: &mut W,
    ) -> ToolResult
    where
        W: Write,
        R: AsyncBufReadExt + Unpin,
    {
        let options: Vec<crate::protocol::AskUserOption> = params
            .options
            .iter()
            .map(|option| crate::protocol::AskUserOption {
                label: option.label.clone(),
                description: option.description.clone(),
            })
            .collect();

        let request_id = self.next_request_id();
        self.emit(
            writer,
            &OutputMessage::ControlRequest {
                request_id: request_id.clone(),
                request: crate::protocol::CoreControlRequest::AskUser {
                    question: params.question.clone(),
                    options,
                    allow_free_text: params.allow_free_text,
                    multi_select: params.multi_select,
                },
            },
        );

        match self.wait_for_answer(&request_id, reader).await {
            Some(answer) => ToolResult::success(answer),
            None => ToolResult::error("User did not answer (interrupted or disconnected)"),
        }
    }
}

#[cfg(test)]
#[path = "tool_execution/tests.rs"]
mod tests;
