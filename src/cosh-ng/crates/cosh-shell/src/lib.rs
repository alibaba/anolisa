pub mod adapter;
pub mod agent_render;
mod approval_actions;
pub mod builtin_hooks;
pub mod exit_classify;
pub mod governance;
pub mod hook_engine;
pub mod hook_types;
pub mod input;
pub mod interactive;
pub mod journal;
pub mod ledger;
pub mod parser;
mod question_choices;
mod raw_input;
pub mod renderer;
pub mod shell_host;
pub mod tool_broker;
pub mod tool_display;
pub mod types;

pub use builtin_hooks::*;
pub use exit_classify::*;
pub use hook_engine::*;
pub use hook_types::*;
pub use adapter::{
    adapter_for_kind, AdapterError, AdapterInstance, AdapterKind, AgentAdapter,
    AgentBackendCapabilities, AgentRunHandle, AgentRunPoll, ClaudeCodeAdapter, FakeAgentAdapter,
    PreparedInvocation, QwenCliAdapter,
};
pub use agent_render::RatatuiInlineRenderer;
pub use governance::{govern_agent_events, GovernanceOutput};
pub use input::{InputClassifier, InputDecision, InterceptReason};
pub use interactive::{run_line_interactive_bash, LineInteractiveOutput};
pub use journal::{read_shell_events, write_shell_events};
pub use ledger::{build_command_blocks, LedgerOutput};
pub use parser::{
    agent_request_after_confirmation, agent_request_confirmed_by_events,
    agent_request_from_intercepted_input, approval_command_from_event,
    event_cancels_failed_command_analysis, event_confirms_failed_command_analysis,
    event_requests_agent_cancel, findings_from_blocks, interventions_from_findings,
    recommendation_action_from_event, recommendation_selection_from_event, ApprovalCommand,
    ApprovalCommandKind, RecommendationAction, RecommendationActionKind,
};
pub use question_choices::{
    question_choice_count, question_custom_answer_index, toggle_question_option,
};
pub use raw_input::{RawInputCapture, RawObserverAction, RawRelayAction};
pub use renderer::render_transcript;
pub use shell_host::{
    run_raw_interactive_bash, run_raw_interactive_bash_with_observer,
    run_raw_interactive_bash_with_output_control, run_raw_interactive_zsh_with_output_control,
    run_raw_relay_bash, run_raw_relay_bash_with_actions, run_raw_relay_bash_with_actions_observer,
    run_raw_relay_bash_with_actions_output_control, run_raw_relay_bash_with_observer,
    run_raw_relay_bash_with_output_control, run_raw_relay_zsh_with_actions,
    run_raw_relay_zsh_with_output_control, run_scripted_bash, run_scripted_zsh,
    run_streaming_line_bash, ScriptedInput, ShellHostConfig, ShellHostOutput,
};
pub use tool_broker::{
    can_run_approved_bash_tool, run_approved_bash_tool, ToolExecutionResult, ToolExecutionStatus,
};
pub use tool_display::*;
pub use types::*;
