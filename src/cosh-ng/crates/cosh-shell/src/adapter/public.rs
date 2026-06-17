#[allow(dead_code, unused_imports)]
#[path = "mod.rs"]
mod implementation;

pub use implementation::{
    adapter_for_kind, AdapterError, AdapterInstance, AdapterKind, AgentAdapter,
    AgentBackendCapabilities, AgentRunHandle, AgentRunPoll, ApprovalDecision, ApprovalResponse,
    AuthFieldInfo, AuthProviderInfo, AuthResponse, ClaudeCodeAdapter, ControlProtocolCapabilities,
    CoshTuiAdapter, FakeAgentAdapter, HostExecutedShellMetadata, HostExecutedShellResult,
    ProviderCancellationArtifact, ProviderCancellationArtifactKind,
    ProviderCancellationArtifactStore, QwenCliAdapter,
};

#[allow(unused_imports)]
pub(crate) use implementation::{prompt_from_request, provider_prompt_contract};
