//! Supervised child-process primitives and private runtime protocol codecs.
//!
//! This module deliberately stops at the process/protocol boundary. Public
//! Task, Run, Agent, and Runtime identities and events belong to
//! `cosh-gateway-contracts` and are mapped by a higher-level bridge.

mod acp;
mod bounded_io;
mod cosh_core_jsonl;
mod process_group;
mod profile;
mod session_driver;
mod supervisor;

pub use acp::{
    AcpV1AgentCapabilities, AcpV1AgentInfo, AcpV1BridgeError, AcpV1BridgeRead, AcpV1ClientConfig,
    AcpV1Codec, AcpV1CodecError, AcpV1Observation, AcpV1PermissionDecision, AcpV1PermissionOption,
    AcpV1PermissionOptionKind, AcpV1PermissionRequest, AcpV1ProtocolPhase, AcpV1RequestId,
    AcpV1RequestKind, AcpV1RuntimeBridge, AcpV1StopReason, ACP_WIRE_PROTOCOL_VERSION,
};
pub use bounded_io::{BoundedLineError, BoundedLineReader, StderrSnapshot};
pub use cosh_core_jsonl::{
    CoshCoreAssistantBody, CoshCoreAssistantMessage, CoshCoreCapabilities, CoshCoreCodecError,
    CoshCoreContentBlock, CoshCoreContentBlockInfo, CoshCoreContentDelta, CoshCoreControlRequest,
    CoshCoreControlRequestEnvelope, CoshCoreControlResponse, CoshCoreJsonlCodec,
    CoshCoreObservation, CoshCoreProtocolPhase, CoshCoreResult, CoshCoreShellContext,
    CoshCoreStreamEvent, CoshCoreSystemMessage, CoshCoreToolResult, CoshCoreUserTurn,
    PRIVATE_COSH_CONTROL_PROTOCOL_VERSION,
};
pub use process_group::{PlatformProcessGroup, ProcessGroupLifecycle};
pub use profile::{
    built_in_acp_runtime_profiles, AcpRuntimeProfile, AcpRuntimeProfileId,
    AcpRuntimeProfileLaunchError, AcpRuntimeProfileRequest, AcpRuntimeProfileResolveError,
    AcpRuntimeProfileResolver, ResolvedAcpRuntimeProfile,
};
pub use session_driver::{
    AcpSessionControl, AcpSessionDriver, AcpSessionDriverConfig, AcpSessionDriverError,
    AcpSessionEvent, AcpSessionTerminal, AcpSessionTerminalKind,
};
pub use supervisor::{
    ProcessExit, ProcessTerminal, RuntimeFrameRead, RuntimeLaunchError, RuntimeLaunchSpec,
    RuntimeState, RuntimeSupervisor, RuntimeSupervisorError,
};
