use std::collections::{HashSet, VecDeque};

use crate::agent::run::ActiveAgentRun;
use crate::evidence::model::{EvidenceExcerptRequest, OutputExcerptDirection};
use crate::evidence::output_policy::{
    bounded_output_excerpt_for_id, output_excerpt_status_for_block, parse_terminal_output_id,
    terminal_output_id,
};
use crate::evidence::request::{CoshRequest, ParsedCoshRequest};
use crate::evidence::stream::{CoshRequestAuditOutcome, CoshRequestAuditRecord};
use crate::runtime::prelude::*;
use cosh_shell::context_window::redact_provider_command_text;
use cosh_shell::raw_input::RawInputCapture;

const DEFAULT_OUTPUT_LINES: usize = 120;
const MAX_OUTPUT_LINES: usize = 300;
const MAX_OUTPUT_BYTES: usize = 12 * 1024;
const HISTORY_LIMIT: usize = 20;

#[derive(Debug, Default)]
pub(crate) struct EvidenceRequestState {
    pending: VecDeque<RuntimeEvidenceRequest>,
    rendered: HashSet<String>,
    handled_actions: HashSet<String>,
    audit_records: Vec<RuntimeCoshRequestAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeEvidenceRequest {
    id: String,
    kind: RuntimeEvidenceRequestKind,
    ignored_multiple_request_blocks: bool,
    audit_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeCoshRequestAudit {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) outcome: CoshRequestAuditOutcome,
    pub(crate) reason: &'static str,
    pub(crate) raw_block: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeEvidenceRequestKind {
    History,
    Output(EvidenceExcerptRequest),
}

pub(crate) struct RecordedEvidenceRequests {
    pub(crate) auto_requests: Vec<AgentRequest>,
    pub(crate) card_ids: Vec<String>,
    pub(crate) notices: Vec<String>,
}

pub(crate) fn record_cosh_requests_from_active_run(
    state: &mut InlineState,
    active_run: &mut ActiveAgentRun,
) -> RecordedEvidenceRequests {
    let mut recorded = RecordedEvidenceRequests {
        auto_requests: Vec::new(),
        card_ids: Vec::new(),
        notices: Vec::new(),
    };
    let parsed_requests = active_run
        .pending_cosh_requests
        .drain(..)
        .collect::<Vec<_>>();
    let first_parsed_audit_id = record_cosh_request_audits(
        state,
        &active_run.request.id,
        &mut active_run.pending_cosh_request_audits,
    );
    let mut parsed_requests = parsed_requests.into_iter();
    let Some(mut parsed) = parsed_requests.next() else {
        return recorded;
    };
    if parsed_requests.next().is_some() {
        parsed.ignored_multiple_request_blocks = true;
    }
    let id = format!("evidence-{}", state.evidence_requests.pending.len() + 1);
    let request = runtime_request_from_parsed(id.clone(), parsed);
    let request = RuntimeEvidenceRequest {
        audit_id: first_parsed_audit_id,
        ..request
    };
    if active_run_has_unclosed_provider_tool_turn(active_run) {
        recorded.notices.push(
            "deferred evidence request because the provider tool turn is still open".to_string(),
        );
        return recorded;
    }
    if matches!(&request.kind, RuntimeEvidenceRequestKind::History)
        && !history_request_needs_confirmation(state)
    {
        match agent_request_from_history_request(&state.session_blocks, id_sequence(&id)) {
            Ok(agent_request) => recorded.auto_requests.push(agent_request),
            Err(message) => recorded.notices.push(message),
        }
    } else {
        state.evidence_requests.pending.push_back(request);
        recorded.card_ids.push(id);
    }
    recorded
}

pub(crate) fn cosh_request_audit_by_id<'a>(
    state: &'a InlineState,
    id: &str,
) -> Option<&'a RuntimeCoshRequestAudit> {
    state
        .evidence_requests
        .audit_records
        .iter()
        .find(|record| record.id == id)
}

pub(crate) fn pending_evidence_capture(state: &InlineState) -> Option<RawInputCapture> {
    state
        .evidence_requests
        .pending
        .front()
        .map(|request| RawInputCapture::Evidence {
            id: request.id.clone(),
        })
}

pub(crate) fn clear_pending_evidence_requests(state: &mut InlineState) {
    state.evidence_requests.pending.clear();
    state.evidence_requests.rendered.clear();
}

pub(crate) fn render_pending_evidence_requests<W: Write>(
    state: &mut InlineState,
    ids: &[String],
    output: &mut W,
) -> std::io::Result<()> {
    for id in ids {
        if !state.evidence_requests.rendered.insert(id.clone()) {
            continue;
        }
        let Some(request) = state
            .evidence_requests
            .pending
            .iter()
            .find(|request| request.id == *id)
        else {
            continue;
        };
        render_evidence_request_card(state.language, request, output)?;
    }
    Ok(())
}

pub(crate) fn render_evidence_request_actions<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        let Some(action) = evidence_action_from_event(event) else {
            continue;
        };
        let key = stable_event_key("evidence-request", event_index, event);
        if !state.evidence_requests.handled_actions.insert(key) {
            continue;
        }
        let Some(request) = take_pending_request(state, &action.id) else {
            continue;
        };
        state.evidence_requests.rendered.remove(&request.id);
        match action.kind {
            EvidenceActionKind::Send => {
                match agent_request_from_evidence_request(blocks, &request, event_index) {
                    Ok(agent_request) => {
                        start_agent_run(&agent_request, adapter, state, output, Some(event_index))?;
                    }
                    Err(message) => {
                        render_evidence_notice(
                            state.language,
                            evidence_notice_title(state.language),
                            &message,
                            output,
                        )?;
                    }
                }
            }
            EvidenceActionKind::Ignore => {
                state.agent_run.needs_prompt_after_run = false;
                state.trigger_pty_prompt = false;
                render_evidence_notice(
                    state.language,
                    evidence_notice_title(state.language),
                    evidence_ignored_body(state.language),
                    output,
                )?;
            }
            EvidenceActionKind::Cancel => {
                state.agent_run.needs_prompt_after_run = false;
                state.trigger_pty_prompt = false;
            }
        }
        output.flush()?;
    }
    Ok(())
}

fn runtime_request_from_parsed(id: String, parsed: ParsedCoshRequest) -> RuntimeEvidenceRequest {
    let kind = match parsed.request {
        CoshRequest::History => RuntimeEvidenceRequestKind::History,
        CoshRequest::Output(request) => RuntimeEvidenceRequestKind::Output(request),
    };
    RuntimeEvidenceRequest {
        id,
        kind,
        ignored_multiple_request_blocks: parsed.ignored_multiple_request_blocks,
        audit_id: None,
    }
}

fn history_request_needs_confirmation(state: &InlineState) -> bool {
    if state.approval_mode == CoshApprovalMode::Recommend {
        return true;
    }
    state
        .session_blocks
        .iter()
        .rev()
        .take(HISTORY_LIMIT)
        .any(|block| {
            let redacted = redact_provider_command_text(&block.command);
            redacted != block.command
        })
}

fn active_run_has_unclosed_provider_tool_turn(active_run: &ActiveAgentRun) -> bool {
    let mut open = HashSet::new();
    let mut unknown_open_tool_call = false;
    for event in &active_run.governed_events {
        match &event.event {
            AgentEvent::ToolCall { tool_id, .. } => {
                if let Some(tool_id) = tool_id {
                    open.insert(tool_id.clone());
                } else {
                    unknown_open_tool_call = true;
                }
            }
            AgentEvent::ToolPermissionRequest { tool_use_id, .. } => {
                open.insert(tool_use_id.clone());
            }
            AgentEvent::ToolCompleted { tool_id, .. } => {
                open.remove(tool_id);
            }
            _ => {}
        }
    }
    unknown_open_tool_call || !open.is_empty()
}

fn record_cosh_request_audits(
    state: &mut InlineState,
    run_id: &str,
    audit_records: &mut Vec<CoshRequestAuditRecord>,
) -> Option<String> {
    let mut first_parsed = None;
    for audit in audit_records.drain(..) {
        let id = format!(
            "cosh-request-{}",
            state.evidence_requests.audit_records.len() + 1
        );
        if audit.outcome == CoshRequestAuditOutcome::Parsed && first_parsed.is_none() {
            first_parsed = Some(id.clone());
        }
        state
            .evidence_requests
            .audit_records
            .push(RuntimeCoshRequestAudit {
                id,
                run_id: run_id.to_string(),
                outcome: audit.outcome,
                reason: audit.reason,
                raw_block: audit.raw_block,
            });
    }
    first_parsed
}

fn id_sequence(id: &str) -> usize {
    id.strip_prefix("evidence-")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

fn render_evidence_request_card<W: Write>(
    language: cosh_shell::Language,
    request: &RuntimeEvidenceRequest,
    output: &mut W,
) -> std::io::Result<()> {
    let body = match &request.kind {
        RuntimeEvidenceRequestKind::History => vec![
            evidence_request_history_body(language).to_string(),
            evidence_request_actions_body(language).to_string(),
        ],
        RuntimeEvidenceRequestKind::Output(request) => vec![
            format!(
                "{} {} {}",
                evidence_request_output_body(language),
                request.output_id,
                direction_label(request.direction)
            ),
            format!(
                "{} {}",
                evidence_request_lines_body(language),
                request.lines.unwrap_or(DEFAULT_OUTPUT_LINES)
            ),
            evidence_request_actions_body(language).to_string(),
        ],
    };
    let footer = request
        .audit_id
        .as_ref()
        .map(|id| format!("Details: {id}"))
        .or_else(|| {
            request
                .ignored_multiple_request_blocks
                .then(|| evidence_multiple_footer(language).to_string())
        });
    RatatuiInlineRenderer::for_terminal()
        .with_language(language)
        .write_notice_panel(
            output,
            NoticePanelModel {
                title: evidence_request_title(language),
                body,
                footer: footer.as_deref(),
            },
        )
}

fn agent_request_from_evidence_request(
    blocks: &[CommandBlock],
    request: &RuntimeEvidenceRequest,
    sequence: usize,
) -> Result<AgentRequest, String> {
    match &request.kind {
        RuntimeEvidenceRequestKind::History => agent_request_from_history_request(blocks, sequence),
        RuntimeEvidenceRequestKind::Output(output) => {
            agent_request_from_output_request(blocks, output, sequence)
        }
    }
}

fn agent_request_from_history_request(
    blocks: &[CommandBlock],
    sequence: usize,
) -> Result<AgentRequest, String> {
    let Some(anchor) = blocks.last() else {
        return Err("no shell history is available".to_string());
    };
    let history = blocks
        .iter()
        .rev()
        .take(HISTORY_LIMIT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|block| {
            let status = match block.status {
                CommandStatus::Completed => "completed",
                CommandStatus::Failed => "failed",
            };
            format!(
                "- command_id: {id}; output_id: {output_id}; status: {status}; exit_code: {exit_code}; command: {command}",
                id = block.id,
                output_id = terminal_output_id(&block.session_id, &block.id),
                exit_code = block.exit_code,
                command = redact_provider_command_text(&block.command)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(AgentRequest {
        id: format!("evidence-history-{sequence}"),
        session_id: anchor.session_id.clone(),
        command_block: anchor.clone(),
        context_blocks: Vec::new(),
        context_hints: Vec::new(),
        user_input: Some(format!(
            "ShellEvidenceExcerpt\nhistory_limit: {HISTORY_LIMIT}\nhistory_index:\n{history}"
        )),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    })
}

fn agent_request_from_output_request(
    blocks: &[CommandBlock],
    request: &EvidenceExcerptRequest,
    sequence: usize,
) -> Result<AgentRequest, String> {
    let parsed = parse_terminal_output_id(&request.output_id)
        .ok_or_else(|| format!("invalid output id: {}", request.output_id))?;
    let block = blocks
        .iter()
        .find(|block| block.session_id == parsed.shell_session_id && block.id == parsed.command_id)
        .ok_or_else(|| {
            format!(
                "output id is not part of this shell session: {}",
                request.output_id
            )
        })?;
    if block.output.terminal_output_ref.is_none() {
        return Err(format!("no captured output for {}", request.output_id));
    }
    let lines = request
        .lines
        .unwrap_or(DEFAULT_OUTPUT_LINES)
        .min(MAX_OUTPUT_LINES);
    let excerpt = bounded_output_excerpt_for_id(
        blocks,
        &request.output_id,
        request.direction,
        lines,
        MAX_OUTPUT_BYTES,
    );
    let Some(text) = excerpt.text.as_deref() else {
        return Err(format!(
            "captured output is unavailable: {}",
            request.output_id
        ));
    };
    let status = match block.status {
        CommandStatus::Completed => "completed",
        CommandStatus::Failed => "failed",
    };
    let output_excerpt_status = output_excerpt_status_for_block(block);
    let user_input = format!(
        "ShellEvidenceExcerpt\n\
         output_id: {output_id}\n\
         command_id: {command_id}\n\
         command: {command}\n\
         cwd: {cwd}\n\
         end_cwd: {end_cwd}\n\
         status: {status}\n\
         exit_code: {exit_code}\n\
         duration_ms: {duration_ms}\n\
         output_bytes: {output_bytes}\n\
         output_excerpt_status: {output_excerpt_status}\n\
         direction: {direction}\n\
         lines_requested: {lines}\n\
         excerpt_status: {excerpt_status}\n\
         redaction_status: {redaction_status}\n\
         bounded_output_excerpt:\n{text}",
        output_id = request.output_id,
        command_id = block.id,
        command = redact_provider_command_text(&block.command),
        cwd = block.cwd,
        end_cwd = block.end_cwd,
        exit_code = block.exit_code,
        duration_ms = block.duration_ms,
        output_bytes = block.output.terminal_output_bytes,
        output_excerpt_status = output_excerpt_status,
        direction = direction_label(request.direction),
        excerpt_status = excerpt.status,
        redaction_status = excerpt.redaction_status,
    );
    Ok(AgentRequest {
        id: format!("evidence-output-{sequence}"),
        session_id: block.session_id.clone(),
        command_block: block.clone(),
        context_blocks: Vec::new(),
        context_hints: Vec::new(),
        user_input: Some(user_input),
        findings: Vec::new(),
        mode: AgentMode::RecommendOnly,
        user_confirmed: true,
        hook_finding: None,
        recommended_skill: None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EvidenceAction {
    id: String,
    kind: EvidenceActionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvidenceActionKind {
    Send,
    Ignore,
    Cancel,
}

fn evidence_action_from_event(event: &ShellEvent) -> Option<EvidenceAction> {
    if event.component.as_deref() != Some("card") {
        return None;
    }
    let id = event.input.clone()?;
    let kind = match event.message.as_deref()? {
        "evidence_send" => EvidenceActionKind::Send,
        "evidence_ignore" => EvidenceActionKind::Ignore,
        "evidence_cancel" => EvidenceActionKind::Cancel,
        _ => return None,
    };
    Some(EvidenceAction { id, kind })
}

fn take_pending_request(state: &mut InlineState, id: &str) -> Option<RuntimeEvidenceRequest> {
    let index = state
        .evidence_requests
        .pending
        .iter()
        .position(|request| request.id == id)?;
    state.evidence_requests.pending.remove(index)
}

fn render_evidence_notice<W: Write>(
    language: cosh_shell::Language,
    title: &str,
    body: &str,
    output: &mut W,
) -> std::io::Result<()> {
    RatatuiInlineRenderer::for_terminal()
        .with_language(language)
        .write_notice_panel(
            output,
            NoticePanelModel {
                title,
                body: vec![body.to_string()],
                footer: None,
            },
        )
}

fn direction_label(direction: OutputExcerptDirection) -> &'static str {
    match direction {
        OutputExcerptDirection::Head => "head",
        OutputExcerptDirection::Tail => "tail",
    }
}

fn evidence_request_title(language: cosh_shell::Language) -> &'static str {
    match language {
        cosh_shell::Language::ZhCn => "Agent 请求更多证据",
        cosh_shell::Language::EnUs => "Agent Requested Evidence",
    }
}

fn evidence_request_history_body(language: cosh_shell::Language) -> &'static str {
    match language {
        cosh_shell::Language::ZhCn => "Agent 想查看最近的 shell 命令索引。",
        cosh_shell::Language::EnUs => "Agent wants to inspect the recent shell command index.",
    }
}

fn evidence_request_output_body(language: cosh_shell::Language) -> &'static str {
    match language {
        cosh_shell::Language::ZhCn => "Agent 想查看捕获输出:",
        cosh_shell::Language::EnUs => "Agent wants to inspect captured output:",
    }
}

fn evidence_request_lines_body(language: cosh_shell::Language) -> &'static str {
    match language {
        cosh_shell::Language::ZhCn => "最大行数:",
        cosh_shell::Language::EnUs => "Max lines:",
    }
}

fn evidence_request_actions_body(language: cosh_shell::Language) -> &'static str {
    match language {
        cosh_shell::Language::ZhCn => "Enter 发送片段 · i 忽略 · Esc/Ctrl+C 取消",
        cosh_shell::Language::EnUs => "Enter sends excerpt · i ignores · Esc/Ctrl+C cancels",
    }
}

fn evidence_multiple_footer(language: cosh_shell::Language) -> &'static str {
    match language {
        cosh_shell::Language::ZhCn => "同一回复中的其它请求已忽略。",
        cosh_shell::Language::EnUs => "Other requests in the same response were ignored.",
    }
}

fn evidence_notice_title(language: cosh_shell::Language) -> &'static str {
    match language {
        cosh_shell::Language::ZhCn => "证据请求",
        cosh_shell::Language::EnUs => "Evidence Request",
    }
}

fn evidence_ignored_body(language: cosh_shell::Language) -> &'static str {
    match language {
        cosh_shell::Language::ZhCn => "已忽略这次证据请求。",
        cosh_shell::Language::EnUs => "Ignored this evidence request.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosh_shell::types::{CommandStatus, OutputRefs};

    #[test]
    fn output_request_injects_bounded_excerpt_without_path() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-request-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output_ref = dir.join("cmd-1.txt");
        std::fs::write(&output_ref, "one\ntwo\nthree\n").expect("write output");
        let mut block = command_block(output_ref.to_str().expect("utf8 output path"));
        block.command =
            "curl --token cli-secret https://example.test/?secret=query-secret".to_string();
        let request = RuntimeEvidenceRequest {
            id: "evidence-1".to_string(),
            kind: RuntimeEvidenceRequestKind::Output(EvidenceExcerptRequest {
                output_id: "terminal-output://session-1/cmd-1".to_string(),
                direction: OutputExcerptDirection::Tail,
                lines: Some(2),
            }),
            ignored_multiple_request_blocks: false,
            audit_id: None,
        };

        let agent_request =
            agent_request_from_evidence_request(&[block], &request, 9).expect("agent request");

        assert_eq!(agent_request.id, "evidence-output-9");
        let input = agent_request.user_input.expect("user input");
        assert!(input.starts_with("ShellEvidenceExcerpt\n"), "{input}");
        assert!(
            input.contains("output_id: terminal-output://session-1/cmd-1"),
            "{input}"
        );
        assert!(
            input.contains("bounded_output_excerpt:\ntwo\nthree"),
            "{input}"
        );
        assert!(
            input.contains("output_excerpt_status: available"),
            "{input}"
        );
        assert!(!input.contains(output_ref.to_str().unwrap()), "{input}");
    }

    #[test]
    fn output_request_marks_capture_truncated_status() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-request-truncated-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output_ref = dir.join("cmd-1.txt");
        std::fs::write(&output_ref, "one\ntwo\nthree\n").expect("write output");
        let mut block = command_block(output_ref.to_str().expect("utf8 output path"));
        block.output.terminal_output_bytes =
            cosh_shell::types::COMMAND_OUTPUT_REF_MAX_BYTES as u64 + 1;
        let request = RuntimeEvidenceRequest {
            id: "evidence-1".to_string(),
            kind: RuntimeEvidenceRequestKind::Output(EvidenceExcerptRequest {
                output_id: "terminal-output://session-1/cmd-1".to_string(),
                direction: OutputExcerptDirection::Tail,
                lines: Some(2),
            }),
            ignored_multiple_request_blocks: false,
            audit_id: None,
        };

        let agent_request =
            agent_request_from_evidence_request(&[block], &request, 9).expect("agent request");
        let input = agent_request.user_input.expect("user input");

        assert!(
            input.contains("output_excerpt_status: truncated_at_capture"),
            "{input}"
        );
        assert!(
            input.contains("bounded_output_excerpt:\ntwo\nthree"),
            "{input}"
        );
    }

    #[test]
    fn history_request_injects_index_without_output_contents() {
        let block = command_block("/tmp/missing-output");
        let request = RuntimeEvidenceRequest {
            id: "evidence-1".to_string(),
            kind: RuntimeEvidenceRequestKind::History,
            ignored_multiple_request_blocks: false,
            audit_id: None,
        };

        let agent_request =
            agent_request_from_evidence_request(&[block], &request, 3).expect("agent request");
        let input = agent_request.user_input.expect("user input");

        assert!(input.contains("history_index:"), "{input}");
        assert!(
            input.contains("terminal-output://session-1/cmd-1"),
            "{input}"
        );
        assert!(!input.contains("bounded_output_excerpt:"), "{input}");
    }

    #[test]
    fn records_history_request_as_auto_follow_up() {
        let mut state = InlineState::default();
        state.session_blocks = vec![command_block("/tmp/missing-output")];
        let mut active_run = test_active_run();
        active_run.pending_cosh_requests = vec![ParsedCoshRequest {
            request: CoshRequest::History,
            ignored_multiple_request_blocks: false,
        }];
        active_run.pending_cosh_request_audits =
            vec![crate::evidence::stream::CoshRequestAuditRecord {
                raw_block: "```cosh-request\nhistory\n```".to_string(),
                outcome: crate::evidence::stream::CoshRequestAuditOutcome::Parsed,
                reason: "parsed",
            }];

        let recorded = record_cosh_requests_from_active_run(&mut state, &mut active_run);

        assert_eq!(recorded.auto_requests.len(), 1);
        assert_eq!(recorded.card_ids.len(), 0);
        assert_eq!(state.evidence_requests.pending.len(), 0);
        let input = recorded.auto_requests[0]
            .user_input
            .as_deref()
            .expect("history input");
        assert!(input.contains("history_index:"), "{input}");
        assert!(
            input.contains("terminal-output://session-1/cmd-1"),
            "{input}"
        );
        assert_eq!(state.evidence_requests.audit_records.len(), 1);
        assert_eq!(
            state.evidence_requests.audit_records[0].id,
            "cosh-request-1"
        );
        assert_eq!(
            state.evidence_requests.audit_records[0].raw_block,
            "```cosh-request\nhistory\n```"
        );
    }

    #[test]
    fn records_history_request_as_card_when_command_redacts() {
        let mut state = InlineState::default();
        let mut block = command_block("/tmp/missing-output");
        block.command = "echo token=super-secret".to_string();
        state.session_blocks = vec![block];
        let mut active_run = test_active_run();
        active_run.pending_cosh_requests = vec![ParsedCoshRequest {
            request: CoshRequest::History,
            ignored_multiple_request_blocks: false,
        }];

        let recorded = record_cosh_requests_from_active_run(&mut state, &mut active_run);

        assert!(recorded.auto_requests.is_empty());
        assert_eq!(recorded.card_ids, vec!["evidence-1"]);
        assert_eq!(state.evidence_requests.pending.len(), 1);
    }

    #[test]
    fn records_history_request_as_card_in_recommend_mode() {
        let mut state = InlineState {
            approval_mode: CoshApprovalMode::Recommend,
            ..InlineState::default()
        };
        state.session_blocks = vec![command_block("/tmp/missing-output")];
        let mut active_run = test_active_run();
        active_run.pending_cosh_requests = vec![ParsedCoshRequest {
            request: CoshRequest::History,
            ignored_multiple_request_blocks: false,
        }];

        let recorded = record_cosh_requests_from_active_run(&mut state, &mut active_run);

        assert!(recorded.auto_requests.is_empty());
        assert_eq!(recorded.card_ids, vec!["evidence-1"]);
        assert_eq!(state.evidence_requests.pending.len(), 1);
    }

    #[test]
    fn does_not_start_follow_up_when_provider_tool_turn_is_open() {
        let mut state = InlineState::default();
        state.session_blocks = vec![command_block("/tmp/missing-output")];
        let mut active_run = test_active_run();
        active_run
            .governed_events
            .push(governed(AgentEvent::ToolCall {
                run_id: "request-1".to_string(),
                tool_id: Some("toolu-open".to_string()),
                name: "Read".to_string(),
                input: "{\"file_path\":\"README.md\"}".to_string(),
            }));
        active_run.pending_cosh_requests = vec![ParsedCoshRequest {
            request: CoshRequest::History,
            ignored_multiple_request_blocks: false,
        }];

        let recorded = record_cosh_requests_from_active_run(&mut state, &mut active_run);

        assert!(recorded.auto_requests.is_empty());
        assert!(recorded.card_ids.is_empty());
        assert_eq!(state.evidence_requests.pending.len(), 0);
        assert!(
            recorded
                .notices
                .iter()
                .any(|notice| notice.contains("provider tool turn is still open")),
            "{:?}",
            recorded.notices
        );
    }

    #[test]
    fn closed_provider_tool_turn_allows_history_follow_up() {
        let mut state = InlineState::default();
        state.session_blocks = vec![command_block("/tmp/missing-output")];
        let mut active_run = test_active_run();
        active_run
            .governed_events
            .push(governed(AgentEvent::ToolCall {
                run_id: "request-1".to_string(),
                tool_id: Some("toolu-closed".to_string()),
                name: "Read".to_string(),
                input: "{\"file_path\":\"README.md\"}".to_string(),
            }));
        active_run
            .governed_events
            .push(governed(AgentEvent::ToolCompleted {
                run_id: "request-1".to_string(),
                tool_id: "toolu-closed".to_string(),
                status: "completed".to_string(),
            }));
        active_run.pending_cosh_requests = vec![ParsedCoshRequest {
            request: CoshRequest::History,
            ignored_multiple_request_blocks: false,
        }];

        let recorded = record_cosh_requests_from_active_run(&mut state, &mut active_run);

        assert_eq!(recorded.auto_requests.len(), 1);
        assert!(recorded.card_ids.is_empty());
        assert!(recorded.notices.is_empty());
    }

    #[test]
    fn records_invalid_request_block_audit_without_evidence_request() {
        let mut state = InlineState::default();
        let mut active_run = test_active_run();
        active_run.pending_cosh_request_audits =
            vec![crate::evidence::stream::CoshRequestAuditRecord {
                raw_block: "```cosh-request\nread /tmp/out\n```".to_string(),
                outcome: crate::evidence::stream::CoshRequestAuditOutcome::Invalid,
                reason: "parse_error",
            }];

        let recorded = record_cosh_requests_from_active_run(&mut state, &mut active_run);

        assert!(recorded.auto_requests.is_empty());
        assert!(recorded.card_ids.is_empty());
        assert_eq!(state.evidence_requests.audit_records.len(), 1);

        let mut output = Vec::new();
        crate::runtime::details::render_runtime_details(&state, &[], "cosh-request-1", &mut output)
            .expect("render details");
        let rendered = String::from_utf8(output).expect("utf8 details");
        assert!(rendered.contains("cosh-request details"), "{rendered}");
        assert!(rendered.contains("outcome: invalid"), "{rendered}");
        assert!(rendered.contains("reason: parse_error"), "{rendered}");
        assert!(rendered.contains("read /tmp/out"), "{rendered}");
    }

    #[test]
    fn evidence_follow_ups_keep_session_and_plain_user_payload() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-evidence-follow-up-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let output_ref = dir.join("cmd-1.txt");
        std::fs::write(&output_ref, "one\ntwo\nthree\n").expect("write output");
        let mut block = command_block(output_ref.to_str().expect("utf8 output path"));
        block.command =
            "curl --token cli-secret https://example.test/?secret=query-secret".to_string();
        let history_request = RuntimeEvidenceRequest {
            id: "evidence-1".to_string(),
            kind: RuntimeEvidenceRequestKind::History,
            ignored_multiple_request_blocks: false,
            audit_id: None,
        };
        let output_request = RuntimeEvidenceRequest {
            id: "evidence-2".to_string(),
            kind: RuntimeEvidenceRequestKind::Output(EvidenceExcerptRequest {
                output_id: "terminal-output://session-1/cmd-1".to_string(),
                direction: OutputExcerptDirection::Tail,
                lines: Some(2),
            }),
            ignored_multiple_request_blocks: false,
            audit_id: None,
        };

        let history =
            agent_request_from_evidence_request(std::slice::from_ref(&block), &history_request, 1)
                .expect("history follow-up");
        let output =
            agent_request_from_evidence_request(std::slice::from_ref(&block), &output_request, 2)
                .expect("output follow-up");

        for request in [history, output] {
            assert_eq!(request.session_id, "session-1");
            assert_eq!(request.command_block.id, "cmd-1");
            assert_eq!(request.mode, AgentMode::RecommendOnly);
            assert!(request.user_confirmed);
            assert!(request.context_blocks.is_empty());
            assert!(request.context_hints.is_empty());
            let input = request.user_input.as_deref().expect("plain user input");
            assert!(input.starts_with("ShellEvidenceExcerpt\n"), "{input}");
            assert!(input.contains("command:"), "{input}");
            assert!(input.contains("--token <redacted>"), "{input}");
            assert!(input.contains("secret=<redacted>"), "{input}");
            assert!(!input.contains("cli-secret"), "{input}");
            assert!(!input.contains("query-secret"), "{input}");
            assert!(!input.contains("tool_result"), "{input}");
            assert!(!input.contains("host_executed_shell"), "{input}");
            assert!(!input.contains("control_response"), "{input}");
        }
    }

    #[test]
    fn records_only_first_output_request_from_provider_response() {
        let mut state = InlineState::default();
        let mut active_run = test_active_run();
        active_run.pending_cosh_requests = vec![
            ParsedCoshRequest {
                request: CoshRequest::Output(EvidenceExcerptRequest {
                    output_id: "terminal-output://session-1/cmd-1".to_string(),
                    direction: OutputExcerptDirection::Tail,
                    lines: None,
                }),
                ignored_multiple_request_blocks: false,
            },
            ParsedCoshRequest {
                request: CoshRequest::Output(EvidenceExcerptRequest {
                    output_id: "terminal-output://session-1/cmd-2".to_string(),
                    direction: OutputExcerptDirection::Head,
                    lines: None,
                }),
                ignored_multiple_request_blocks: false,
            },
        ];

        let recorded = record_cosh_requests_from_active_run(&mut state, &mut active_run);

        assert!(recorded.auto_requests.is_empty());
        assert_eq!(recorded.card_ids, vec!["evidence-1".to_string()]);
        assert_eq!(state.evidence_requests.pending.len(), 1);
        assert!(matches!(
            state.evidence_requests.pending[0].kind,
            RuntimeEvidenceRequestKind::Output(_)
        ));
        assert!(state.evidence_requests.pending[0].ignored_multiple_request_blocks);
    }

    #[test]
    fn clear_pending_evidence_requests_drops_pending_cards() {
        let mut state = InlineState::default();
        let mut active_run = test_active_run();
        active_run.pending_cosh_requests = vec![ParsedCoshRequest {
            request: CoshRequest::Output(EvidenceExcerptRequest {
                output_id: "terminal-output://session-1/cmd-1".to_string(),
                direction: OutputExcerptDirection::Tail,
                lines: None,
            }),
            ignored_multiple_request_blocks: false,
        }];

        let recorded = record_cosh_requests_from_active_run(&mut state, &mut active_run);
        assert_eq!(recorded.card_ids, vec!["evidence-1".to_string()]);
        state
            .evidence_requests
            .rendered
            .insert("evidence-1".to_string());

        clear_pending_evidence_requests(&mut state);

        assert!(state.evidence_requests.pending.is_empty());
        assert!(state.evidence_requests.rendered.is_empty());
    }

    fn command_block(output_ref: &str) -> CommandBlock {
        CommandBlock {
            id: "cmd-1".to_string(),
            session_id: "session-1".to_string(),
            command: "printf 'one\\ntwo\\nthree\\n'".to_string(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            status: CommandStatus::Completed,
            exit_code: 0,
            duration_ms: 10,
            output: OutputRefs {
                terminal_output_ref: Some(output_ref.to_string()),
                terminal_output_bytes: 14,
            },
            started_at_ms: 1,
            ended_at_ms: 11,
        }
    }

    fn test_active_run() -> ActiveAgentRun {
        let request = AgentRequest {
            id: "request-1".to_string(),
            session_id: "session-1".to_string(),
            command_block: command_block("/tmp/missing-output"),
            context_blocks: Vec::new(),
            context_hints: Vec::new(),
            user_input: Some("hello".to_string()),
            findings: Vec::new(),
            mode: AgentMode::RecommendOnly,
            user_confirmed: true,
            hook_finding: None,
            recommended_skill: None,
        };
        let adapter = AdapterInstance::Fake(FakeAgentAdapter);
        let handle = adapter.start_cancellable(request.clone(), CoshApprovalMode::Recommend);
        let renderer = RatatuiInlineRenderer::for_terminal();
        ActiveAgentRun {
            request,
            handle,
            provider_name: "fake",
            language: cosh_shell::Language::EnUs,
            renderer: renderer.clone(),
            status_animation: renderer.status_animation(),
            markdown_stream: renderer.stream_markdown_agent(),
            governed_events: Vec::new(),
            deferred_events: Vec::new(),
            held_events: Vec::new(),
            cosh_request_filter: crate::evidence::stream::CoshRequestStreamFilter::default(),
            pending_cosh_requests: Vec::new(),
            pending_cosh_request_audits: Vec::new(),
            rendered_governed_event_count: 0,
            selectable_after_event_index: None,
            started_at: std::time::Instant::now(),
            last_activity_at: std::time::Instant::now(),
            last_heartbeat_at: std::time::Instant::now(),
            current_phase: String::new(),
            current_message: String::new(),
            has_visible_text_delta: false,
            completed: false,
        }
    }

    fn governed(event: AgentEvent) -> GovernedEvent {
        GovernedEvent {
            decision: cosh_shell::types::GovernanceDecision::Display,
            policy_decision: cosh_shell::types::GovernancePolicyDecision::DisplayOnly,
            event,
            reason: "test".to_string(),
            display_text: "test".to_string(),
            auto_execute: false,
        }
    }
}
