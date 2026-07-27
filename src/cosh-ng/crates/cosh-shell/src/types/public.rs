#[allow(dead_code)]
#[path = "mod.rs"]
mod implementation;

pub use implementation::{
    AgentEvent, AgentMode, AgentRequest, AuditRecord, CommandBlock, CommandOrigin, CommandStatus,
    CoshApprovalMode, Finding, FindingKind, FindingSeverity, GovernanceDecision,
    GovernancePolicyDecision, GovernedEvent, HookFinding, Intervention, InterventionDecision,
    OutputRefs, Policy, QuestionSelectionMode, ShellCaptureLifecycle, ShellCaptureMetadata,
    ShellCommandAuditIdentity, ShellEvent, ShellEventKind, ShellHandoffRequest,
    ShellRoutingMetadata, COMMAND_OUTPUT_REF_MAX_BYTES, SESSION_OUTPUT_REF_MAX_BYTES,
};

#[allow(unused_imports)]
pub(crate) use implementation::{
    request_is_analysis_only_continuation, set_request_context_binding, AgentContextBinding,
    BuiltinFactRecord, BuiltinFindingFacts, EvaluatedHookFinding, HighMemoryProcessFacts,
    HookProvenance, MemoryPressureFacts, MetricsConfidence, ProcessMemoryFact,
    ShellEnvironmentSnapshot, SHELL_HANDOFF_CONTINUATION_HINT, SHELL_HANDOFF_UNTRACKED_STATUS,
    USER_APPROVAL_MODE_HINT_PREFIX,
};

pub(crate) use implementation::audit;
