//! Custom HTTP header handling for MCP HTTP transports.

use std::collections::HashMap;
use std::fmt;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::RequestBuilder;

use super::expand_env_vars;

/// Header names the MCP transports or HTTP framework control themselves.
/// User entries with these names (case-insensitive) are dropped at
/// construction so protocol behavior cannot be overridden through
/// configuration; authentication is owned by `bearer_token` and `oauth`
/// instead. The framework-managed entries (`content-length`, `host`,
/// `transfer-encoding`, `connection`, `user-agent`) are dropped because the
/// client derives them from the actual request, and a stale configured value
/// would desynchronize the wire from the body.
const RESERVED_HEADER_NAMES: &[&str] = &[
    "accept",
    "authorization",
    "connection",
    "content-length",
    "content-type",
    "host",
    "last-event-id",
    "mcp-protocol-version",
    "mcp-session-id",
    "transfer-encoding",
    "user-agent",
];

/// Validated custom headers for one MCP server's HTTP requests.
///
/// Reserved protocol header names are filtered out and values are
/// environment-expanded at construction, so request builders can apply the
/// surviving entries on every outbound request unconditionally.
#[derive(Default, Clone)]
pub(super) struct CustomHeaders {
    headers: HeaderMap,
}

impl fmt::Debug for CustomHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Values frequently carry credentials (PRIVATE-TOKEN and friends),
        // so the Debug output lists header names only.
        let mut names: Vec<String> = self
            .headers
            .keys()
            .map(|name| name.as_str().to_string())
            .collect();
        names.sort();
        f.debug_struct("CustomHeaders")
            .field("headers", &names)
            .finish()
    }
}

impl CustomHeaders {
    /// Validates raw configuration entries into an applicable header set.
    ///
    /// # Errors
    ///
    /// Returns an error naming the offending entry when a header name is not
    /// a valid HTTP token, a value contains characters forbidden in header
    /// values (for example newlines), or two names differ only by case.
    pub(super) fn from_config(config: &HashMap<String, String>) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        // Sorted iteration keeps duplicate-name errors deterministic across
        // process starts (HashMap iteration order is randomized).
        let mut entries: Vec<_> = config.iter().collect();
        entries.sort_by(|left, right| left.0.cmp(right.0));
        for (name, value) in entries {
            if RESERVED_HEADER_NAMES.contains(&name.to_ascii_lowercase().as_str()) {
                continue;
            }
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| format!("invalid MCP header name '{name}': {error}"))?;
            // HeaderMap matching is case-insensitive while TOML keys are
            // case-sensitive, so X-Token and x-token would otherwise race for
            // the slot and pick a winner at random on every start.
            if headers.contains_key(&header_name) {
                return Err(format!(
                    "duplicate MCP header name '{name}': header names are case-insensitive"
                ));
            }
            let mut header_value = HeaderValue::from_str(&expand_env_vars(value))
                .map_err(|error| format!("invalid MCP header value for '{name}': {error}"))?;
            // Values are usually credentials; the sensitive flag keeps
            // reqwest/hyper tracing from logging the raw values.
            header_value.set_sensitive(true);
            headers.insert(header_name, header_value);
        }
        Ok(Self { headers })
    }

    /// Applies the custom headers to an outgoing request.
    ///
    /// Reserved names were already filtered at construction, so protocol
    /// headers set by the caller are never overridden.
    pub(super) fn apply(&self, request: RequestBuilder) -> RequestBuilder {
        if self.headers.is_empty() {
            return request;
        }
        request.headers(self.headers.clone())
    }

    /// Returns whether any custom header will be applied.
    pub(super) fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn drops_reserved_header_names_case_insensitively() {
        let custom = CustomHeaders::from_config(&config(&[
            ("Authorization", "token"),
            ("AUTHORIZATION", "other"),
            ("CONTENT-TYPE", "text/plain"),
            ("mcp-session-id", "session"),
            ("Content-Length", "9999"),
            ("Host", "evil.example.com"),
            ("PRIVATE-TOKEN", "secret"),
        ]))
        .unwrap();
        assert_eq!(custom.headers.len(), 1);
        assert_eq!(custom.headers.get("PRIVATE-TOKEN").unwrap(), "secret");
    }

    #[test]
    fn rejects_case_only_duplicate_header_names() {
        let error = CustomHeaders::from_config(&config(&[("X-Token", "one"), ("x-token", "two")]))
            .unwrap_err();
        assert!(error.contains("duplicate MCP header name"), "got: {error}");
    }

    #[test]
    fn debug_output_redacts_header_values() {
        let custom = CustomHeaders::from_config(&config(&[("PRIVATE-TOKEN", "secret")])).unwrap();
        let debug = format!("{custom:?}");
        // HeaderMap normalizes names to lowercase.
        assert!(debug.contains("private-token"), "got: {debug}");
        assert!(!debug.contains("secret"), "got: {debug}");
    }

    #[test]
    fn marks_header_values_sensitive() {
        let custom = CustomHeaders::from_config(&config(&[("PRIVATE-TOKEN", "secret")])).unwrap();
        assert!(custom.headers.get("PRIVATE-TOKEN").unwrap().is_sensitive());
    }

    #[test]
    fn keeps_unknown_headers_verbatim() {
        let custom = CustomHeaders::from_config(&config(&[("X-Trace-Id", "abc 123")])).unwrap();
        assert_eq!(custom.headers.get("X-Trace-Id").unwrap(), "abc 123");
    }

    #[test]
    fn expands_unset_environment_variables_to_empty_values() {
        let custom =
            CustomHeaders::from_config(&config(&[("X-Env", "${R3032_UNDEFINED_VARIABLE}")]))
                .unwrap();
        assert_eq!(custom.headers.get("X-Env").unwrap(), "");
    }

    #[test]
    fn rejects_invalid_header_name() {
        let error = CustomHeaders::from_config(&config(&[("bad name", "value")])).unwrap_err();
        assert!(error.contains("invalid MCP header name"), "got: {error}");
    }

    #[test]
    fn rejects_header_value_with_newline() {
        let error = CustomHeaders::from_config(&config(&[("X-Bad", "line\nsplit")])).unwrap_err();
        assert!(error.contains("invalid MCP header value"), "got: {error}");
    }

    #[test]
    fn applies_headers_without_touching_protocol_headers() {
        let custom = CustomHeaders::from_config(&config(&[("PRIVATE-TOKEN", "secret")])).unwrap();
        let request = reqwest::Client::new()
            .post("http://127.0.0.1:1/mcp")
            .header(reqwest::header::CONTENT_TYPE, "application/json");
        let request = custom.apply(request);
        let request = request.build().unwrap();
        assert_eq!(request.headers().get("PRIVATE-TOKEN").unwrap(), "secret");
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .unwrap(),
            "application/json"
        );
        assert_eq!(
            request
                .headers()
                .get_all(reqwest::header::CONTENT_TYPE)
                .iter()
                .count(),
            1
        );
    }

    #[test]
    fn applies_nothing_for_empty_header_set() {
        let custom = CustomHeaders::default();
        let request = reqwest::Client::new().post("http://127.0.0.1:1/mcp");
        let request = custom.apply(request);
        let request = request.build().unwrap();
        assert!(request.headers().get("PRIVATE-TOKEN").is_none());
    }
}
