use asc_daemon_client::ClientError;

/// Stable command execution failures printed to stderr.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The request failed before a daemon response was available.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The structured input file cannot be opened or read.
    #[error("input_unavailable: structured input could not be read")]
    InputUnavailable,
    /// The structured input exceeds the daemon frame bound.
    #[error("input_too_large: structured input exceeds the frame limit")]
    InputTooLarge,
    /// The input does not match the shared daemon DTO.
    #[error("invalid_input: structured input does not match the daemon DTO")]
    InvalidInput,
    /// The daemon rejected the envelope or handler boundary.
    #[error("{code}: {message}")]
    Daemon {
        /// Stable daemon error code.
        code: String,
        /// Safe daemon error message.
        message: String,
    },
    /// The daemon completed the handler but rejected the domain operation.
    #[error("{code}: {message}")]
    Rejected {
        /// Stable domain error code.
        code: String,
        /// Safe domain error message.
        message: String,
    },
    /// A response violated the expected three-layer shape.
    #[error("protocol_error: daemon response layers are inconsistent")]
    Protocol,
    /// Structured output could not be written.
    #[error("output_error: command output could not be written")]
    Output,
}
