use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable daemon error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DaemonError {
    /// Stable machine-readable code.
    pub code: String,
    /// Safe operator-facing message.
    pub message: String,
}

/// One response frame. Domain rejection remains an `ok=true` handler result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DaemonResponse {
    /// Daemon-generated request correlation identity.
    pub request_id: String,
    /// Whether the daemon boundary and handler completed.
    pub ok: bool,
    /// Method-specific result or rejection.
    pub data: Value,
    /// Reserved compatibility field.
    pub stdout: String,
    /// Safe failure text for command-oriented clients.
    pub stderr: String,
    /// Zero for accepted method results, one for rejection/failure.
    pub exit_code: i32,
    /// Present only for daemon-boundary failures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DaemonError>,
}

impl DaemonResponse {
    /// Creates a daemon-boundary failure.
    pub fn daemon_error(request_id: String, code: &str, message: &str) -> Self {
        Self {
            request_id,
            ok: false,
            data: empty_object(),
            stdout: String::new(),
            stderr: message.to_owned(),
            exit_code: 1,
            error: Some(DaemonError {
                code: code.to_owned(),
                message: message.to_owned(),
            }),
        }
    }

    /// Creates a successful method result.
    pub fn success(request_id: String, data: Value) -> Self {
        Self {
            request_id,
            ok: true,
            data,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            error: None,
        }
    }

    /// Creates a structured domain rejection while preserving the daemon response layer.
    pub fn rejected(request_id: String, code: &str, message: &str) -> Self {
        Self {
            request_id,
            ok: true,
            data: serde_json::json!({
                "disposition": "REJECTED",
                "error": {"code": code, "message": message}
            }),
            stdout: String::new(),
            stderr: message.to_owned(),
            exit_code: 1,
            error: None,
        }
    }
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}
