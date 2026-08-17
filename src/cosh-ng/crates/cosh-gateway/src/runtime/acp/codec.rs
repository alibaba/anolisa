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

#[derive(Debug, Clone)]
struct PendingOutboundRequest {
    kind: AcpV1RequestKind,
    session_id: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingPermission {
    option_ids: BTreeMap<String, AcpV1PermissionOptionKind>,
    response_destination: InboundResponseDestination,
}

#[derive(Debug, Clone, Copy)]
enum InboundResponseDestination {
    Individual,
    Batch { batch_id: u64, slot: usize },
}

#[derive(Debug, Clone)]
struct PendingInboundBatch {
    responses: Vec<Option<RawJsonRpcMessage>>,
}

#[derive(Debug)]
pub(crate) struct AcpV1DecodedFrame {
    pub(crate) observations: Vec<AcpV1Observation>,
    pub(crate) outbound_frames: Vec<String>,
}

/// Stateful encoder and decoder for one ACP v1 process generation.
#[derive(Debug, Clone)]
pub struct AcpV1Codec {
    config: AcpV1ClientConfig,
    phase: AcpV1ProtocolPhase,
    next_request_sequence: u64,
    next_inbound_batch_sequence: u64,
    pending_outbound: BTreeMap<AcpV1RequestId, PendingOutboundRequest>,
    pending_permissions: BTreeMap<AcpV1RequestId, PendingPermission>,
    pending_unsupported: BTreeMap<AcpV1RequestId, InboundResponseDestination>,
    pending_inbound_batches: BTreeMap<u64, PendingInboundBatch>,
    capabilities: Option<AcpV1AgentCapabilities>,
    session_id: Option<String>,
    prompt_request_id: Option<AcpV1RequestId>,
    cancellation_sent: bool,
}

impl AcpV1Codec {
    /// Creates one codec with an explicit client identity and frame bound.
    ///
    /// # Errors
    ///
    /// Rejects empty implementation metadata and a zero frame bound.
    pub fn new(config: AcpV1ClientConfig) -> Result<Self, AcpV1CodecError> {
        if config.max_frame_bytes == 0 || config.max_frame_bytes > MAX_ACP_FRAME_BYTES {
            return Err(AcpV1CodecError::InvalidFrameLimit {
                actual: config.max_frame_bytes,
                maximum: MAX_ACP_FRAME_BYTES,
            });
        }
        if config.name.trim().is_empty() {
            return Err(AcpV1CodecError::InvalidClientInfo { field: "name" });
        }
        if config.version.trim().is_empty() {
            return Err(AcpV1CodecError::InvalidClientInfo { field: "version" });
        }
        Ok(Self {
            config,
            phase: AcpV1ProtocolPhase::Created,
            next_request_sequence: 1,
            next_inbound_batch_sequence: 1,
            pending_outbound: BTreeMap::new(),
            pending_permissions: BTreeMap::new(),
            pending_unsupported: BTreeMap::new(),
            pending_inbound_batches: BTreeMap::new(),
            capabilities: None,
            session_id: None,
            prompt_request_id: None,
            cancellation_sent: false,
        })
    }

    /// Returns the current ACP protocol phase.
    #[must_use]
    pub fn phase(&self) -> AcpV1ProtocolPhase {
        self.phase
    }

    /// Returns the opaque session bound after `session/new` succeeds.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Encodes the mandatory ACP v1 initialize request.
    ///
    /// # Errors
    ///
    /// Rejects repeated initialization and frames above the configured bound.
    pub fn initialize_frame(&mut self) -> Result<String, AcpV1CodecError> {
        self.require_phase(AcpV1ProtocolPhase::Created, "initialize_frame")?;
        let request = InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new(
            self.config.name.clone(),
            self.config.version.clone(),
        ));
        let (id, frame) = self.encode_request(ClientRequest::InitializeRequest(request))?;
        self.pending_outbound.insert(
            id,
            PendingOutboundRequest {
                kind: AcpV1RequestKind::Initialize,
                session_id: None,
            },
        );
        self.phase = AcpV1ProtocolPhase::AwaitingInitialize;
        Ok(frame)
    }

    /// Encodes `session/new` for one pinned workspace.
    ///
    /// # Errors
    ///
    /// Requires successful initialization, absolute roots, no existing
    /// session, and advertised additional-directory support when used.
    pub fn new_session_frame(
        &mut self,
        workspace: impl Into<PathBuf>,
        additional_directories: Vec<PathBuf>,
    ) -> Result<String, AcpV1CodecError> {
        self.require_phase(AcpV1ProtocolPhase::Ready, "new_session_frame")?;
        if self.session_id.is_some()
            || self
                .pending_outbound
                .values()
                .any(|pending| pending.kind == AcpV1RequestKind::NewSession)
        {
            return Err(AcpV1CodecError::SessionAlreadyBound);
        }
        let workspace = workspace.into();
        validate_absolute(&workspace)?;
        for directory in &additional_directories {
            validate_absolute(directory)?;
        }
        if !additional_directories.is_empty()
            && !self
                .capabilities
                .is_some_and(|capabilities| capabilities.additional_directories)
        {
            return Err(AcpV1CodecError::UnsupportedCapability(
                "session.additionalDirectories",
            ));
        }

        let request =
            NewSessionRequest::new(workspace).additional_directories(additional_directories);
        let (id, frame) = self.encode_request(ClientRequest::NewSessionRequest(request))?;
        self.pending_outbound.insert(
            id,
            PendingOutboundRequest {
                kind: AcpV1RequestKind::NewSession,
                session_id: None,
            },
        );
        Ok(frame)
    }

    /// Encodes one text-only prompt in the bound Agent session.
    ///
    /// # Errors
    ///
    /// Requires an open session, non-empty text, and no active prompt.
    pub fn prompt_frame(&mut self, text: impl Into<String>) -> Result<String, AcpV1CodecError> {
        self.require_phase(AcpV1ProtocolPhase::Ready, "prompt_frame")?;
        let session_id = self
            .session_id
            .clone()
            .ok_or(AcpV1CodecError::SessionNotOpen)?;
        if self.prompt_request_id.is_some() {
            return Err(AcpV1CodecError::PromptAlreadyActive);
        }
        let text = text.into();
        if text.trim().is_empty() {
            return Err(AcpV1CodecError::EmptyPrompt);
        }
        let request = PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(text))],
        );
        let (id, frame) = self.encode_request(ClientRequest::PromptRequest(request))?;
        self.pending_outbound.insert(
            id.clone(),
            PendingOutboundRequest {
                kind: AcpV1RequestKind::Prompt,
                session_id: Some(session_id),
            },
        );
        self.prompt_request_id = Some(id);
        self.cancellation_sent = false;
        Ok(frame)
    }

    /// Encodes `session/cancel` and cancels every pending permission callback.
    ///
    /// The first frame is the cancellation notification. Remaining frames are
    /// mandatory `cancelled` responses for outstanding permission requests.
    ///
    /// # Errors
    ///
    /// Requires an active prompt and rejects duplicate cancellation.
    pub fn cancel_frames(&mut self) -> Result<Vec<String>, AcpV1CodecError> {
        let mut candidate = self.clone();
        let frames = candidate.cancel_frames_inner()?;
        *self = candidate;
        Ok(frames)
    }

    fn cancel_frames_inner(&mut self) -> Result<Vec<String>, AcpV1CodecError> {
        self.require_phase(AcpV1ProtocolPhase::Ready, "cancel_frames")?;
        if self.prompt_request_id.is_none() {
            return Err(AcpV1CodecError::PromptNotActive);
        }
        if self.cancellation_sent {
            return Err(AcpV1CodecError::CancellationAlreadySent);
        }
        let session_id = self
            .session_id
            .clone()
            .ok_or(AcpV1CodecError::SessionNotOpen)?;
        let notification =
            ClientNotification::CancelNotification(CancelNotification::new(session_id));
        let mut frames = vec![self.encode_notification(notification)?];
        let permission_ids = self.pending_permissions.keys().cloned().collect::<Vec<_>>();
        for request_id in permission_ids {
            frames.extend(
                self.permission_response_frames(&request_id, AcpV1PermissionDecision::Cancelled)?,
            );
        }
        let unsupported_ids = self.pending_unsupported.keys().cloned().collect::<Vec<_>>();
        for request_id in unsupported_ids {
            let raw = RawJsonRpcMessage::response(
                to_sdk_request_id(&request_id),
                Err(AcpError::method_not_found()),
            );
            let destination = self
                .pending_unsupported
                .remove(&request_id)
                .ok_or_else(|| AcpV1CodecError::UnknownUnsupportedRequest(request_id.clone()))?;
            frames.extend(self.settle_inbound_response(destination, raw)?);
        }
        self.cancellation_sent = true;
        Ok(frames)
    }

    /// Encodes a response to one pending permission callback.
    ///
    /// # Errors
    ///
    /// Rejects unknown requests and selected option IDs not offered by the
    /// correlated Agent request.
    pub fn permission_response_frames(
        &mut self,
        request_id: &AcpV1RequestId,
        decision: AcpV1PermissionDecision,
    ) -> Result<Vec<String>, AcpV1CodecError> {
        let mut candidate = self.clone();
        let frames = candidate.permission_response_frames_inner(request_id, decision)?;
        *self = candidate;
        Ok(frames)
    }

    fn permission_response_frames_inner(
        &mut self,
        request_id: &AcpV1RequestId,
        decision: AcpV1PermissionDecision,
    ) -> Result<Vec<String>, AcpV1CodecError> {
        self.require_phase(AcpV1ProtocolPhase::Ready, "permission_response_frames")?;
        let pending = self
            .pending_permissions
            .get(request_id)
            .ok_or_else(|| AcpV1CodecError::UnknownPermissionRequest(request_id.clone()))?;
        let outcome = match decision {
            AcpV1PermissionDecision::Cancelled => RequestPermissionOutcome::Cancelled,
            AcpV1PermissionDecision::Selected { option_id } => {
                let Some(kind) = pending.option_ids.get(&option_id) else {
                    return Err(AcpV1CodecError::UnknownPermissionOption {
                        request_id: request_id.clone(),
                        option_id,
                    });
                };
                if !matches!(
                    kind,
                    AcpV1PermissionOptionKind::AllowOnce | AcpV1PermissionOptionKind::RejectOnce
                ) {
                    return Err(AcpV1CodecError::UnsupportedPermissionOption {
                        request_id: request_id.clone(),
                        option_id,
                    });
                }
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))
            }
        };
        let response = self.permission_outcome_message(request_id, outcome)?;
        let destination = self
            .pending_permissions
            .remove(request_id)
            .ok_or_else(|| AcpV1CodecError::UnknownPermissionRequest(request_id.clone()))?
            .response_destination;
        self.settle_inbound_response(destination, response)
    }

    /// Encodes a fail-closed method-not-found response for an unadvertised callback.
    ///
    /// # Errors
    ///
    /// Rejects request IDs that were not returned by
    /// [`AcpV1Observation::UnsupportedClientRequest`].
    pub fn reject_unsupported_request_frames(
        &mut self,
        request_id: &AcpV1RequestId,
    ) -> Result<Vec<String>, AcpV1CodecError> {
        let mut candidate = self.clone();
        let frames = candidate.reject_unsupported_request_frames_inner(request_id)?;
        *self = candidate;
        Ok(frames)
    }

    fn reject_unsupported_request_frames_inner(
        &mut self,
        request_id: &AcpV1RequestId,
    ) -> Result<Vec<String>, AcpV1CodecError> {
        self.require_phase(
            AcpV1ProtocolPhase::Ready,
            "reject_unsupported_request_frames",
        )?;
        let destination = self
            .pending_unsupported
            .remove(request_id)
            .ok_or_else(|| AcpV1CodecError::UnknownUnsupportedRequest(request_id.clone()))?;
        let raw = RawJsonRpcMessage::response(
            to_sdk_request_id(request_id),
            Err(AcpError::method_not_found()),
        );
        self.settle_inbound_response(destination, raw)
    }

    /// Decodes and validates one bounded ACP v1 JSON-RPC line.
    ///
    /// # Errors
    ///
    /// Rejects malformed frames, wrong ordering, identity mismatches, unknown
    /// responses, and invalid callback correlations. Any error makes the codec
    /// terminal so the supervising bridge can fail closed.
    pub fn decode_frame(&mut self, frame: &[u8]) -> Result<AcpV1Observation, AcpV1CodecError> {
        let decoded = self.decode_transport_frame(frame)?;
        if !decoded.outbound_frames.is_empty() || decoded.observations.len() != 1 {
            self.phase = AcpV1ProtocolPhase::Terminal;
            return Err(AcpV1CodecError::MultiMessageFrameRequiresBridge);
        }
        decoded
            .observations
            .into_iter()
            .next()
            .ok_or(AcpV1CodecError::MultiMessageFrameRequiresBridge)
    }

    /// Decodes one single-message or batch-aware ACP transport frame.
    ///
    /// Valid batch entries are processed independently in source order. JSON-RPC
    /// errors for malformed call-shaped entries are returned as outbound frames;
    /// malformed response-shaped entries are ignored as required by JSON-RPC.
    ///
    /// # Errors
    ///
    /// Rejects frame bounds, protocol ordering, identity, or correlation
    /// violations. Any error makes the codec terminal.
    pub(crate) fn decode_transport_frame(
        &mut self,
        frame: &[u8],
    ) -> Result<AcpV1DecodedFrame, AcpV1CodecError> {
        if self.phase == AcpV1ProtocolPhase::Terminal {
            return Err(self.invalid_phase("decode_transport_frame"));
        }
        let mut candidate = self.clone();
        match candidate.decode_transport_frame_inner(frame) {
            Ok(decoded) => {
                *self = candidate;
                Ok(decoded)
            }
            Err(error) => {
                // Never retain a successfully decoded batch prefix after a
                // later entry invalidates the entire protocol generation.
                self.phase = AcpV1ProtocolPhase::Terminal;
                Err(error)
            }
        }
    }

    /// Produces one terminal observation when runtime stdout closes.
    #[must_use]
    pub fn finish_stdout(&mut self) -> Option<AcpV1Observation> {
        if self.phase == AcpV1ProtocolPhase::Terminal {
            return None;
        }
        self.phase = AcpV1ProtocolPhase::Terminal;
        Some(AcpV1Observation::TransportClosed)
    }

    fn decode_transport_frame_inner(
        &mut self,
        frame: &[u8],
    ) -> Result<AcpV1DecodedFrame, AcpV1CodecError> {
        if frame.len() > self.config.max_frame_bytes {
            return Err(AcpV1CodecError::FrameTooLarge {
                limit: self.config.max_frame_bytes,
            });
        }
        let frame = std::str::from_utf8(frame).map_err(|_| AcpV1CodecError::InvalidUtf8)?;
        let frame = frame.trim_end_matches(['\r', '\n']);
        if frame.is_empty() {
            return Err(AcpV1CodecError::EmptyFrame);
        }
        if self.phase == AcpV1ProtocolPhase::Created {
            return Err(self.invalid_phase("decode_transport_frame"));
        }

        match TransportFrame::parse_json(frame) {
            TransportFrame::Single(message) => Ok(AcpV1DecodedFrame {
                observations: vec![
                    self.decode_message(message, InboundResponseDestination::Individual)?
                ],
                outbound_frames: Vec::new(),
            }),
            TransportFrame::Malformed { raw, error }
                if serde_json::from_str::<serde_json::Value>(&raw)
                    .is_ok_and(|value| value.as_array().is_some_and(Vec::is_empty)) =>
            {
                let response = RawJsonRpcMessage::response(RequestId::Null, Err(error));
                Ok(AcpV1DecodedFrame {
                    observations: Vec::new(),
                    outbound_frames: vec![self.encode_raw(&response)?],
                })
            }
            TransportFrame::Malformed { error, .. } => {
                // stdout is a dedicated supervised ACP stream, not a general
                // JSON-RPC server endpoint. Recovery would let log pollution
                // or a corrupted frame blur the process/protocol boundary.
                self.phase = AcpV1ProtocolPhase::Terminal;
                Err(AcpV1CodecError::Sdk(error.to_string()))
            }
            TransportFrame::Batch(batch) => self.decode_batch(batch),
        }
    }

    fn decode_message(
        &mut self,
        message: RawJsonRpcMessage,
        response_destination: InboundResponseDestination,
    ) -> Result<AcpV1Observation, AcpV1CodecError> {
        match message {
            RawJsonRpcMessage::Response(response) => self.decode_response(response),
            RawJsonRpcMessage::Notification(notification) => {
                self.require_phase(AcpV1ProtocolPhase::Ready, "decode_notification")?;
                self.decode_notification(notification.method.as_ref(), notification.params)
            }
            RawJsonRpcMessage::Request(request) => {
                self.require_phase(AcpV1ProtocolPhase::Ready, "decode_request")?;
                self.decode_request(
                    request.id,
                    request.method.as_ref(),
                    request.params,
                    response_destination,
                )
            }
        }
    }

    fn decode_batch(
        &mut self,
        batch: TransportBatch,
    ) -> Result<AcpV1DecodedFrame, AcpV1CodecError> {
        if batch.len() > MAX_ACP_BATCH_ENTRIES {
            return Err(AcpV1CodecError::BatchTooLarge {
                limit: MAX_ACP_BATCH_ENTRIES,
            });
        }

        let response_count = batch
            .entries()
            .filter(|entry| batch_entry_requires_response(entry))
            .count();
        let batch_id = if response_count == 0 {
            None
        } else {
            let batch_id = self.next_inbound_batch_sequence;
            self.next_inbound_batch_sequence = batch_id
                .checked_add(1)
                .ok_or(AcpV1CodecError::BatchIdExhausted)?;
            self.pending_inbound_batches.insert(
                batch_id,
                PendingInboundBatch {
                    responses: vec![None; response_count],
                },
            );
            Some(batch_id)
        };

        let mut next_response_slot = 0;
        let mut observations = Vec::with_capacity(batch.len());
        let mut outbound_frames = Vec::new();
        for entry in batch.into_entries() {
            let requires_response = batch_entry_requires_response(&entry);
            let response_destination = match (batch_id, requires_response) {
                (Some(batch_id), true) => {
                    let destination = InboundResponseDestination::Batch {
                        batch_id,
                        slot: next_response_slot,
                    };
                    next_response_slot += 1;
                    destination
                }
                _ => InboundResponseDestination::Individual,
            };
            match entry {
                TransportBatchEntry::Message(message) => {
                    observations.push(self.decode_message(message, response_destination)?);
                }
                TransportBatchEntry::Malformed { raw: _, error } => {
                    if requires_response {
                        let response = RawJsonRpcMessage::response(RequestId::Null, Err(error));
                        outbound_frames
                            .extend(self.settle_inbound_response(response_destination, response)?);
                    }
                }
            }
        }
        if let Some(batch_id) = batch_id {
            outbound_frames.extend(self.take_completed_batch(batch_id)?);
        }
        Ok(AcpV1DecodedFrame {
            observations,
            outbound_frames,
        })
    }

    fn decode_response(
        &mut self,
        response: Response<serde_json::Value>,
    ) -> Result<AcpV1Observation, AcpV1CodecError> {
        match response {
            Response::Result { id, result } => {
                let request_id = from_sdk_request_id(id)?;
                let pending = self
                    .pending_outbound
                    .remove(&request_id)
                    .ok_or_else(|| AcpV1CodecError::UnknownResponse(request_id.clone()))?;
                self.decode_success(pending, result)
            }
            Response::Error { id, error } => {
                let request_id = from_sdk_request_id(id)?;
                let pending = self
                    .pending_outbound
                    .remove(&request_id)
                    .ok_or_else(|| AcpV1CodecError::UnknownResponse(request_id.clone()))?;
                if pending.kind == AcpV1RequestKind::Prompt {
                    self.prompt_request_id = None;
                    self.cancellation_sent = false;
                }
                if pending.kind == AcpV1RequestKind::Initialize {
                    self.phase = AcpV1ProtocolPhase::Terminal;
                }
                Ok(AcpV1Observation::RequestFailed {
                    request: pending.kind,
                    code: i32::from(error.code),
                    message: error.message,
                })
            }
        }
    }

    fn decode_success(
        &mut self,
        pending: PendingOutboundRequest,
        result: serde_json::Value,
    ) -> Result<AcpV1Observation, AcpV1CodecError> {
        match pending.kind {
            AcpV1RequestKind::Initialize => self.decode_initialize_response(result),
            AcpV1RequestKind::NewSession => self.decode_new_session_response(result),
            AcpV1RequestKind::Prompt => self.decode_prompt_response(pending, result),
        }
    }

    fn decode_initialize_response(
        &mut self,
        result: serde_json::Value,
    ) -> Result<AcpV1Observation, AcpV1CodecError> {
        let response: InitializeResponse = serde_json::from_value(result)?;
        if response.protocol_version != ProtocolVersion::V1 {
            return Err(AcpV1CodecError::UnsupportedProtocolVersion {
                actual: response.protocol_version.as_u16(),
            });
        }
        let capabilities = copy_capabilities(&response.agent_capabilities);
        let agent_info = response.agent_info.map(|info| AcpV1AgentInfo {
            name: info.name,
            title: info.title,
            version: info.version,
        });
        self.capabilities = Some(capabilities);
        self.phase = AcpV1ProtocolPhase::Ready;
        Ok(AcpV1Observation::Initialized {
            agent_info,
            capabilities,
        })
    }

    fn decode_new_session_response(
        &mut self,
        result: serde_json::Value,
    ) -> Result<AcpV1Observation, AcpV1CodecError> {
        let response: NewSessionResponse = serde_json::from_value(result)?;
        let session_id = response.session_id.0.to_string();
        if self.session_id.replace(session_id.clone()).is_some() {
            return Err(AcpV1CodecError::SessionAlreadyBound);
        }
        Ok(AcpV1Observation::SessionOpened { session_id })
    }

    fn decode_prompt_response(
        &mut self,
        pending: PendingOutboundRequest,
        result: serde_json::Value,
    ) -> Result<AcpV1Observation, AcpV1CodecError> {
        let response: PromptResponse = serde_json::from_value(result)?;
        if !self.pending_permissions.is_empty() {
            return Err(AcpV1CodecError::PromptFinishedWithPendingPermissions {
                count: self.pending_permissions.len(),
            });
        }
        if !self.pending_unsupported.is_empty() {
            return Err(AcpV1CodecError::PromptFinishedWithPendingUnsupported {
                count: self.pending_unsupported.len(),
            });
        }
        let session_id = pending.session_id.ok_or(AcpV1CodecError::SessionNotOpen)?;
        self.require_session(&session_id)?;
        self.prompt_request_id = None;
        self.cancellation_sent = false;
        self.pending_permissions.clear();
        Ok(AcpV1Observation::PromptFinished {
            session_id,
            stop_reason: copy_stop_reason(response.stop_reason),
        })
    }

    fn decode_notification(
        &mut self,
        method: &str,
        params: Option<RawJsonRpcParams>,
    ) -> Result<AcpV1Observation, AcpV1CodecError> {
        if method != CLIENT_METHOD_NAMES.session_update {
            return Ok(AcpV1Observation::UnsupportedNotification {
                method: method.to_owned(),
            });
        }
        let notification: SessionNotification = decode_params(params)?;
        let session_id = notification.session_id.0.to_string();
        self.require_session(&session_id)?;
        if self.prompt_request_id.is_none() {
            return Err(AcpV1CodecError::PromptNotActive);
        }
        if self.cancellation_sent {
            return Err(AcpV1CodecError::CancellationAlreadySent);
        }
        let update = serde_json::to_value(notification.update)?;
        Ok(AcpV1Observation::SessionUpdate { session_id, update })
    }

    fn decode_request(
        &mut self,
        id: RequestId,
        method: &str,
        params: Option<RawJsonRpcParams>,
        response_destination: InboundResponseDestination,
    ) -> Result<AcpV1Observation, AcpV1CodecError> {
        let request_id = from_sdk_request_id(id)?;
        if self.pending_permissions.contains_key(&request_id)
            || self.pending_unsupported.contains_key(&request_id)
        {
            return Err(AcpV1CodecError::DuplicateInboundRequest(request_id));
        }
        if self.pending_permissions.len() + self.pending_unsupported.len()
            >= MAX_PENDING_CLIENT_REQUESTS
        {
            return Err(AcpV1CodecError::TooManyPendingClientRequests {
                limit: MAX_PENDING_CLIENT_REQUESTS,
            });
        }
        if method != CLIENT_METHOD_NAMES.session_request_permission {
            self.pending_unsupported
                .insert(request_id.clone(), response_destination);
            return Ok(AcpV1Observation::UnsupportedClientRequest {
                request_id,
                method: method.to_owned(),
            });
        }
        if self.prompt_request_id.is_none() {
            return Err(AcpV1CodecError::PromptNotActive);
        }
        if self.cancellation_sent {
            return Err(AcpV1CodecError::CancellationAlreadySent);
        }
        let request: RequestPermissionRequest = decode_params(params)?;
        let session_id = request.session_id.0.to_string();
        self.require_session(&session_id)?;
        if request.options.is_empty() {
            return Err(AcpV1CodecError::EmptyPermissionOptions);
        }
        let mut option_ids = BTreeMap::new();
        let mut options = Vec::with_capacity(request.options.len());
        for option in request.options {
            let option_id = option.option_id.0.to_string();
            let kind = copy_permission_kind(option.kind);
            if option_ids.insert(option_id.clone(), kind).is_some() {
                return Err(AcpV1CodecError::DuplicatePermissionOption(option_id));
            }
            options.push(AcpV1PermissionOption {
                option_id,
                name: option.name,
                kind,
            });
        }
        let tool_call = serde_json::to_value(request.tool_call)?;
        self.pending_permissions.insert(
            request_id.clone(),
            PendingPermission {
                option_ids,
                response_destination,
            },
        );
        Ok(AcpV1Observation::PermissionRequested(
            AcpV1PermissionRequest {
                request_id,
                session_id,
                tool_call,
                options,
            },
        ))
    }

    fn encode_request(
        &mut self,
        request: ClientRequest,
    ) -> Result<(AcpV1RequestId, String), AcpV1CodecError> {
        let method = request.method().to_owned();
        let params = serde_json::to_value(request)?;
        let request_id = self.next_request_id()?;
        let raw = RawJsonRpcMessage::request(method, params, to_sdk_request_id(&request_id))
            .map_err(|error| AcpV1CodecError::Sdk(error.to_string()))?;
        let frame = self.encode_raw(&raw)?;
        Ok((request_id, frame))
    }

    fn encode_notification(
        &self,
        notification: ClientNotification,
    ) -> Result<String, AcpV1CodecError> {
        let method = notification.method().to_owned();
        let params = serde_json::to_value(notification)?;
        let raw = RawJsonRpcMessage::notification(method, params)
            .map_err(|error| AcpV1CodecError::Sdk(error.to_string()))?;
        self.encode_raw(&raw)
    }

    fn permission_outcome_message(
        &self,
        request_id: &AcpV1RequestId,
        outcome: RequestPermissionOutcome,
    ) -> Result<RawJsonRpcMessage, AcpV1CodecError> {
        let response = RequestPermissionResponse::new(outcome);
        let result = serde_json::to_value(response)?;
        Ok(RawJsonRpcMessage::response(
            to_sdk_request_id(request_id),
            Ok(result),
        ))
    }

    fn settle_inbound_response(
        &mut self,
        destination: InboundResponseDestination,
        response: RawJsonRpcMessage,
    ) -> Result<Vec<String>, AcpV1CodecError> {
        match destination {
            InboundResponseDestination::Individual => Ok(vec![self.encode_raw(&response)?]),
            InboundResponseDestination::Batch { batch_id, slot } => {
                let pending = self
                    .pending_inbound_batches
                    .get_mut(&batch_id)
                    .ok_or(AcpV1CodecError::UnknownInboundBatch(batch_id))?;
                let target = pending
                    .responses
                    .get_mut(slot)
                    .ok_or(AcpV1CodecError::UnknownInboundBatchSlot { batch_id, slot })?;
                if target.replace(response).is_some() {
                    return Err(AcpV1CodecError::InboundBatchSlotAlreadySettled { batch_id, slot });
                }
                self.take_completed_batch(batch_id)
            }
        }
    }

    fn take_completed_batch(&mut self, batch_id: u64) -> Result<Vec<String>, AcpV1CodecError> {
        let Some(pending) = self.pending_inbound_batches.get(&batch_id) else {
            return Ok(Vec::new());
        };
        if pending.responses.iter().any(Option::is_none) {
            return Ok(Vec::new());
        }
        let pending = self
            .pending_inbound_batches
            .remove(&batch_id)
            .ok_or(AcpV1CodecError::UnknownInboundBatch(batch_id))?;
        let responses = pending
            .responses
            .into_iter()
            .map(|response| response.ok_or(AcpV1CodecError::UnknownInboundBatch(batch_id)))
            .collect::<Result<Vec<_>, _>>()?;
        let batch = TransportBatch::from_messages(responses)
            .ok_or(AcpV1CodecError::UnknownInboundBatch(batch_id))?;
        let frame = TransportFrame::Batch(batch)
            .to_json()
            .map_err(|error| AcpV1CodecError::Sdk(error.to_string()))?;
        self.validate_encoded_frame(frame).map(|frame| vec![frame])
    }

    fn encode_raw(&self, raw: &RawJsonRpcMessage) -> Result<String, AcpV1CodecError> {
        let frame = serde_json::to_string(raw)?;
        self.validate_encoded_frame(frame)
    }

    fn validate_encoded_frame(&self, frame: String) -> Result<String, AcpV1CodecError> {
        if frame.len() > self.config.max_frame_bytes {
            return Err(AcpV1CodecError::FrameTooLarge {
                limit: self.config.max_frame_bytes,
            });
        }
        Ok(frame)
    }

    fn next_request_id(&mut self) -> Result<AcpV1RequestId, AcpV1CodecError> {
        let sequence = self.next_request_sequence;
        self.next_request_sequence = sequence
            .checked_add(1)
            .ok_or(AcpV1CodecError::RequestIdExhausted)?;
        Ok(AcpV1RequestId::String(format!("cosh-acp-{sequence}")))
    }

    fn require_phase(
        &self,
        expected: AcpV1ProtocolPhase,
        operation: &'static str,
    ) -> Result<(), AcpV1CodecError> {
        if self.phase != expected {
            return Err(self.invalid_phase(operation));
        }
        Ok(())
    }

    fn require_session(&self, actual: &str) -> Result<(), AcpV1CodecError> {
        let expected = self
            .session_id
            .as_deref()
            .ok_or(AcpV1CodecError::SessionNotOpen)?;
        if expected != actual {
            return Err(AcpV1CodecError::SessionMismatch {
                expected: expected.to_owned(),
                actual: actual.to_owned(),
            });
        }
        Ok(())
    }

    fn invalid_phase(&self, operation: &'static str) -> AcpV1CodecError {
        AcpV1CodecError::InvalidPhase {
            operation,
            phase: self.phase,
        }
    }
}

fn validate_absolute(path: &Path) -> Result<(), AcpV1CodecError> {
    if !path.is_absolute() {
        return Err(AcpV1CodecError::WorkspaceNotAbsolute(path.to_path_buf()));
    }
    Ok(())
}

fn batch_entry_requires_response(entry: &TransportBatchEntry) -> bool {
    match entry {
        TransportBatchEntry::Message(RawJsonRpcMessage::Request(_)) => true,
        TransportBatchEntry::Message(
            RawJsonRpcMessage::Notification(_) | RawJsonRpcMessage::Response(_),
        ) => false,
        TransportBatchEntry::Malformed { raw, .. } => !is_response_only_shape(raw),
    }
}

fn is_response_only_shape(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|object| {
        !object.contains_key("method")
            && (object.contains_key("result") || object.contains_key("error"))
    })
}

fn decode_params<T: serde::de::DeserializeOwned>(
    params: Option<RawJsonRpcParams>,
) -> Result<T, AcpV1CodecError> {
    let value = params.map_or(serde_json::Value::Null, RawJsonRpcParams::into_value);
    serde_json::from_value(value).map_err(Into::into)
}

fn from_sdk_request_id(id: RequestId) -> Result<AcpV1RequestId, AcpV1CodecError> {
    match id {
        RequestId::Null => Err(AcpV1CodecError::NullRequestId),
        RequestId::Number(value) => Ok(AcpV1RequestId::Number(value)),
        RequestId::Str(value) => Ok(AcpV1RequestId::String(value)),
    }
}

fn to_sdk_request_id(id: &AcpV1RequestId) -> RequestId {
    match id {
        AcpV1RequestId::Number(value) => RequestId::Number(*value),
        AcpV1RequestId::String(value) => RequestId::Str(value.clone()),
    }
}

fn copy_capabilities(capabilities: &AgentCapabilities) -> AcpV1AgentCapabilities {
    AcpV1AgentCapabilities {
        load_session: capabilities.load_session,
        list_sessions: capabilities.session_capabilities.list.is_some(),
        delete_session: capabilities.session_capabilities.delete.is_some(),
        additional_directories: capabilities
            .session_capabilities
            .additional_directories
            .is_some(),
        resume_session: capabilities.session_capabilities.resume.is_some(),
        close_session: capabilities.session_capabilities.close.is_some(),
        image_prompts: capabilities.prompt_capabilities.image,
        audio_prompts: capabilities.prompt_capabilities.audio,
        embedded_context: capabilities.prompt_capabilities.embedded_context,
    }
}

fn copy_stop_reason(reason: StopReason) -> AcpV1StopReason {
    match reason {
        StopReason::EndTurn => AcpV1StopReason::EndTurn,
        StopReason::MaxTokens => AcpV1StopReason::MaxTokens,
        StopReason::MaxTurnRequests => AcpV1StopReason::MaxTurnRequests,
        StopReason::Refusal => AcpV1StopReason::Refusal,
        StopReason::Cancelled => AcpV1StopReason::Cancelled,
        _ => AcpV1StopReason::Unsupported,
    }
}

fn copy_permission_kind(kind: PermissionOptionKind) -> AcpV1PermissionOptionKind {
    match kind {
        PermissionOptionKind::AllowOnce => AcpV1PermissionOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways => AcpV1PermissionOptionKind::AllowAlways,
        PermissionOptionKind::RejectOnce => AcpV1PermissionOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways => AcpV1PermissionOptionKind::RejectAlways,
        _ => AcpV1PermissionOptionKind::Unsupported,
    }
}

const _: () = assert!(ProtocolVersion::V1.as_u16() == ACP_WIRE_PROTOCOL_VERSION);
