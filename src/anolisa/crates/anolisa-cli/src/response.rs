//! Unified CLI response envelope, error model, and renderer.
//!
//! Both human-readable and `--json` output flow through the same
//! [`CliResponse`] envelope (see launch spec §4). Handlers may render
//! their own human text directly to stdout, and on `--json` they hand a
//! payload to [`render_json`] / [`render_error`] so the on-the-wire
//! shape stays consistent across surfaces.
//!
//! Exit codes:
//! - `NOT_IMPLEMENTED` -> 64 (reserved CLI code for "command exists but
//!   handler is not wired"; chosen because POSIX `EX_USAGE` is 64 and is
//!   the closest established sentinel — launch spec §4 does not pin an
//!   exact value, so we pick a non-zero reserved code and document it
//!   here for future tightening).
//! - `INVALID_ARGUMENT` -> 2 (POSIX convention shared with clap).
//! - `EXECUTION_FAILED` -> 1 (generic non-zero "the command ran but the
//!   underlying operation failed at runtime"). Distinct from
//!   `INVALID_ARGUMENT` so callers can tell "I gave you bad input" apart
//!   from "you tried and something on the machine refused": download
//!   IO, install IO, state-write IO, log-write IO, lock IO. Plan-time
//!   refusals (e.g. blocked plan, unknown component) stay
//!   `INVALID_ARGUMENT` — they tell the caller to fix the input or the
//!   environment before retrying.

use std::process::ExitCode;

use serde::Serialize;

use crate::color::Palette;
use crate::context::CliContext;

/// JSON schema version for the CLI response envelope. Bump when the
/// envelope shape changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Common envelope shared by human and JSON output paths.
#[derive(Debug, Serialize)]
pub struct CliResponse<T: Serialize> {
    pub ok: bool,
    pub schema_version: u32,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CliErrorPayload>,
}

#[derive(Debug, Serialize)]
pub struct CliErrorPayload {
    pub code: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Errors a handler can surface. The dispatcher converts these into the
/// process exit code via [`render_error`].
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Command exists in the surface but no real implementation yet.
    #[error("command '{command}' is not implemented")]
    NotImplemented {
        command: String,
        hint: Option<String>,
    },

    /// Caller-supplied arguments violated a contract.
    #[error("invalid argument: {reason}")]
    InvalidArgument { command: String, reason: String },

    /// The command was well-formed but the underlying operation failed
    /// at runtime (download IO, install IO, state-write IO, log-write
    /// IO, install-lock contention/IO, etc.). Surfaced as exit code 1
    /// so wrapping scripts can distinguish "bad input" (exit 2) from
    /// "the machine refused" (exit 1).
    #[error("execution failed: {reason}")]
    Runtime { command: String, reason: String },

    /// The command completed but the resulting state is degraded
    /// (e.g. sandbox install where one or more phases emitted warnings
    /// rather than hard failure). Maps to exit code 2 so wrapping
    /// scripts can distinguish "clean success" (0) from "installed
    /// but needs attention" (2). Phase-level failures are still
    /// surfaced as `Runtime` (exit 1).
    #[error("degraded: {reason}")]
    Degraded { command: String, reason: String },
}

impl CliError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented { .. } => "NOT_IMPLEMENTED",
            Self::InvalidArgument { .. } => "INVALID_ARGUMENT",
            Self::Runtime { .. } => "EXECUTION_FAILED",
            Self::Degraded { .. } => "DEGRADED",
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotImplemented { .. } => 64,
            Self::InvalidArgument { .. } => 2,
            Self::Runtime { .. } => 1,
            Self::Degraded { .. } => 2,
        }
    }

    pub fn command(&self) -> &str {
        match self {
            Self::NotImplemented { command, .. } => command,
            Self::InvalidArgument { command, .. } => command,
            Self::Runtime { command, .. } => command,
            Self::Degraded { command, .. } => command,
        }
    }

    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::NotImplemented { hint, .. } => hint.as_deref(),
            Self::InvalidArgument { .. } => None,
            Self::Runtime { .. } => None,
            Self::Degraded { .. } => None,
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Self::NotImplemented { command, .. } => {
                format!("command '{command}' is not implemented")
            }
            Self::InvalidArgument { reason, .. } => reason.clone(),
            Self::Runtime { reason, .. } => reason.clone(),
            Self::Degraded { reason, .. } => reason.clone(),
        }
    }

    pub fn not_implemented(command: impl Into<String>) -> Self {
        Self::NotImplemented {
            command: command.into(),
            hint: None,
        }
    }

    pub fn not_implemented_with_hint(command: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::NotImplemented {
            command: command.into(),
            hint: Some(hint.into()),
        }
    }
}

/// Print a successful JSON envelope to stdout. Callers should only invoke
/// this on the `--json` branch (human path stays plain `println!`).
///
/// A serialization failure surfaces as `CliError::Runtime` so the
/// caller's exit code reflects the failure instead of silently
/// returning `Ok(())`.
pub fn render_json<T: Serialize>(command: &str, data: T) -> Result<(), CliError> {
    let response = CliResponse {
        ok: true,
        schema_version: SCHEMA_VERSION,
        command: command.to_string(),
        data: Some(data),
        warnings: Vec::new(),
        error: None,
    };
    write_json(&response).map_err(|e| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to serialize JSON response: {e}"),
    })
}

/// Print an empty success envelope (no data payload).
#[allow(dead_code)]
pub fn render_ok(command: &str) -> Result<(), CliError> {
    let response: CliResponse<()> = CliResponse {
        ok: true,
        schema_version: SCHEMA_VERSION,
        command: command.to_string(),
        data: None,
        warnings: Vec::new(),
        error: None,
    };
    write_json(&response).map_err(|e| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to serialize JSON response: {e}"),
    })
}

/// Render an error and return the process exit code to surface.
///
/// On `--json` we emit a `CliResponse` envelope on stdout (so machine
/// callers always get parseable output, error or not). On the human path
/// we write to stderr per launch spec §4 ("warnings/debug to stderr").
///
/// If the error envelope itself fails to serialize, we fall back to
/// stderr but still return the original error's exit code so callers
/// see the failure they expected.
pub fn render_error(ctx: &CliContext, err: &CliError) -> ExitCode {
    if ctx.json {
        let payload = CliErrorPayload {
            code: err.code().to_string(),
            reason: err.reason(),
            hint: err.hint().map(|s| s.to_string()),
        };
        let response: CliResponse<()> = CliResponse {
            ok: false,
            schema_version: SCHEMA_VERSION,
            command: err.command().to_string(),
            data: None,
            warnings: Vec::new(),
            error: Some(payload),
        };
        if let Err(serialize_err) = write_json(&response) {
            eprintln!(
                "internal: failed to serialize error envelope: {serialize_err}; original error[{}]: {}",
                err.code(),
                err.reason()
            );
        }
    } else {
        let color = Palette::new(ctx.no_color);
        eprintln!(
            "{} {}",
            color.err(format!("error[{}]:", err.code())),
            err.reason()
        );
        if let Some(hint) = err.hint() {
            eprintln!("{} {}", color.warn("hint:"), hint);
        }
    }
    ExitCode::from(err.exit_code())
}

fn write_json<T: Serialize>(response: &CliResponse<T>) -> Result<(), serde_json::Error> {
    let s = serde_json::to_string_pretty(response)?;
    println!("{s}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::{Error as SerError, Serializer};

    /// A payload whose `Serialize` impl always fails. Used to prove
    /// `render_json` surfaces serialization failures as `CliError`
    /// instead of silently returning `Ok(())`.
    struct AlwaysFails;

    impl Serialize for AlwaysFails {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(S::Error::custom("intentional test failure"))
        }
    }

    #[test]
    fn render_json_returns_runtime_error_when_payload_fails_to_serialize() {
        let err = render_json("status", AlwaysFails).expect_err("serialization must fail");
        match err {
            CliError::Runtime { command, reason } => {
                assert_eq!(command, "status");
                assert!(
                    reason.contains("intentional test failure"),
                    "reason should carry the underlying serde error, got: {reason}"
                );
            }
            other => panic!("expected CliError::Runtime, got {other:?}"),
        }
    }
}
