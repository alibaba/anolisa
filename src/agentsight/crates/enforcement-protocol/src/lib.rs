//! Versioned wire contract between AgentSight and its privileged enforcer.

use std::io::{self, BufRead, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub mod security;
pub use security::*;

/// Wire protocol version implemented by this crate.
pub const PROTOCOL_VERSION: u16 = 3;

/// Maximum JSON payload size accepted for one NDJSON frame.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// One request sent from AgentSight to the enforcer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Protocol version used to encode the request.
    pub protocol_version: u16,
    /// Correlation identifier copied into the response.
    pub request_id: Uuid,
    /// Operation requested from the enforcer.
    pub command: Command,
}

impl Request {
    /// Builds a request using the current protocol version and a fresh identifier.
    pub fn new(command: Command) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: Uuid::new_v4(),
            command,
        }
    }
}

/// Operations supported by protocol version 3.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", content = "params", rename_all = "snake_case")]
pub enum Command {
    /// Reports backend readiness.
    Health,
    /// Compiles and attaches one policy binding.
    ApplyPolicy(ApplyPolicy),
    /// Applies a policy only while the named required evidence stream is live.
    ApplyPolicyLeased {
        /// Desired policy binding.
        request: ApplyPolicy,
        /// Required violation subscription proving evidence delivery is live.
        required_subscription_id: Uuid,
    },
    /// Compiles a product-level credential policy inside the privileged adapter.
    ApplyCredentialPolicy(ApplyCredentialPolicy),
    /// Compiles and applies a credential policy only while evidence delivery is live.
    ApplyCredentialPolicyLeased {
        /// Product-level credential policy compiled by the privileged adapter.
        request: ApplyCredentialPolicy,
        /// Required violation subscription proving evidence delivery is live.
        required_subscription_id: Uuid,
    },
    /// Detaches a binding by its stable identifier.
    DetachAgent {
        /// Binding to detach.
        binding_id: Uuid,
    },
    /// Lists known policy bindings.
    ListBindings,
    /// Keeps a best-effort observer connection open for violation responses.
    SubscribeViolations {
        /// Identity used to prune this observer during connection cleanup.
        subscription_id: Uuid,
    },
    /// Registers the one required evidence stream used to authorize applies.
    SubscribeRequiredViolations {
        /// Fresh generation identity for this required stream.
        subscription_id: Uuid,
    },
    /// Keeps the connection open and streams normalized security events.
    SubscribeSecurityEvents,
}

/// Product-level credential policy binding sent across the privilege boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyCredentialPolicy {
    /// Stable idempotency key for this binding.
    pub binding_id: Uuid,
    /// Product-level Agent identity.
    pub agent_id: String,
    /// Optional AgentSight session identity.
    pub session_id: Option<String>,
    /// Root PID whose process tree receives the policy.
    pub root_pid: i32,
    /// Linux process start time used to reject PID reuse.
    pub process_start_time: u64,
    /// ActPlane-independent taint and destination policy.
    pub policy: CredentialExfiltrationPolicy,
}

/// Desired policy binding for one Agent process tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyPolicy {
    /// Stable idempotency key for this binding.
    pub binding_id: Uuid,
    /// Product-level Agent identity.
    pub agent_id: String,
    /// Optional AgentSight session identity.
    pub session_id: Option<String>,
    /// Root PID whose process tree receives the policy.
    pub root_pid: i32,
    /// Linux `/proc/<pid>/stat` start time used to reject PID reuse.
    pub process_start_time: u64,
    /// Product policy identifier.
    pub policy_id: String,
    /// Immutable product policy revision.
    pub policy_revision: String,
    /// ActPlane DSL compiled only by the privileged adapter.
    pub policy_dsl: String,
}

/// Lifecycle state acknowledged for a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingState {
    /// Desired state is persisted but not acknowledged by the enforcer.
    Pending,
    /// The enforcer acknowledged attachment.
    Enforced,
    /// Compilation or attachment failed.
    Failed,
    /// A previously active binding lost runtime assurance.
    Degraded,
    /// Detachment has been requested but not acknowledged.
    Detaching,
    /// The enforcer acknowledged detachment.
    Detached,
}

/// Enforcer acknowledgement for one desired binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// Original desired policy request.
    pub request: ApplyPolicy,
    /// Last acknowledged lifecycle state.
    pub state: BindingState,
    /// Sanitized status detail suitable for APIs and logs.
    pub message: Option<String>,
    /// ActPlane domain assigned by the backend when attached.
    pub domain_id: Option<u32>,
}

/// Kernel action requested by the matching rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Records the match without changing the operation.
    Notify,
    /// Rejects the operation in the kernel.
    Block,
    /// Terminates the protected process.
    Kill,
}

/// Explicit operations supported by the selected enforcement backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcementCapabilities {
    /// Whether credential policies may run without changing operations.
    pub credential_observe: bool,
    /// Whether credential policies may record matching decisions.
    pub credential_audit: bool,
    /// Whether credential policies may block matching operations.
    pub credential_enforce: bool,
    /// Whether a policy may be atomically handed off to another policy.
    pub policy_handoff: bool,
    /// Whether containment may retarget a different process identity.
    pub alternate_pid_retarget: bool,
    /// Whether this backend is an explicit test or development implementation.
    pub test_development: bool,
}

impl EnforcementCapabilities {
    /// Returns a safe fallback for peers that did not send capability metadata.
    pub const fn unsupported() -> Self {
        Self {
            credential_observe: false,
            credential_audit: false,
            credential_enforce: false,
            policy_handoff: false,
            alternate_pid_retarget: false,
            test_development: false,
        }
    }

    /// Returns the capabilities implemented by the production ActPlane adapter.
    pub const fn actplane() -> Self {
        Self {
            credential_observe: true,
            credential_audit: true,
            credential_enforce: false,
            policy_handoff: false,
            alternate_pid_retarget: false,
            test_development: false,
        }
    }

    /// Returns the capabilities implemented only by the mock test backend.
    pub const fn mock_development() -> Self {
        Self {
            credential_observe: true,
            credential_audit: true,
            credential_enforce: true,
            policy_handoff: false,
            alternate_pid_retarget: true,
            test_development: true,
        }
    }
}

impl Default for EnforcementCapabilities {
    fn default() -> Self {
        Self::unsupported()
    }
}

/// Runtime health returned by the selected enforcement backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Whether the backend can accept policy operations.
    pub ready: bool,
    /// Stable backend name such as `mock` or `actplane`.
    pub backend: String,
    /// Operations this backend explicitly implements.
    #[serde(default)]
    pub capabilities: EnforcementCapabilities,
    /// Optional actionable readiness detail.
    pub message: Option<String>,
}

/// Stable violation fact published by the privileged enforcer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViolationEvent {
    /// Stable event identifier used for idempotent ingestion.
    pub event_id: Uuid,
    /// Binding that produced the event.
    pub binding_id: Uuid,
    /// Product-level Agent identity.
    pub agent_id: String,
    /// Optional AgentSight session identity.
    pub session_id: Option<String>,
    /// Product policy identifier.
    pub policy_id: String,
    /// Immutable product policy revision.
    pub policy_revision: String,
    /// PID observed by the enforcement backend.
    pub pid: i32,
    /// Parent PID when supplied by the backend.
    pub ppid: Option<i32>,
    /// Process start time used to disambiguate PID reuse.
    pub process_start_time: u64,
    /// Normalized kernel operation such as `open` or `connect`.
    pub operation: String,
    /// Redacted operation target.
    pub target: String,
    /// Rule effect requested by the compiled policy.
    pub effect: Effect,
    /// Whether the kernel actually rejected the operation.
    pub blocked: bool,
    /// Whether the kernel actually terminated the process.
    pub killed: bool,
    /// Upstream rule identifier when available.
    pub rule_id: Option<String>,
    /// Sanitized upstream reason when available.
    pub reason: Option<String>,
    /// Event occurrence time as Unix epoch nanoseconds.
    pub occurred_at_ns: u64,
    /// Time the enforcer received and normalized the event, as Unix epoch nanoseconds.
    pub observed_at_ns: u64,
    /// Exact upstream ActPlane revision that produced the event.
    pub actplane_revision: String,
}

/// One response correlated to a request ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// Protocol version used to encode the response.
    pub protocol_version: u16,
    /// Identifier copied from the originating request.
    pub request_id: Uuid,
    /// Successful payload or typed remote failure.
    pub result: Result<ResponseBody, RemoteError>,
}

/// Successful response payloads supported by protocol version 3.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "response", content = "data", rename_all = "snake_case")]
pub enum ResponseBody {
    /// Backend readiness information.
    Health(HealthStatus),
    /// Binding returned after apply.
    Applied(Binding),
    /// Successful detach acknowledgement.
    Detached,
    /// Current backend bindings.
    Bindings(Vec<Binding>),
    /// Subscription acknowledgement before event frames.
    Subscribed,
    /// One violation on a subscription connection.
    Violation(ViolationEvent),
    /// One normalized security event on a subscription connection.
    SecurityEvent(SecurityEvent),
}

/// Sanitized operation failure returned across the trust boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteError {
    /// Stable machine-readable error code.
    pub code: String,
    /// Sanitized operator-facing error detail.
    pub message: String,
}

/// Frames expose their protocol version for validation after deserialization.
pub trait VersionedFrame {
    /// Returns the version encoded in this frame.
    fn protocol_version(&self) -> u16;
}

impl VersionedFrame for Request {
    fn protocol_version(&self) -> u16 {
        self.protocol_version
    }
}

impl VersionedFrame for Response {
    fn protocol_version(&self) -> u16 {
        self.protocol_version
    }
}

/// Codec failures detected before a request reaches the enforcer backend.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Reading or writing the underlying stream failed.
    #[error("protocol I/O failed: {0}")]
    Io(#[from] io::Error),
    /// JSON serialization or deserialization failed.
    #[error("protocol JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// A frame exceeded the bounded payload size.
    #[error("frame is {actual} bytes; maximum is {max} bytes")]
    FrameTooLarge {
        /// Configured maximum payload size.
        max: usize,
        /// Bytes observed before rejecting the frame.
        actual: usize,
    },
    /// NDJSON frames must end in a newline.
    #[error("frame is missing its newline terminator")]
    MissingNewline,
    /// The peer encoded a protocol version this crate cannot interpret.
    #[error("unsupported protocol version {actual}; expected {expected}")]
    UnsupportedVersion {
        /// Version implemented by this crate.
        expected: u16,
        /// Version supplied by the peer.
        actual: u16,
    },
}

/// Stateful bounded NDJSON decoder for streams that may time out mid-frame.
pub struct FrameReader<R> {
    reader: R,
    bytes: Vec<u8>,
}

impl<R> FrameReader<R> {
    /// Wraps a stream while retaining partial frame bytes between reads.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            bytes: Vec::new(),
        }
    }

    /// Returns the wrapped stream and discards any incomplete frame.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

impl<R: Read> FrameReader<R> {
    /// Reads and validates one bounded NDJSON frame.
    ///
    /// An I/O timeout leaves already-read bytes buffered so the next call can
    /// finish the same frame instead of interpreting its suffix as new JSON.
    ///
    /// # Errors
    ///
    /// Returns a typed error for I/O, malformed JSON, missing termination, an
    /// oversized frame, or a protocol-version mismatch.
    pub fn read_frame<T>(&mut self) -> Result<Option<T>, ProtocolError>
    where
        T: DeserializeOwned + VersionedFrame,
    {
        loop {
            if let Some(newline) = self.bytes.iter().position(|byte| *byte == b'\n') {
                if newline > MAX_FRAME_BYTES {
                    return Err(ProtocolError::FrameTooLarge {
                        max: MAX_FRAME_BYTES,
                        actual: newline,
                    });
                }
                let mut encoded = self.bytes.drain(..=newline).collect::<Vec<_>>();
                encoded.pop();
                return decode_frame(&encoded).map(Some);
            }
            if self.bytes.len() > MAX_FRAME_BYTES {
                return Err(ProtocolError::FrameTooLarge {
                    max: MAX_FRAME_BYTES,
                    actual: self.bytes.len(),
                });
            }

            let remaining = MAX_FRAME_BYTES + 1 - self.bytes.len();
            let mut chunk = [0_u8; 8 * 1024];
            let read_limit = remaining.min(chunk.len());
            let read = self.reader.read(&mut chunk[..read_limit])?;
            if read == 0 {
                return if self.bytes.is_empty() {
                    Ok(None)
                } else {
                    Err(ProtocolError::MissingNewline)
                };
            }
            self.bytes.extend_from_slice(&chunk[..read]);
        }
    }
}

fn decode_frame<T>(bytes: &[u8]) -> Result<T, ProtocolError>
where
    T: DeserializeOwned + VersionedFrame,
{
    let frame: T = serde_json::from_slice(bytes)?;
    let actual = frame.protocol_version();
    if actual != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion {
            expected: PROTOCOL_VERSION,
            actual,
        });
    }
    Ok(frame)
}

/// Reads and validates one bounded NDJSON frame.
///
/// # Errors
///
/// Returns a typed error for I/O, malformed JSON, missing termination, an
/// oversized frame, or a protocol-version mismatch.
pub fn read_frame<R, T>(reader: &mut R) -> Result<Option<T>, ProtocolError>
where
    R: BufRead,
    T: DeserializeOwned + VersionedFrame,
{
    let mut bytes = Vec::new();
    let mut bounded = Read::take(reader, (MAX_FRAME_BYTES + 1) as u64);
    let bytes_read = bounded.read_until(b'\n', &mut bytes)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    if bytes.last() != Some(&b'\n') {
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge {
                max: MAX_FRAME_BYTES,
                actual: bytes.len(),
            });
        }
        return Err(ProtocolError::MissingNewline);
    }
    bytes.pop();
    decode_frame(&bytes).map(Some)
}

/// Serializes and flushes one bounded NDJSON frame.
///
/// # Errors
///
/// Returns a typed error when JSON encoding or stream output fails, or when the
/// encoded payload exceeds [`MAX_FRAME_BYTES`].
pub fn write_frame<W, T>(writer: &mut W, frame: &T) -> Result<(), ProtocolError>
where
    W: Write,
    T: Serialize,
{
    let bytes = serde_json::to_vec(frame)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            max: MAX_FRAME_BYTES,
            actual: bytes.len(),
        });
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::{BufReader, Cursor, Read};

    use super::*;

    struct InterruptingReader {
        steps: VecDeque<io::Result<Vec<u8>>>,
    }

    impl Read for InterruptingReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            match self.steps.pop_front() {
                Some(Ok(bytes)) => {
                    assert!(bytes.len() <= output.len());
                    output[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
                Some(Err(error)) => Err(error),
                None => Ok(0),
            }
        }
    }

    #[test]
    fn stateful_reader_preserves_a_partial_frame_across_timeout() {
        let request = Request::new(Command::Health);
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &request).expect("fixture should encode");
        let split = encoded.len() / 2;
        let reader = InterruptingReader {
            steps: VecDeque::from([
                Ok(encoded[..split].to_vec()),
                Err(io::Error::new(io::ErrorKind::TimedOut, "fixture timeout")),
                Ok(encoded[split..].to_vec()),
            ]),
        };
        let mut reader = FrameReader::new(reader);

        let first = reader
            .read_frame::<Request>()
            .expect_err("timeout should remain observable");
        assert!(
            matches!(first, ProtocolError::Io(error) if error.kind() == io::ErrorKind::TimedOut)
        );
        assert_eq!(
            reader
                .read_frame::<Request>()
                .expect("the suffix should complete the retained prefix"),
            Some(request)
        );
    }

    #[test]
    fn one_shot_reader_leaves_the_next_buffered_frame_intact() {
        let first = Request::new(Command::Health);
        let second = Request::new(Command::ListBindings);
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &first).expect("first fixture should encode");
        write_frame(&mut encoded, &second).expect("second fixture should encode");
        let mut reader = BufReader::new(Cursor::new(encoded));

        assert_eq!(read_frame::<_, Request>(&mut reader).unwrap(), Some(first));
        assert_eq!(read_frame::<_, Request>(&mut reader).unwrap(), Some(second));
    }

    #[test]
    fn request_round_trips_as_one_frame() {
        let request = Request::new(Command::Health);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).expect("fixture should encode");
        let decoded: Request = read_frame(&mut BufReader::new(Cursor::new(bytes)))
            .expect("fixture should decode")
            .expect("frame should exist");
        assert_eq!(decoded, request);
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let input = vec![b'x'; MAX_FRAME_BYTES + 1];
        let error = read_frame::<_, Request>(&mut BufReader::new(Cursor::new(input)))
            .expect_err("oversized input must fail");
        assert!(matches!(error, ProtocolError::FrameTooLarge { .. }));
    }

    #[test]
    fn security_subscription_command_round_trips_as_one_frame() {
        let request = Request::new(Command::SubscribeSecurityEvents);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).expect("fixture should encode");
        let decoded: Request = read_frame(&mut BufReader::new(Cursor::new(bytes)))
            .expect("fixture should decode")
            .expect("frame should exist");
        assert_eq!(decoded, request);
    }

    #[test]
    fn security_event_response_round_trips_as_one_frame() {
        let event = SecurityEvent::policy_decision(
            EventIdentity {
                binding_id: Uuid::new_v4(),
                agent_id: "agent-1".into(),
                agent_name: None,
                session_id: Some("session-1".into()),
                conversation_id: None,
                tool_call_id: None,
                pid: 42,
                process_start_time: 101,
                ppid: None,
                cgroup_id: None,
                protocol_version: PROTOCOL_VERSION,
                enforcer_version: "0.1.0".into(),
                actplane_revision: "fixture-revision".into(),
            },
            PolicyDecision {
                policy_id: "credential-exfiltration".into(),
                policy_revision: 1,
                source_event_id: Uuid::new_v4(),
                sink_event_id: Uuid::new_v4(),
                mode: PolicyMode::Audit,
                requested_effect: Effect::Notify,
                blocked: false,
                killed: false,
                errno: None,
                risk_score: 60,
                reason: "audit fixture".into(),
            },
        );
        let response = Response {
            protocol_version: PROTOCOL_VERSION,
            request_id: Uuid::new_v4(),
            result: Ok(ResponseBody::SecurityEvent(event)),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &response).expect("fixture should encode");
        let decoded: Response = read_frame(&mut BufReader::new(Cursor::new(bytes)))
            .expect("fixture should decode")
            .expect("frame should exist");
        assert_eq!(decoded, response);
    }

    #[test]
    fn credential_policy_command_round_trips_as_one_frame() {
        let request = Request::new(Command::ApplyCredentialPolicyLeased {
            request: ApplyCredentialPolicy {
                binding_id: Uuid::new_v4(),
                agent_id: "agent-1".into(),
                session_id: Some("session-1".into()),
                root_pid: 42,
                process_start_time: 101,
                policy: CredentialExfiltrationPolicy {
                    policy_id: "credential-exfiltration".into(),
                    revision: 3,
                    source_patterns: vec!["/root/.ssh/id_rsa".into()],
                    trusted_endpoints: vec!["10.0.0.8".into()],
                    taint_label: "CREDENTIAL".into(),
                    taint_ttl_secs: 900,
                    destination_scope: DestinationScope::PublicIpv4,
                    mode: PolicyMode::Audit,
                },
            },
            required_subscription_id: Uuid::new_v4(),
        });
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).expect("fixture should encode");
        let decoded: Request = read_frame(&mut BufReader::new(Cursor::new(bytes)))
            .expect("fixture should decode")
            .expect("frame should exist");
        assert_eq!(decoded, request);
    }
}
