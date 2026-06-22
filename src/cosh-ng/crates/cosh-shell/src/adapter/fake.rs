use crate::evidence::provider_safe_command_fact_line;
use crate::types::{AgentEvent, AgentRequest, QuestionSelectionMode};

use super::{first_token, AdapterError, AgentAdapter, AgentBackendCapabilities};
use control_protocol::emit_fake_control_protocol_stream;
use fixtures::{
    extract_fake_approval_result, extract_fake_pending_answer, extract_fake_tool_result,
    fake_long_tool_output,
};
use responses_markdown::fake_markdown_response;
use stream_markdown::emit_fake_markdown_stream;
use stream_state::{
    emit_fake_late_card_or_artifact_stream, emit_fake_slow_stream, emit_fake_stale_question_stream,
};
use stream_tool_approval::emit_fake_tool_approval_stream;

mod control_protocol;
mod fixtures;
mod responses_markdown;
mod stream_markdown;
mod stream_state;
mod stream_tool_approval;

#[derive(Debug, Default, Clone)]
pub struct FakeAgentAdapter;

impl AgentAdapter for FakeAgentAdapter {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn capabilities(&self) -> AgentBackendCapabilities {
        AgentBackendCapabilities {
            text_stream: true,
            thinking_stream: false,
            session_resume: false,
            tool_intent: true,
            user_question: true,
            cancellable: true,
            control_protocol: false,
        }
    }

    fn run(&self, request: &AgentRequest) -> Result<Vec<AgentEvent>, AdapterError> {
        if let Some(input) = &request.user_input {
            let run_id = format!("fake-run-{}", request.command_block.id);
            if let Some(answer) = extract_fake_pending_answer(input) {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "answer_received".to_string(),
                        message: "received answer for pending Agent question".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: format!("Got your answer: {answer}"),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "answer handled without executing commands".to_string(),
                    },
                ]);
            }
            if let Some(tool_result) = extract_fake_tool_result(input) {
                let text = match tool_result.status.as_deref() {
                    Some("executed") => format!(
                        "Command result analysis for {request}: the approved Bash command finished. Review the native output above before the next step.",
                        request = tool_result.request
                    ),
                    Some("blocked" | "timed_out" | "failed") => format!(
                        "Command result analysis for {request}: the approved Bash command did not produce a successful execution result. Use the broker message above and request a simpler single read-only command if more evidence is needed.",
                        request = tool_result.request
                    ),
                    _ => format!(
                        "Command result analysis for {request}: review the native tool result above before the next step.",
                        request = tool_result.request
                    ),
                };
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "tool_result_received".to_string(),
                        message: "received approved tool result for same session".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text,
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "tool result handled without requesting another tool".to_string(),
                    },
                ]);
            }
            if input.contains("ShellCommandCompleted evidence") {
                let approval = input
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("approval_id: "))
                    .unwrap_or("<unknown>");
                if input.contains("structured-before-recovery")
                    && !request
                        .context_hints
                        .iter()
                        .any(|hint| hint.contains("disable provider resume"))
                {
                    return Ok(vec![
                        AgentEvent::AgentFailed {
                            run_id,
                            error: "Agent timed out: No provider response within 20s".to_string(),
                        },
                    ]);
                }
                if input.contains("trigger resume timeout")
                    && !request
                        .context_hints
                        .iter()
                        .any(|hint| hint.contains("disable provider resume"))
                {
                    return Ok(vec![AgentEvent::AgentFailed {
                        run_id,
                        error: "Agent timed out: No provider response within 20s".to_string(),
                    }]);
                }
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "shell_evidence_received".to_string(),
                        message: "received foreground shell evidence".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: format!(
                            "Command result analysis for {approval}: foreground shell evidence received. No additional shell command is required."
                        ),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "shell evidence handled without requesting another tool"
                            .to_string(),
                    },
                ]);
            }
            if input.contains("ShellEvidenceExcerpt") {
                if input.contains("history_index:") {
                    let text = if input.contains("token=<redacted>")
                        && !input.contains("command: echo token=super-secret")
                    {
                        "Redacted history index received by fake adapter."
                    } else {
                        "Evidence history index received by fake adapter."
                    };
                    return Ok(vec![
                        AgentEvent::StatusChanged {
                            run_id: run_id.clone(),
                            phase: "evidence_history_received".to_string(),
                            message: "received shell history index".to_string(),
                        },
                        AgentEvent::TextDelta {
                            run_id: run_id.clone(),
                            text: text.to_string(),
                        },
                        AgentEvent::AgentCompleted {
                            run_id,
                            summary: "evidence history handled".to_string(),
                        },
                    ]);
                }
                if input.contains("bounded_output_excerpt:") {
                    let excerpt = input
                        .split_once("bounded_output_excerpt:\n")
                        .map(|(_, excerpt)| excerpt.trim())
                        .unwrap_or("<missing excerpt>");
                    return Ok(vec![
                        AgentEvent::StatusChanged {
                            run_id: run_id.clone(),
                            phase: "evidence_excerpt_received".to_string(),
                            message: "received bounded shell evidence excerpt".to_string(),
                        },
                        AgentEvent::TextDelta {
                            run_id: run_id.clone(),
                            text: format!("Evidence excerpt received by fake adapter: {excerpt}"),
                        },
                        AgentEvent::AgentCompleted {
                            run_id,
                            summary: "evidence excerpt handled".to_string(),
                        },
                    ]);
                }
            }
            if let Some(approval_result) = extract_fake_approval_result(input) {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "approval_result_received".to_string(),
                        message: "received approval denial for same session".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: format!(
                            "Command was not executed for {approval_result}. No shell output exists for that request."
                        ),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "approval result handled without executing commands".to_string(),
                    },
                ]);
            }
            if input.contains("adapter crash") {
                return Err(AdapterError {
                    message: "fake adapter crashed".to_string(),
                });
            }
            if input.contains("backend unavailable") || input.contains("adapter failure") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "failed".to_string(),
                        message: "simulating fake backend failure".to_string(),
                    },
                    AgentEvent::AgentFailed {
                        run_id,
                        error: "fake backend unavailable".to_string(),
                    },
                ]);
            }
            if input.contains("context") {
                let context = request
                    .context_blocks
                    .iter()
                    .map(provider_safe_command_fact_line)
                    .collect::<Vec<_>>()
                    .join("\n");
                let hook_hints = request.context_hints.join("\n");
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "context".to_string(),
                        message: "returning recent shell context".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: format!(
                            "Recent context visible to Agent:\n{}\nHook routing hints visible to Agent:\n{}",
                            if context.is_empty() {
                                "<none>".to_string()
                            } else {
                                context
                            },
                            if hook_hints.is_empty() {
                                "<none>".to_string()
                            } else {
                                hook_hints
                            }
                        ),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "recent context fake analysis completed".to_string(),
                    },
                ]);
            }
            if let Some(events) = fake_markdown_response(input, &run_id) {
                return Ok(events);
            }
            if input.contains("ask") && input.contains("question") {
                let (question, options, selection_mode) = if input.contains("multi") {
                    (
                        "Choose checks to run".to_string(),
                        vec![
                            "Lint".to_string(),
                            "Unit tests".to_string(),
                            "Raw shell smoke".to_string(),
                        ],
                        QuestionSelectionMode::Multiple,
                    )
                } else if input.contains("free") {
                    (
                        "Tell me the branch name to inspect".to_string(),
                        Vec::new(),
                        QuestionSelectionMode::Single,
                    )
                } else {
                    (
                        "Choose a color for the next step".to_string(),
                        vec!["Green".to_string(), "Blue".to_string(), "Gray".to_string()],
                        QuestionSelectionMode::Single,
                    )
                };
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "question".to_string(),
                        message: "asking user to choose an option".to_string(),
                    },
                    AgentEvent::UserQuestion {
                        run_id: run_id.clone(),
                        provider_request_id: None,
                        question,
                        options,
                        allow_free_text: true,
                        selection_mode,
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "question displayed without executing commands".to_string(),
                    },
                ]);
            }
            if input.contains("readonly builtin tool") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "routing".to_string(),
                        message: "matching read-only builtin fake tool workflow".to_string(),
                    },
                    AgentEvent::ToolCall {
                        run_id: run_id.clone(),
                        tool_id: None,
                        name: "Read".to_string(),
                        input: r#"{"file_path":"Cargo.toml"}"#.to_string(),
                    },
                    AgentEvent::ToolCall {
                        run_id: run_id.clone(),
                        tool_id: None,
                        name: "Grep".to_string(),
                        input: r#"{"pattern":"cosh","path":"crates/cosh-shell"}"#.to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "read-only builtin tool request completed".to_string(),
                    },
                ]);
            }
            if input.contains("unsafe tool") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "routing".to_string(),
                        message: "matching shell-first request to unsafe fake tool workflow"
                            .to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: format!("Received shell prompt request: {input}"),
                    },
                    AgentEvent::ToolCall {
                        run_id: run_id.clone(),
                        tool_id: None,
                        name: "Bash".to_string(),
                        input: "touch /tmp/cosh-shell-fake-action-should-not-run".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "unsafe tool request requires approval".to_string(),
                    },
                ]);
            }
            if input.contains("long tool") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "routing".to_string(),
                        message: "matching long-running fake tool workflow".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: format!("Received shell prompt request: {input}"),
                    },
                    AgentEvent::ToolCall {
                        run_id: run_id.clone(),
                        tool_id: None,
                        name: "Bash".to_string(),
                        input: "sleep 4; printf done".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "long-running tool request requires approval".to_string(),
                    },
                ]);
            }
            if input.contains("agent memory hook fallback") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "routing".to_string(),
                        message: "matching local agent fallback memory hook workflow".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: format!("Received shell prompt request: {input}"),
                    },
                    AgentEvent::ToolCall {
                        run_id: run_id.clone(),
                        tool_id: None,
                        name: "Bash".to_string(),
                        input: "free -m; touch /tmp/cosh-shell-fake-memory-hook-marker".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "local agent fallback requires approval".to_string(),
                    },
                ]);
            }
            if input.contains("provider native tool") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "routing".to_string(),
                        message: "matching provider-native fake tool workflow".to_string(),
                    },
                    AgentEvent::ToolPermissionRequest {
                        run_id: run_id.clone(),
                        request_id: "ctrl-1".to_string(),
                        tool_name: "run_shell_command".to_string(),
                        tool_input: serde_json::json!({ "command": "git status" }),
                        tool_use_id: "toolu-1".to_string(),
                        hook_requires_approval: false,
                    },
                    AgentEvent::ToolOutputDelta {
                        run_id: run_id.clone(),
                        tool_id: "toolu-1".to_string(),
                        stream: "stdout".to_string(),
                        text: "On branch main\nnothing to commit\n".to_string(),
                    },
                    AgentEvent::ToolCompleted {
                        run_id: run_id.clone(),
                        tool_id: "toolu-1".to_string(),
                        status: "completed".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "Provider-native tool result rendered.".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "provider-native fake tool completed".to_string(),
                    },
                ]);
            }
            if input.contains("provider interactive failure") {
                return Ok(vec![
                    AgentEvent::ToolCall {
                        run_id: run_id.clone(),
                        tool_id: Some("toolu-tty".to_string()),
                        name: "run_shell_command".to_string(),
                        input: r#"{"command":"git status"}"#.to_string(),
                    },
                    AgentEvent::ToolOutputDelta {
                        run_id: run_id.clone(),
                        tool_id: "toolu-tty".to_string(),
                        stream: "stderr".to_string(),
                        text: "sudo: a terminal is required\n".to_string(),
                    },
                    AgentEvent::ToolCompleted {
                        run_id: run_id.clone(),
                        tool_id: "toolu-tty".to_string(),
                        status: "error".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "provider interactive failure rendered".to_string(),
                    },
                ]);
            }
            if input.contains("tool output finalization") {
                return Ok(vec![
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "Before tool **markdown**.\n\n".to_string(),
                    },
                    AgentEvent::ToolOutputDelta {
                        run_id: run_id.clone(),
                        tool_id: "toolu-md".to_string(),
                        stream: "stdout".to_string(),
                        text: "clean\n".to_string(),
                    },
                    AgentEvent::ToolCompleted {
                        run_id: run_id.clone(),
                        tool_id: "toolu-md".to_string(),
                        status: "success".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "\nAfter tool **markdown**.".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "markdown around provider tool output completed".to_string(),
                    },
                ]);
            }
            if input.contains("request shell history evidence") {
                return Ok(vec![
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "I need shell history.\n```cosh-request\nhistory\n```\n".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "requested shell history evidence".to_string(),
                    },
                ]);
            }
            if input.contains("request captured output evidence") {
                return Ok(vec![
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "I need captured output.\n```cosh-request\noutput terminal-output://raw-session/cmd-1 tail\nlines 2\n```\n"
                            .to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "requested captured output evidence".to_string(),
                    },
                ]);
            }
            if input.contains("tool") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "routing".to_string(),
                        message: "matching shell-first request to fake tool workflow".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: format!("Received shell prompt request: {input}"),
                    },
                    AgentEvent::ToolCall {
                        run_id: run_id.clone(),
                        tool_id: None,
                        name: "shell".to_string(),
                        input: "git status".to_string(),
                    },
                    AgentEvent::ToolOutputDelta {
                        run_id: run_id.clone(),
                        tool_id: "tool-1".to_string(),
                        stream: "stdout".to_string(),
                        text: fake_long_tool_output(),
                    },
                    AgentEvent::ToolCompleted {
                        run_id: run_id.clone(),
                        tool_id: "tool-1".to_string(),
                        status: "completed".to_string(),
                    },
                    AgentEvent::Action {
                        run_id: run_id.clone(),
                        command: "touch /tmp/cosh-shell-fake-action-should-not-run".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "analysis completed without executing commands".to_string(),
                    },
                ]);
            }
            return Ok(vec![
                AgentEvent::StatusChanged {
                    run_id: run_id.clone(),
                    phase: "routing".to_string(),
                    message: "matching shell-first request to fake guidance workflow".to_string(),
                },
                AgentEvent::TextDelta {
                    run_id: run_id.clone(),
                    text: format!("Received shell prompt request: {input}"),
                },
                AgentEvent::Recommendation {
                    run_id: run_id.clone(),
                    summary: "Ask the configured Agent backend for recommend-only shell guidance."
                        .to_string(),
                    commands: Vec::new(),
                    auto_execute: false,
                },
                AgentEvent::AgentCompleted {
                    run_id,
                    summary: "analysis completed without executing commands".to_string(),
                },
            ]);
        }

        let run_id = format!("fake-run-{}", request.command_block.id);
        Ok(vec![
            AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "analyzing".to_string(),
                message: "building failed command context for fake adapter".to_string(),
            },
            AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: format!(
                    "The command `{}` failed with exit code {}.",
                    request.command_block.command, request.command_block.exit_code
                ),
            },
            AgentEvent::Recommendation {
                run_id: run_id.clone(),
                summary: "Inspect the command, working directory, and captured terminal output."
                    .to_string(),
                commands: vec![
                    "pwd".to_string(),
                    "echo $PATH".to_string(),
                    format!("{} --help", first_token(&request.command_block.command)),
                ],
                auto_execute: false,
            },
            AgentEvent::AgentCompleted {
                run_id,
                summary: "analysis completed without executing commands".to_string(),
            },
        ])
    }

    fn run_stream(
        &self,
        request: &AgentRequest,
        sink: &mut dyn FnMut(AgentEvent) -> Result<(), AdapterError>,
    ) -> Result<(), AdapterError> {
        let input = request
            .user_input
            .as_deref()
            .unwrap_or(request.command_block.command.as_str());

        if input.contains("ShellCommandCompleted evidence") {
            for event in self.run(request)? {
                sink(event)?;
            }
            return Ok(());
        }

        if emit_fake_markdown_stream(input, request, sink)? {
            return Ok(());
        }

        if emit_fake_tool_approval_stream(input, request, sink)? {
            return Ok(());
        }

        if emit_fake_stale_question_stream(input, request, sink)? {
            return Ok(());
        }

        if emit_fake_late_card_or_artifact_stream(input, request, sink)? {
            return Ok(());
        }

        if emit_fake_control_protocol_stream(input, request, sink)? {
            return Ok(());
        }

        if !input.contains("slow") {
            for event in self.run(request)? {
                sink(event)?;
            }
            return Ok(());
        }

        emit_fake_slow_stream(input, request, sink)
    }
}
