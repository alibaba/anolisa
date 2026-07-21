// SPDX-License-Identifier: Apache-2.0
//! Unified error type for `anvil-core`.

use std::path::PathBuf;

use thiserror::Error;

/// Convenient `Result` alias defaulting to [`AnvilError`].
pub type Result<T> = std::result::Result<T, AnvilError>;

#[derive(Debug, Error)]
pub enum AnvilError {
    #[error("failed to load policy from {path}: {source}")]
    PolicyLoadError {
        path: PathBuf,
        #[source]
        source: Box<AnvilError>,
    },

    #[error("policy evaluation failed: {reason}")]
    PolicyEvalError { reason: String },

    #[error("no available backend for request: requested={requested:?}, available={available:?}")]
    BackendUnavailable {
        requested: Vec<String>,
        available: Vec<String>,
    },

    #[error("invalid sandbox state transition: {from} -> {to}")]
    InvalidStateTransition { from: String, to: String },

    #[error("template registry error: {msg}")]
    TemplateError { msg: String },

    #[error("hook '{hook_name}' error: {msg}")]
    HookError { hook_name: String, msg: String },

    #[error("config error: {source}")]
    ConfigError {
        #[source]
        source: ConfigErrorSource,
    },

    #[error("io error: {source}")]
    IoError {
        #[source]
        source: std::io::Error,
    },

    #[error("backend error: {msg}")]
    BackendError { msg: String },
}

/// Internal wrapper that lets [`AnvilError::ConfigError`] carry either a
/// TOML deserialization error or a JSON one without leaking those types
/// to public APIs.
#[derive(Debug, Error)]
pub enum ConfigErrorSource {
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid value: {0}")]
    InvalidValue(String),
}

impl From<std::io::Error> for AnvilError {
    fn from(source: std::io::Error) -> Self {
        AnvilError::IoError { source }
    }
}

impl From<toml::de::Error> for AnvilError {
    fn from(err: toml::de::Error) -> Self {
        AnvilError::ConfigError {
            source: ConfigErrorSource::Toml(err),
        }
    }
}

impl From<serde_json::Error> for AnvilError {
    fn from(err: serde_json::Error) -> Self {
        AnvilError::ConfigError {
            source: ConfigErrorSource::Json(err),
        }
    }
}
