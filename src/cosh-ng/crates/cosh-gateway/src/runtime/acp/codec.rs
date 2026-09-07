//! Stateful ACP v1 JSON-RPC codec built from official SDK wire types.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_client_protocol::schema::{
    v1::{
        AgentCapabilities, CancelNotification, ClientNotification, ClientRequest, ContentBlock,
        Error as AcpError, Implementation, InitializeRequest, InitializeResponse,
        NewSessionRequest, NewSessionResponse, PermissionOptionKind, PromptRequest, PromptResponse,
        RequestId, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        Response, SelectedPermissionOutcome, SessionNotification, StopReason, TextContent,
        CLIENT_METHOD_NAMES,
    },
    ProtocolVersion,
};
use agent_client_protocol::{
    RawJsonRpcMessage, RawJsonRpcParams, TransportBatch, TransportBatchEntry, TransportFrame,
};

use super::types::{
    AcpV1AgentCapabilities, AcpV1AgentInfo, AcpV1ClientConfig, AcpV1CodecError, AcpV1Observation,
    AcpV1PermissionDecision, AcpV1PermissionOption, AcpV1PermissionOptionKind,
    AcpV1PermissionRequest, AcpV1ProtocolPhase, AcpV1RequestId, AcpV1RequestKind, AcpV1StopReason,
    ACP_WIRE_PROTOCOL_VERSION,
};

const MAX_ACP_FRAME_BYTES: usize = 1024 * 1024;
const MAX_ACP_BATCH_ENTRIES: usize = 1024;
const MAX_PENDING_CLIENT_REQUESTS: usize = 64;

// Codec phases share pending-request invariants in one private namespace; the
// fragments separate ownership without exposing those invariants as an API.
include!("codec/model.rs");
include!("codec/outbound.rs");
include!("codec/decode.rs");
include!("codec/encode.rs");
include!("codec/support.rs");
