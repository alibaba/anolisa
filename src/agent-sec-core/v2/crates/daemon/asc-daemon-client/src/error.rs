/// Stable failures produced before a daemon response is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// The management credential cannot be loaded safely.
    #[error("credential_unavailable: management credential could not be loaded")]
    CredentialUnavailable,
    /// The configured daemon socket cannot be connected.
    #[error("daemon_unavailable: daemon socket is not available")]
    DaemonUnavailable,
    /// A connected daemon did not complete I/O within the configured bound.
    #[error("daemon_timeout: daemon request exceeded the I/O timeout")]
    Timeout,
    /// A connected socket failed while sending or receiving a frame.
    #[error("daemon_transport: daemon request transport failed")]
    Transport,
    /// Request parameters could not be represented by the protocol.
    #[error("request_serialization: daemon request could not be serialized")]
    RequestSerialization,
    /// The serialized request exceeds the shared frame limit.
    #[error("request_too_large: daemon request exceeds the frame limit")]
    RequestTooLarge,
    /// The daemon response exceeds the shared frame limit.
    #[error("response_too_large: daemon response exceeds the frame limit")]
    ResponseTooLarge,
    /// The peer returned an incomplete or invalid daemon response.
    #[error("protocol_error: daemon returned an invalid response frame")]
    Protocol,
}

impl ClientError {
    /// Returns the stable machine-readable error category.
    pub const fn code(self) -> &'static str {
        match self {
            Self::CredentialUnavailable => "credential_unavailable",
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::Timeout => "daemon_timeout",
            Self::Transport => "daemon_transport",
            Self::RequestSerialization => "request_serialization",
            Self::RequestTooLarge => "request_too_large",
            Self::ResponseTooLarge => "response_too_large",
            Self::Protocol => "protocol_error",
        }
    }
}
