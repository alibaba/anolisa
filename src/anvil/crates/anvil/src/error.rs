// SPDX-License-Identifier: Apache-2.0
//! Local errors for the anvil binary (daemon + CLI client).
//!
//! Wraps [`anvil_core::AnvilError`] so the daemon can additionally
//! surface I/O, hyper, and CLI-side failures without expanding the
//! public core error enum.

use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, AnvilDaemonError>;

#[derive(Debug, Error)]
pub enum AnvilDaemonError {
    #[error("core error: {0}")]
    Core(#[from] anvil_core::AnvilError),

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
        "could not connect to anvil daemon at {socket}: {source}\nIs the daemon running? Try: anvil daemon start --foreground"
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

impl AnvilDaemonError {
    /// HTTP status code that should accompany this error in API responses.
    pub fn status_code(&self) -> u16 {
        match self {
            AnvilDaemonError::BadRequest(_) => 400,
            AnvilDaemonError::NotFound(_) => 404,
            AnvilDaemonError::HttpStatus { status, .. } => *status,
            AnvilDaemonError::Core(anvil_core::AnvilError::PolicyEvalError { .. })
            | AnvilDaemonError::Core(anvil_core::AnvilError::InvalidStateTransition { .. }) => 422,
            AnvilDaemonError::Core(anvil_core::AnvilError::BackendUnavailable { .. }) => 503,
            _ => 500,
        }
    }
}
