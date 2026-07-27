// SPDX-License-Identifier: Apache-2.0
//! Local errors for the blazed binary (daemon + CLI client).
//!
//! Wraps [`blaze_core::BlazeError`] so the daemon can additionally
//! surface I/O, hyper, and CLI-side failures without expanding the
//! public core error enum.

use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, BlazeDaemonError>;

#[derive(Debug, Error)]
pub enum BlazeDaemonError {
    #[error("core error: {0}")]
    Core(#[from] blaze_core::BlazeError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("hyper http error: {0}")]
    HyperHttp(#[from] hyper::http::Error),

    #[error("hyper protocol error: {0}")]
    Hyper(#[from] hyper::Error),

    #[error(
        "could not connect to blaze daemon at {socket}: {source}\nIs the daemon running? Try: blazed daemon start --foreground"
    )]
    #[allow(dead_code)] // Constructed by client code; kept for future use.
    SocketConnect {
        socket: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("daemon returned status {status}: {body}")]
    #[allow(dead_code)] // Constructed by client code; kept for future use.
    HttpStatus { status: u16, body: String },

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl BlazeDaemonError {
    /// HTTP status code that should accompany this error in API responses.
    pub fn status_code(&self) -> u16 {
        match self {
            BlazeDaemonError::BadRequest(_) => 400,
            BlazeDaemonError::NotFound(_) => 404,
            BlazeDaemonError::HttpStatus { status, .. } => *status,
            BlazeDaemonError::Core(blaze_core::BlazeError::PolicyEvalError { .. })
            | BlazeDaemonError::Core(blaze_core::BlazeError::InvalidStateTransition { .. }) => 422,
            BlazeDaemonError::Core(blaze_core::BlazeError::BackendUnavailable { .. }) => 503,
            _ => 500,
        }
    }
}
