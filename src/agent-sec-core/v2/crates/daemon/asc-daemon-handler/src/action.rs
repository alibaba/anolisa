//! Code-scan Action adapter invoked by the daemon dispatcher.
//!
//! Translates one `action.code_scan` request into a capability call and back.
//! The capability is pure and stateless, so this handler holds no application
//! port: it decodes parameters, resolves the language, runs the scan, and
//! projects the [`ScanResult`] as the method result.

use asc_capability_code_scan::{Language, ScanResult, scan};
use asc_daemon_protocol::{
    CodeScanParams, DaemonResponse, MAX_DAEMON_ERROR_MESSAGE_BYTES, RequestId, error_code,
};

/// Default engine mode when the request omits one.
const DEFAULT_MODE: &str = "regex";

const INVALID_PARAMETER_MESSAGE: &str = "request parameters are invalid";

/// Stateless code-scan protocol adapter.
pub(super) struct CodeScanHandler;

impl CodeScanHandler {
    pub(super) const fn new() -> Self {
        Self
    }

    /// Runs one scan and projects its result or a parameter failure.
    ///
    /// A scan that produces an error verdict is still a successful request: the
    /// scan ran and returned a verdict the caller must act on. Only malformed
    /// parameters or an unsupported language name become protocol errors.
    #[allow(
        clippy::unused_self,
        reason = "mirrors the stateful PapHandler and reserves room for future engine configuration"
    )]
    pub(super) fn handle(
        &self,
        request_id: RequestId,
        params: serde_json::Value,
    ) -> DaemonResponse {
        let params: CodeScanParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return DaemonResponse::error(
                    request_id,
                    error_code::INVALID_REQUEST,
                    &bounded_parameter_error(&error),
                );
            }
        };

        let language = match Language::parse(&params.language) {
            Ok(language) => language,
            Err(error) => {
                return DaemonResponse::error(
                    request_id,
                    error_code::INVALID_ARGUMENT,
                    &error.to_string(),
                );
            }
        };

        let mode = params.mode.as_deref().unwrap_or(DEFAULT_MODE);
        let result = scan(&params.code, language, params.rules.as_deref(), mode);
        match project(&result) {
            Ok(value) => DaemonResponse::success(request_id, value),
            // A ScanResult is a fixed, bounded shape of owned strings; failing
            // to serialize it would be an internal invariant break, not caller
            // input, so it is projected as an internal error.
            Err(()) => DaemonResponse::error(
                request_id,
                error_code::INTERNAL,
                "scan result is unprojectable",
            ),
        }
    }
}

/// Serializes a [`ScanResult`] into the transport result value.
fn project(result: &ScanResult) -> Result<serde_json::Value, ()> {
    serde_json::to_value(result).map_err(|_| ())
}

fn bounded_parameter_error(error: &serde_json::Error) -> String {
    let message = error.to_string();
    if message.len() > MAX_DAEMON_ERROR_MESSAGE_BYTES {
        INVALID_PARAMETER_MESSAGE.to_owned()
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_id() -> RequestId {
        RequestId::new("test").expect("non-empty request id")
    }

    /// Returns the success result value or panics if the response was an error.
    fn success_value(response: DaemonResponse) -> serde_json::Value {
        match response {
            DaemonResponse::Success(response) => response.result,
            DaemonResponse::Error(response) => {
                panic!("expected success, got error {}", response.error.message())
            }
        }
    }

    /// Returns the error code or panics if the response was a success.
    fn error_code_of(response: DaemonResponse) -> String {
        match response {
            DaemonResponse::Error(response) => response.error.code.as_str().to_owned(),
            DaemonResponse::Success(_) => panic!("expected an error response"),
        }
    }

    #[test]
    fn clean_code_scans_to_a_pass_verdict() {
        let response = CodeScanHandler::new().handle(
            request_id(),
            serde_json::json!({"code": "echo hi", "language": "bash"}),
        );
        let value = success_value(response);
        assert_eq!(value["ok"], serde_json::json!(true));
        assert_eq!(value["verdict"], serde_json::json!("pass"));
        assert_eq!(value["language"], serde_json::json!("bash"));
    }

    #[test]
    fn dangerous_code_reports_findings() {
        let response = CodeScanHandler::new().handle(
            request_id(),
            serde_json::json!({"code": "rm -rf /tmp/x", "language": "bash"}),
        );
        let value = success_value(response);
        assert_eq!(value["verdict"], serde_json::json!("warn"));
        assert!(
            value["findings"].as_array().is_some_and(|f| !f.is_empty()),
            "expected at least one finding"
        );
    }

    #[test]
    fn an_error_verdict_is_still_a_success_response() {
        // Empty input is a valid request that yields an error verdict, not a
        // protocol error: the scan ran and returned a verdict.
        let response = CodeScanHandler::new().handle(
            request_id(),
            serde_json::json!({"code": "   ", "language": "python"}),
        );
        let value = success_value(response);
        assert_eq!(value["ok"], serde_json::json!(false));
        assert_eq!(value["verdict"], serde_json::json!("error"));
    }

    #[test]
    fn an_unsupported_language_is_invalid_argument() {
        let response = CodeScanHandler::new().handle(
            request_id(),
            serde_json::json!({"code": "puts 1", "language": "ruby"}),
        );
        assert_eq!(error_code_of(response), error_code::INVALID_ARGUMENT);
    }

    #[test]
    fn missing_required_fields_are_invalid_request() {
        let response =
            CodeScanHandler::new().handle(request_id(), serde_json::json!({"language": "bash"}));
        assert_eq!(error_code_of(response), error_code::INVALID_REQUEST);
    }

    #[test]
    fn llm_mode_yields_an_engine_unavailable_verdict_not_a_protocol_error() {
        let response = CodeScanHandler::new().handle(
            request_id(),
            serde_json::json!({"code": "echo hi", "language": "bash", "mode": "llm"}),
        );
        let value = success_value(response);
        assert_eq!(value["verdict"], serde_json::json!("error"));
        assert_eq!(
            value["summary"],
            serde_json::json!("scan error: LLM model not available")
        );
    }
}
