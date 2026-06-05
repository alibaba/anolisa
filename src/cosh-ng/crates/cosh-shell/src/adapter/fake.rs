use std::thread;
use std::time::Duration;

use crate::types::{AgentEvent, AgentRequest, QuestionSelectionMode};

use super::{first_token, AdapterError, AgentAdapter, AgentBackendCapabilities};

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
            if input.contains("context") {
                let context = request
                    .context_blocks
                    .iter()
                    .map(|block| {
                        format!(
                            "{} exit={} ref={} command={}",
                            block.id,
                            block.exit_code,
                            block
                                .output
                                .terminal_output_ref
                                .as_deref()
                                .unwrap_or("<missing>"),
                            block.command
                        )
                    })
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
                            "Recent context visible to Agent:\n{}\nHook hints visible to Agent:\n{}",
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
            if input.contains("markdown indented code") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "rendering".to_string(),
                        message: "returning indented markdown code guidance".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "Indented code check\n\n    cargo test --package cosh-shell\n    git status --short\n\nDone.".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "markdown indented code fake analysis completed".to_string(),
                    },
                ]);
            }
            if input.contains("markdown paragraph") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "rendering".to_string(),
                        message: "returning soft-wrapped markdown paragraph".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "Paragraph rendering\n\nThis Agent answer is split\nacross multiple source lines with 中文内容\nbut should read as one Markdown paragraph.".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "markdown paragraph fake analysis completed".to_string(),
                    },
                ]);
            }
            if input.contains("markdown pipe output") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "rendering".to_string(),
                        message: "returning markdown pipe output".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "Shell output:\n\n| 1 | Virtualization.VirtualMachine | ~1470 MB |\n| 2 | Node | ~572 MB |\n\nDone.".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "markdown pipe output fake analysis completed".to_string(),
                    },
                ]);
            }
            if input.contains("markdown table") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "rendering".to_string(),
                        message: "returning markdown table guidance".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "内存占用 Top 10 分析:\n\n| 排名 | 进程 | RSS (MB) | 说明 |\n| --- | --- | --- | --- |\n| 1 | Virtualization.VirtualMachine | ~1470 MB | 虚拟机进程，最大内存消耗者 |\n| 2 | ps aux \\| grep cosh | ~42 MB | escaped pipe 应保留在单元格中 |\n\n关键发现：Qoder 占用最多。".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "markdown table fake analysis completed".to_string(),
                    },
                ]);
            }
            if input.contains("markdown") {
                return Ok(vec![
                    AgentEvent::StatusChanged {
                        run_id: run_id.clone(),
                        phase: "rendering".to_string(),
                        message: "returning markdown guidance".to_string(),
                    },
                    AgentEvent::TextDelta {
                        run_id: run_id.clone(),
                        text: "# Project check\n\n- Run `git status`\n- Build workspace\n  - Use package scoped tests\n  1. Keep shell-first validation repeatable\n1. Review rendered transcript\n\n```bash\ncargo build --workspace\nif test -d crates; then\n  cargo test --package cosh-shell\nfi\n```\n\n> Commands are suggestions only.".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "markdown fake analysis completed".to_string(),
                    },
                ]);
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
                        name: "Read".to_string(),
                        input: r#"{"file_path":"Cargo.toml"}"#.to_string(),
                    },
                    AgentEvent::ToolCall {
                        run_id: run_id.clone(),
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
                        name: "Bash".to_string(),
                        input: "touch /tmp/cosh-shell-fake-action-should-not-run".to_string(),
                    },
                    AgentEvent::AgentCompleted {
                        run_id,
                        summary: "unsafe tool request requires approval".to_string(),
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
                    AgentEvent::SkillLoadStarted {
                        run_id: run_id.clone(),
                        skill: "git-project".to_string(),
                        reason: "project directory contains git metadata".to_string(),
                    },
                    AgentEvent::SkillLoadCompleted {
                        run_id: run_id.clone(),
                        skill: "git-project".to_string(),
                        summary: "loaded git troubleshooting guidance".to_string(),
                    },
                    AgentEvent::ToolCall {
                        run_id: run_id.clone(),
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
                message: "building failed-command context for fake adapter".to_string(),
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
        let Some(input) = &request.user_input else {
            for event in self.run(request)? {
                sink(event)?;
            }
            return Ok(());
        };

        if input.contains("stream markdown table") {
            let run_id = format!("fake-run-{}", request.command_block.id);
            sink(AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "streaming".to_string(),
                message: "streaming markdown table fake response".to_string(),
            })?;
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "# Streaming table\n\n".to_string(),
            })?;
            thread::sleep(Duration::from_millis(100));
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "| 排名 | 进程 | RSS |\n".to_string(),
            })?;
            thread::sleep(Duration::from_millis(100));
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "| --- | --- | --- |\n| 1 | ps aux \\| grep cosh | ~42 MB |\n\nDone."
                    .to_string(),
            })?;
            sink(AgentEvent::AgentCompleted {
                run_id,
                summary: "stream markdown table fake analysis completed".to_string(),
            })?;
            return Ok(());
        }

        if input.contains("stream markdown paragraph") {
            let run_id = format!("fake-run-{}", request.command_block.id);
            sink(AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "streaming".to_string(),
                message: "streaming markdown paragraph fake response".to_string(),
            })?;
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "# Streaming paragraph\n\nThis Agent answer starts\n".to_string(),
            })?;
            thread::sleep(Duration::from_millis(100));
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "and continues on another source line with 中文内容.\n\nDone.".to_string(),
            })?;
            sink(AgentEvent::AgentCompleted {
                run_id,
                summary: "stream markdown paragraph fake analysis completed".to_string(),
            })?;
            return Ok(());
        }

        if input.contains("stream markdown") {
            let run_id = format!("fake-run-{}", request.command_block.id);
            sink(AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "streaming".to_string(),
                message: "streaming markdown fake response".to_string(),
            })?;
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "# Streaming check\n\n".to_string(),
            })?;
            thread::sleep(Duration::from_millis(100));
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "- First item\n- Second item\n\n".to_string(),
            })?;
            thread::sleep(Duration::from_millis(100));
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "```bash\ncargo test --package cosh-shell\n```\n\nDone.".to_string(),
            })?;
            sink(AgentEvent::AgentCompleted {
                run_id,
                summary: "stream markdown fake analysis completed".to_string(),
            })?;
            return Ok(());
        }

        if input.contains("stream pwd tool approval") {
            let run_id = format!("fake-run-{}", request.command_block.id);
            sink(AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "streaming".to_string(),
                message: "streaming fake pwd approval request".to_string(),
            })?;
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "Preparing a streamed pwd request before finishing.".to_string(),
            })?;
            thread::sleep(Duration::from_millis(100));
            sink(AgentEvent::ToolCall {
                run_id: run_id.clone(),
                name: "Bash".to_string(),
                input: "pwd".to_string(),
            })?;
            thread::sleep(Duration::from_millis(800));
            sink(AgentEvent::AgentCompleted {
                run_id,
                summary: "stream pwd approval fake analysis completed".to_string(),
            })?;
            return Ok(());
        }

        if input.contains("stream stale tool approval") {
            let run_id = format!("fake-run-{}", request.command_block.id);
            sink(AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "streaming".to_string(),
                message: "streaming fake stale approval request".to_string(),
            })?;
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "Preparing a command before approval.".to_string(),
            })?;
            thread::sleep(Duration::from_millis(100));
            sink(AgentEvent::ToolCall {
                run_id: run_id.clone(),
                name: "Bash".to_string(),
                input: "pwd".to_string(),
            })?;
            thread::sleep(Duration::from_millis(800));
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "STALE APPROVAL TEXT SHOULD NOT RENDER".to_string(),
            })?;
            sink(AgentEvent::AgentCompleted {
                run_id,
                summary: "stream stale approval fake analysis completed".to_string(),
            })?;
            return Ok(());
        }

        if input.contains("stream tool approval") {
            let run_id = format!("fake-run-{}", request.command_block.id);
            sink(AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "streaming".to_string(),
                message: "streaming fake approval request".to_string(),
            })?;
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "Preparing a streamed tool request before finishing.".to_string(),
            })?;
            thread::sleep(Duration::from_millis(100));
            sink(AgentEvent::ToolCall {
                run_id: run_id.clone(),
                name: "Bash".to_string(),
                input: "git status --short".to_string(),
            })?;
            thread::sleep(Duration::from_millis(800));
            sink(AgentEvent::AgentCompleted {
                run_id,
                summary: "stream approval fake analysis completed".to_string(),
            })?;
            return Ok(());
        }

        if input.contains("stream blocked tool approval") {
            let run_id = format!("fake-run-{}", request.command_block.id);
            sink(AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "streaming".to_string(),
                message: "streaming fake blocked approval request".to_string(),
            })?;
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "Preparing a blocked streamed tool request before finishing.".to_string(),
            })?;
            thread::sleep(Duration::from_millis(100));
            sink(AgentEvent::ToolCall {
                run_id: run_id.clone(),
                name: "Bash".to_string(),
                input: "ps aux | head".to_string(),
            })?;
            thread::sleep(Duration::from_millis(800));
            sink(AgentEvent::AgentCompleted {
                run_id,
                summary: "blocked stream approval fake analysis completed".to_string(),
            })?;
            return Ok(());
        }

        if input.contains("stream stale question") {
            let run_id = format!("fake-run-{}", request.command_block.id);
            sink(AgentEvent::StatusChanged {
                run_id: run_id.clone(),
                phase: "question".to_string(),
                message: "streaming fake question request".to_string(),
            })?;
            sink(AgentEvent::UserQuestion {
                run_id: run_id.clone(),
                question: "Choose a color for the next step".to_string(),
                options: vec!["Green".to_string(), "Blue".to_string()],
                allow_free_text: true,
                selection_mode: QuestionSelectionMode::Single,
            })?;
            thread::sleep(Duration::from_millis(800));
            sink(AgentEvent::TextDelta {
                run_id: run_id.clone(),
                text: "STALE QUESTION TEXT SHOULD NOT RENDER".to_string(),
            })?;
            sink(AgentEvent::AgentCompleted {
                run_id,
                summary: "stale question fake analysis completed".to_string(),
            })?;
            return Ok(());
        }

        if !input.contains("slow") {
            for event in self.run(request)? {
                sink(event)?;
            }
            return Ok(());
        }

        let run_id = format!("fake-run-{}", request.command_block.id);
        sink(AgentEvent::StatusChanged {
            run_id: run_id.clone(),
            phase: "thinking".to_string(),
            message: "simulating a slow fake Agent run".to_string(),
        })?;
        let delay = if input.contains("hold test") {
            Duration::from_millis(1800)
        } else if input.contains("very slow") {
            Duration::from_millis(1500)
        } else {
            Duration::from_millis(500)
        };
        thread::sleep(delay);
        sink(AgentEvent::TextDelta {
            run_id: run_id.clone(),
            text: format!("Slow fake response for: {input}"),
        })?;
        sink(AgentEvent::AgentCompleted {
            run_id,
            summary: "slow fake analysis completed".to_string(),
        })?;
        Ok(())
    }
}

fn fake_long_tool_output() -> String {
    (1..=24)
        .map(|idx| format!("line {idx}: fake tool output for details view"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_fake_pending_answer(input: &str) -> Option<String> {
    input
        .lines()
        .find_map(|line| line.strip_prefix("User answer: "))
        .map(str::trim)
        .filter(|answer| !answer.is_empty())
        .map(ToString::to_string)
}

struct FakeToolResult {
    request: String,
    status: Option<String>,
}

fn extract_fake_tool_result(input: &str) -> Option<FakeToolResult> {
    if !input.starts_with("Tool result for approved request ") {
        return None;
    }
    let request = input
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("Tool result for approved request "))
        .map(str::trim)
        .filter(|request| !request.is_empty())
        .map(ToString::to_string)?;
    let status = input
        .lines()
        .find_map(|line| line.strip_prefix("Status: "))
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .map(ToString::to_string);
    Some(FakeToolResult { request, status })
}

fn extract_fake_approval_result(input: &str) -> Option<String> {
    if !input.starts_with("Approval result for request ") {
        return None;
    }
    input
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("Approval result for request "))
        .map(str::trim)
        .filter(|request| !request.is_empty())
        .map(ToString::to_string)
}
