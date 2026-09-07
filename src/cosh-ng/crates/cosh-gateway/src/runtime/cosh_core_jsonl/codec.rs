//! Stateful encoder and decoder for private cosh-core JSONL frames.

use cosh_gateway_contracts::profile::{GatewayCapabilityProfile, GatewayCapabilityProfileIdentity};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::*;

/// Stateful encoder/decoder for one private cosh-core process lifecycle.
#[derive(Debug)]
pub struct CoshCoreJsonlCodec {
    initialize_request_id: String,
    max_frame_bytes: usize,
    phase: CoshCoreProtocolPhase,
    profile: CoshCoreExecutionProfile,
    capability_profile: Option<GatewayCapabilityProfileIdentity>,
}

impl CoshCoreJsonlCodec {
    /// Creates a codec for one correlated initialization exchange.
    ///
    /// # Errors
    ///
    /// Returns `InvalidLimit` when `max_frame_bytes` is zero.
    pub fn new(
        initialize_request_id: impl Into<String>,
        max_frame_bytes: usize,
    ) -> Result<Self, CoshCoreCodecError> {
        if max_frame_bytes == 0 {
            return Err(CoshCoreCodecError::InvalidLimit);
        }
        Ok(Self {
            initialize_request_id: initialize_request_id.into(),
            max_frame_bytes,
            phase: CoshCoreProtocolPhase::Created,
            profile: CoshCoreExecutionProfile::Legacy,
            capability_profile: None,
        })
    }

    /// Creates a codec that requires the exact brokered v3 acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns `InvalidLimit` when `max_frame_bytes` is zero.
    pub fn new_gateway_brokered(
        initialize_request_id: impl Into<String>,
        max_frame_bytes: usize,
    ) -> Result<Self, CoshCoreCodecError> {
        let mut codec = Self::new(initialize_request_id, max_frame_bytes)?;
        codec.profile = CoshCoreExecutionProfile::GatewayBrokeredV1;
        codec.capability_profile = Some(GatewayCapabilityProfile::task_only_v1().identity());
        Ok(codec)
    }

    /// Returns the current private protocol phase.
    pub fn phase(&self) -> CoshCoreProtocolPhase {
        self.phase
    }

    /// Encodes the mandatory exact-version initialize request.
    ///
    /// # Errors
    ///
    /// Returns an invalid-phase or frame-bound error.
    pub fn initialize_frame(
        &mut self,
        fire_session_start: bool,
    ) -> Result<String, CoshCoreCodecError> {
        if self.phase != CoshCoreProtocolPhase::Created {
            return Err(self.invalid_phase("initialize_frame"));
        }
        let frame = encode_frame(
            &InitializeInput {
                message_type: "control_request",
                request_id: &self.initialize_request_id,
                request: InitializeRequest {
                    subtype: "initialize",
                    fire_session_start,
                    protocol_version: self.profile.protocol_version(),
                    execution_profile: self.profile.wire_name(),
                    capability_profile: self.capability_profile.as_ref(),
                },
            },
            self.max_frame_bytes,
        )?;
        self.phase = CoshCoreProtocolPhase::AwaitingInitialize;
        Ok(frame)
    }

    /// Encodes one typed user turn after successful negotiation.
    ///
    /// # Errors
    ///
    /// Returns an invalid-phase, serialization, or frame-bound error.
    pub fn user_frame(&self, turn: &CoshCoreUserTurn) -> Result<String, CoshCoreCodecError> {
        if self.phase != CoshCoreProtocolPhase::Ready {
            return Err(self.invalid_phase("user_frame"));
        }
        encode_frame(
            &UserInput {
                message_type: "user",
                message: UserInputBody {
                    role: "user",
                    content: &turn.content,
                    raw_user_input: turn.raw_user_input.as_deref(),
                },
                session_id: turn.provider_session_id.as_deref(),
                shell_context: turn.shell_context.as_ref(),
            },
            self.max_frame_bytes,
        )
    }

    /// Encodes a correlated interrupt request during initialization or a turn.
    ///
    /// # Errors
    ///
    /// Returns an invalid-phase, serialization, or frame-bound error.
    pub fn interrupt_frame(&self, request_id: &str) -> Result<String, CoshCoreCodecError> {
        self.control_frame(request_id, "interrupt", "interrupt_frame")
    }

    /// Encodes a correlated graceful shutdown request.
    ///
    /// # Errors
    ///
    /// Returns an invalid-phase, serialization, or frame-bound error.
    pub fn shutdown_frame(&self, request_id: &str) -> Result<String, CoshCoreCodecError> {
        self.control_frame(request_id, "shutdown", "shutdown_frame")
    }

    /// Encodes durable takeover of a pending brokered callback.
    ///
    /// # Errors
    ///
    /// Returns an invalid-phase, profile, serialization, or frame-bound error.
    pub fn brokered_acknowledgement_frame(
        &self,
        request_id: &str,
    ) -> Result<String, CoshCoreCodecError> {
        self.require_brokered("brokered_acknowledgement_frame")?;
        if self.phase != CoshCoreProtocolPhase::Ready {
            return Err(self.invalid_phase("brokered_acknowledgement_frame"));
        }
        encode_frame(
            &ApprovalReceiptInput {
                message_type: "approval_receipt",
                request_id,
            },
            self.max_frame_bytes,
        )
    }

    /// Encodes a bounded terminal rejection for a brokered operation.
    ///
    /// # Errors
    ///
    /// Returns an invalid-phase, profile, serialization, or frame-bound error.
    pub fn brokered_denial_frame(
        &self,
        request_id: &str,
        safe_message: &str,
    ) -> Result<String, CoshCoreCodecError> {
        self.require_brokered("brokered_denial_frame")?;
        if self.phase != CoshCoreProtocolPhase::Ready {
            return Err(self.invalid_phase("brokered_denial_frame"));
        }
        encode_frame(
            &BrokeredControlResponseInput {
                message_type: "control_response",
                response: BrokeredControlResponse {
                    subtype: "success",
                    request_id,
                    response: BrokeredControlResponseBody::Deny {
                        behavior: "deny",
                        message: safe_message,
                    },
                },
            },
            self.max_frame_bytes,
        )
    }

    /// Encodes one bounded answer for an exact brokered `ask_user` request.
    ///
    /// # Errors
    ///
    /// Returns an invalid-phase, profile, serialization, or frame-bound error.
    pub fn brokered_input_response_frame(
        &self,
        request_id: &str,
        answer: &str,
    ) -> Result<String, CoshCoreCodecError> {
        self.require_brokered("brokered_input_response_frame")?;
        if self.phase != CoshCoreProtocolPhase::Ready {
            return Err(self.invalid_phase("brokered_input_response_frame"));
        }
        encode_frame(
            &BrokeredControlResponseInput {
                message_type: "control_response",
                response: BrokeredControlResponse {
                    subtype: "success",
                    request_id,
                    response: BrokeredControlResponseBody::Answer {
                        behavior: "answer",
                        answer,
                    },
                },
            },
            self.max_frame_bytes,
        )
    }

    fn require_brokered(&self, operation: &'static str) -> Result<(), CoshCoreCodecError> {
        if self.profile == CoshCoreExecutionProfile::GatewayBrokeredV1 {
            Ok(())
        } else {
            Err(CoshCoreCodecError::ProfileMismatch { operation })
        }
    }

    fn control_frame(
        &self,
        request_id: &str,
        subtype: &'static str,
        operation: &'static str,
    ) -> Result<String, CoshCoreCodecError> {
        if !matches!(
            self.phase,
            CoshCoreProtocolPhase::AwaitingInitialize | CoshCoreProtocolPhase::Ready
        ) {
            return Err(self.invalid_phase(operation));
        }
        encode_frame(
            &SimpleControlInput {
                message_type: "control_request",
                request_id,
                request: SimpleControlRequest { subtype },
            },
            self.max_frame_bytes,
        )
    }

    /// Decodes and validates one private cosh-core output frame.
    ///
    /// # Errors
    ///
    /// Rejects malformed/oversized frames, negotiation violations, unknown
    /// message types, and any output after the first terminal result.
    pub fn decode_frame(
        &mut self,
        frame: &[u8],
    ) -> Result<CoshCoreObservation, CoshCoreCodecError> {
        if self.phase == CoshCoreProtocolPhase::Terminal {
            return Err(CoshCoreCodecError::OutputAfterTerminal);
        }
        if frame.len() > self.max_frame_bytes {
            return Err(CoshCoreCodecError::FrameTooLarge {
                limit: self.max_frame_bytes,
            });
        }
        let frame = std::str::from_utf8(frame).map_err(|_| CoshCoreCodecError::InvalidUtf8)?;
        let frame = frame.trim_end_matches(['\r', '\n']);
        if frame.is_empty() {
            return Err(CoshCoreCodecError::EmptyFrame);
        }

        let value: Value = serde_json::from_str(frame)?;
        let message_type = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| CoshCoreCodecError::UnknownMessageType(String::new()))?
            .to_string();

        if self.phase == CoshCoreProtocolPhase::Created {
            return Err(self.invalid_phase("decode_frame"));
        }
        if self.phase == CoshCoreProtocolPhase::AwaitingInitialize {
            return self.decode_initializing(&message_type, value);
        }
        self.decode_ready(&message_type, value)
    }

    /// Produces one synthetic terminal when stdout ends before a core result.
    pub fn finish_stdout(&mut self) -> Option<CoshCoreObservation> {
        if self.phase == CoshCoreProtocolPhase::Terminal {
            return None;
        }
        self.phase = CoshCoreProtocolPhase::Terminal;
        Some(CoshCoreObservation::ProtocolEndedWithoutResult)
    }

    fn decode_initializing(
        &mut self,
        message_type: &str,
        value: Value,
    ) -> Result<CoshCoreObservation, CoshCoreCodecError> {
        match message_type {
            "control_response" => self.decode_initialize_response(value),
            "control_request" => {
                let request = decode_control_request(value)?;
                if matches!(request.request, CoshCoreControlRequest::AuthRequired { .. }) {
                    Ok(CoshCoreObservation::ControlRequest(request))
                } else {
                    Err(CoshCoreCodecError::UnexpectedBeforeInitialization(
                        "control_request".to_string(),
                    ))
                }
            }
            other => Err(CoshCoreCodecError::UnexpectedBeforeInitialization(
                other.to_string(),
            )),
        }
    }

    fn decode_initialize_response(
        &mut self,
        value: Value,
    ) -> Result<CoshCoreObservation, CoshCoreCodecError> {
        let envelope: WireControlResponseEnvelope = serde_json::from_value(value)?;
        if envelope.response.request_id != self.initialize_request_id
            || envelope.response.response.subtype != "initialize"
        {
            return Err(CoshCoreCodecError::InitializeCorrelationMismatch);
        }
        if envelope.response.subtype != "success" {
            return Err(CoshCoreCodecError::InitializeRejected(
                envelope
                    .response
                    .response
                    .error
                    .unwrap_or_else(|| envelope.response.subtype.clone()),
            ));
        }
        let version = envelope
            .response
            .response
            .protocol_version
            .ok_or(CoshCoreCodecError::InitializeVersionMissing)?;
        let required_version = self.profile.protocol_version();
        if version != required_version {
            return Err(CoshCoreCodecError::InitializeVersionMismatch {
                required: required_version,
                actual: version,
            });
        }
        let acknowledged_profile = envelope.response.response.execution_profile.as_deref();
        if acknowledged_profile != self.profile.wire_name() {
            return Err(CoshCoreCodecError::InitializeProfileMismatch);
        }
        if envelope.response.response.capability_profile != self.capability_profile {
            return Err(CoshCoreCodecError::InitializeProfileMismatch);
        }
        if self.profile == CoshCoreExecutionProfile::GatewayBrokeredV1 {
            let runtime_tools = envelope
                .response
                .response
                .runtime_tools
                .as_ref()
                .ok_or(CoshCoreCodecError::InitializeCapabilitiesMissing)?;
            let runtime_tools = runtime_tools.iter().map(String::as_str).collect::<Vec<_>>();
            GatewayCapabilityProfile::task_only_v1()
                .verify_runtime_tools(&runtime_tools)
                .map_err(|_| CoshCoreCodecError::InitializeCapabilitiesInvalid)?;
        }
        let capabilities = envelope
            .response
            .response
            .capabilities
            .ok_or(CoshCoreCodecError::InitializeCapabilitiesMissing)?;
        if self.profile == CoshCoreExecutionProfile::GatewayBrokeredV1
            && (!capabilities.can_handle_can_use_tool
                || capabilities.can_handle_host_executed_shell_tool_result
                || capabilities.can_handle_shell_evidence_tool
                || !capabilities.can_handle_approval_receipt
                || capabilities.can_handle_hosted_checkpoint_create
                || !capabilities.can_handle_brokered_ask_user)
        {
            return Err(CoshCoreCodecError::InitializeCapabilitiesInvalid);
        }
        self.phase = CoshCoreProtocolPhase::Ready;
        Ok(CoshCoreObservation::Initialized(capabilities))
    }

    fn decode_ready(
        &mut self,
        message_type: &str,
        value: Value,
    ) -> Result<CoshCoreObservation, CoshCoreCodecError> {
        match message_type {
            "system" => serde_json::from_value(value)
                .map(CoshCoreObservation::System)
                .map_err(Into::into),
            "stream_event" => {
                let message: WireStreamEnvelope = serde_json::from_value(value)?;
                Ok(CoshCoreObservation::Stream(message.event))
            }
            "assistant" => serde_json::from_value(value)
                .map(CoshCoreObservation::Assistant)
                .map_err(Into::into),
            "user" => {
                let message: WireUserOutput = serde_json::from_value(value)?;
                Ok(CoshCoreObservation::ToolResults {
                    provider_session_id: message.provider_session_id,
                    results: message
                        .message
                        .content
                        .into_iter()
                        .map(|content| match content {
                            WireUserContent::ToolResult(result) => result,
                        })
                        .collect(),
                })
            }
            "control_request" => {
                decode_control_request(value).map(CoshCoreObservation::ControlRequest)
            }
            "control_response" => {
                let message: WireGenericControlResponseEnvelope = serde_json::from_value(value)?;
                if message.response.request_id == self.initialize_request_id
                    || message
                        .response
                        .response
                        .get("subtype")
                        .and_then(Value::as_str)
                        == Some("initialize")
                {
                    return Err(CoshCoreCodecError::DuplicateInitializeResponse);
                }
                Ok(CoshCoreObservation::ControlResponse(
                    CoshCoreControlResponse {
                        request_id: message.response.request_id,
                        subtype: message.response.subtype,
                        body: message.response.response,
                    },
                ))
            }
            "registry_response" => {
                let message: WireRegistryResponse = serde_json::from_value(value)?;
                Ok(CoshCoreObservation::RegistryResponse {
                    request_id: message.request_id,
                    success: message.success,
                    data: message.data,
                    error: message.error,
                })
            }
            "result" => {
                let result = serde_json::from_value(value)?;
                self.phase = CoshCoreProtocolPhase::Terminal;
                Ok(CoshCoreObservation::Result(result))
            }
            other => Err(CoshCoreCodecError::UnknownMessageType(other.to_string())),
        }
    }

    fn invalid_phase(&self, operation: &'static str) -> CoshCoreCodecError {
        CoshCoreCodecError::InvalidPhase {
            operation,
            phase: self.phase,
        }
    }
}

fn decode_control_request(
    value: Value,
) -> Result<CoshCoreControlRequestEnvelope, CoshCoreCodecError> {
    let message: WireControlRequestEnvelope = serde_json::from_value(value)?;
    Ok(CoshCoreControlRequestEnvelope {
        request_id: message.request_id,
        request: message.request,
    })
}

fn encode_frame<T: Serialize>(
    value: &T,
    max_frame_bytes: usize,
) -> Result<String, CoshCoreCodecError> {
    let mut frame = serde_json::to_string(value)?;
    if frame.len() > max_frame_bytes {
        return Err(CoshCoreCodecError::FrameTooLarge {
            limit: max_frame_bytes,
        });
    }
    frame.push('\n');
    Ok(frame)
}

#[derive(Serialize)]
struct InitializeInput<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    request_id: &'a str,
    request: InitializeRequest<'a>,
}

#[derive(Serialize)]
struct InitializeRequest<'a> {
    subtype: &'static str,
    fire_session_start: bool,
    protocol_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_profile: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capability_profile: Option<&'a GatewayCapabilityProfileIdentity>,
}

#[derive(Serialize)]
struct ApprovalReceiptInput<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    request_id: &'a str,
}

#[derive(Serialize)]
struct BrokeredControlResponseInput<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    response: BrokeredControlResponse<'a>,
}

#[derive(Serialize)]
struct BrokeredControlResponse<'a> {
    subtype: &'static str,
    request_id: &'a str,
    response: BrokeredControlResponseBody<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum BrokeredControlResponseBody<'a> {
    Deny {
        behavior: &'static str,
        message: &'a str,
    },
    Answer {
        behavior: &'static str,
        answer: &'a str,
    },
}

#[derive(Serialize)]
struct SimpleControlInput<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    request_id: &'a str,
    request: SimpleControlRequest,
}

#[derive(Serialize)]
struct SimpleControlRequest {
    subtype: &'static str,
}

#[derive(Serialize)]
struct UserInput<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    message: UserInputBody<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shell_context: Option<&'a CoshCoreShellContext>,
}

#[derive(Serialize)]
struct UserInputBody<'a> {
    role: &'static str,
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_user_input: Option<&'a str>,
}

#[derive(Deserialize)]
struct WireControlResponseEnvelope {
    response: WireInitializeResponse,
}

#[derive(Deserialize)]
struct WireInitializeResponse {
    subtype: String,
    request_id: String,
    response: WireInitializeBody,
}

#[derive(Deserialize)]
struct WireInitializeBody {
    subtype: String,
    #[serde(default)]
    protocol_version: Option<u32>,
    #[serde(default)]
    execution_profile: Option<String>,
    #[serde(default)]
    capability_profile: Option<GatewayCapabilityProfileIdentity>,
    #[serde(default)]
    runtime_tools: Option<Vec<String>>,
    #[serde(default)]
    capabilities: Option<CoshCoreCapabilities>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct WireStreamEnvelope {
    event: CoshCoreStreamEvent,
}

#[derive(Deserialize)]
struct WireUserOutput {
    #[serde(rename = "session_id")]
    provider_session_id: String,
    message: WireUserBody,
}

#[derive(Deserialize)]
struct WireUserBody {
    content: Vec<WireUserContent>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WireUserContent {
    #[serde(rename = "tool_result")]
    ToolResult(CoshCoreToolResult),
}

#[derive(Deserialize)]
struct WireControlRequestEnvelope {
    request_id: String,
    request: CoshCoreControlRequest,
}

#[derive(Deserialize)]
struct WireGenericControlResponseEnvelope {
    response: WireGenericControlResponse,
}

#[derive(Deserialize)]
struct WireGenericControlResponse {
    subtype: String,
    request_id: String,
    response: Value,
}

#[derive(Deserialize)]
struct WireRegistryResponse {
    request_id: String,
    success: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}
