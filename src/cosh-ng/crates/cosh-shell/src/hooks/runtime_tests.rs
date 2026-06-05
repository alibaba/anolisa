use super::*;
use cosh_shell::hook_types::{FindingSeverity, HookFinding};

#[path = "runtime_tests/active_agent.rs"]
mod active_agent;
#[path = "runtime_tests/aggregation.rs"]
mod aggregation;
#[path = "runtime_tests/analyze.rs"]
mod analyze;
#[path = "runtime_tests/command_intent.rs"]
mod command_intent;
#[path = "runtime_tests/confidence.rs"]
mod confidence;
#[path = "runtime_tests/consultation_queue.rs"]
mod consultation_queue;
#[path = "runtime_tests/display_policy.rs"]
mod display_policy;
#[path = "runtime_tests/feedback_policy.rs"]
mod feedback_policy;
#[path = "runtime_tests/hint_details.rs"]
mod hint_details;
#[path = "runtime_tests/session_input.rs"]
mod session_input;
#[path = "runtime_tests/suppression_budget.rs"]
mod suppression_budget;

fn finding(hook_id: &str, severity: FindingSeverity) -> HookFinding {
    HookFinding {
        hook_id: hook_id.to_string(),
        severity,
        title: format!("{hook_id} title"),
        description: format!("{hook_id} description"),
        suggestion: format!("{hook_id} suggestion"),
        skill: Some("memory-analysis".to_string()),
        cli_hint: Some("free -m".to_string()),
        context_refs: Vec::new(),
    }
}

fn process_finding(title: &str) -> HookFinding {
    let mut finding = finding("high-memory-process", FindingSeverity::Warning);
    finding.title = title.to_string();
    finding
}

fn process_finding_with_severity(title: &str, severity: FindingSeverity) -> HookFinding {
    let mut finding = finding("high-memory-process", severity);
    finding.title = title.to_string();
    finding
}

fn external_finding(hook_id: &str, severity: FindingSeverity, skill: Option<&str>) -> HookFinding {
    let mut finding = finding(hook_id, severity);
    finding.skill = skill.map(String::from);
    finding
}

fn mark_consultation_idle(consultation: &mut PendingConsultation) {
    consultation.queued_at = std::time::Instant::now()
        - SUCCESS_CONSULTATION_IDLE_GRACE
        - std::time::Duration::from_millis(1);
}

fn mark_front_consultation_idle(state: &mut InlineState) {
    let consultation = state
        .hooks
        .pending_consultation_queue
        .front_mut()
        .expect("queued consultation");
    mark_consultation_idle(consultation);
}

fn block(exit_code: i32) -> CommandBlock {
    CommandBlock {
        id: "cmd-1".to_string(),
        session_id: "session".to_string(),
        command: "top -b -n1".to_string(),
        cwd: "/tmp".to_string(),
        end_cwd: "/tmp".to_string(),
        started_at_ms: 10,
        ended_at_ms: 20,
        duration_ms: 10,
        exit_code,
        status: if exit_code == 0 {
            CommandStatus::Completed
        } else {
            CommandStatus::Failed
        },
        output: OutputRefs {
            terminal_output_ref: Some("/tmp/out".to_string()),
            terminal_output_bytes: 10,
        },
    }
}

fn block_with_command(command: &str) -> CommandBlock {
    let mut block = block(0);
    block.command = command.to_string();
    block
}

fn block_with_command_at(command: &str, ended_at_ms: u64) -> CommandBlock {
    let mut block = block_with_command(command);
    block.ended_at_ms = ended_at_ms;
    block
}
