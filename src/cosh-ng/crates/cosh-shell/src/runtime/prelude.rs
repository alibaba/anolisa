pub(crate) use std::collections::HashSet;
pub(crate) use std::io::Write;

pub(crate) use cosh_shell::adapter::ApprovalResponse;
pub(crate) use cosh_shell::governance::govern_agent_events_with_language;
pub(crate) use cosh_shell::parser::{
    agent_request_after_confirmation, agent_request_from_intercepted_input,
    approval_command_from_event, event_cancels_failed_command_analysis,
    event_confirms_failed_command_analysis, findings_from_blocks, ApprovalCommandKind,
};
pub(crate) use cosh_shell::tools::can_run_approved_bash_tool;
pub(crate) use cosh_shell::{
    agent_render::{
        ApprovalDetailsPanelModel, ApprovalJournalEntryModel, ApprovalJournalPanelModel,
        ApprovalPanelAction, ApprovalPanelModel, ApprovalReceiptPanelModel, NoticePanelModel,
        RatatuiInlineRenderer,
    },
    types::{
        AgentEvent, AgentMode, AgentRequest, CommandBlock, CommandStatus, Finding, GovernedEvent,
        OutputRefs, Policy, ShellEvent, ShellEventKind,
    },
    AdapterInstance, AgentAdapter,
};

#[cfg(test)]
pub(crate) use cosh_shell::adapter::FakeAgentAdapter;

pub(crate) use crate::activity::runtime::{
    record_activity_rows_with_policy, render_activity_rows,
    render_provider_native_shell_transcript, ActivityRecordPolicy,
};
pub(crate) use crate::agent::events::flush_held_agent_events;
pub(crate) use crate::agent::failed_command::{
    start_agent_for_block, FailedCommandAnalysisTrigger,
};
pub(crate) use crate::agent::run::{start_agent_run, stop_active_agent_run_without_rendering};
pub(crate) use crate::approval::approved_tool::{
    request_is_executable_bash_tool, request_is_readonly_builtin_tool,
};
pub(crate) use crate::approval::handoff::{
    command_matches_trust_key, queue_approved_shell_handoff,
};
pub(crate) use crate::approval::panel::render_approval_requests;
pub(crate) use crate::approval::requests::{
    approval_request_from_governed_event, record_approval_requests, record_auto_approved_request,
    record_deferred_fallback_request,
};
pub(crate) use crate::approval::runtime::render_approval_resolution;
pub(crate) use crate::hooks::runtime::hook_routing_hints_for_block;
pub(crate) use crate::question::runtime::{
    agent_request_from_pending_question_answer, has_pending_question, record_user_questions,
    render_question_answer_notice, render_user_questions,
};
pub(crate) use crate::recommendation::runtime::{
    record_selectable_recommendations, render_selectable_recommendations,
};
pub(crate) use crate::runtime::continuity::{
    continuity_debug_lines, continuity_prompt_hint, record_agent_run_facts, record_user_intent,
};
pub(crate) use crate::runtime::details::render_runtime_details;
pub(crate) use crate::runtime::dispatcher::stable_event_key;
pub(crate) use crate::runtime::state::{
    AnalysisMode, ApprovalRequestKind, ApprovalRequestStatus, CoshApprovalMode, InlineState,
    ProviderShellRequestKind, RuntimeApprovalJournalEntry, RuntimeApprovalRequest,
};
