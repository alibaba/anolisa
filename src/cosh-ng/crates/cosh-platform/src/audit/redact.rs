//! Sensitive-field redaction for audit log entries.
//!
//! Redaction happens at log-write time, not at PEP→PDP boundary — see
//! `docs/audit-design.md` §8.4. The PDP may legitimately need to see raw
//! values (e.g. "deny if api_key is empty"), but those values must not
//! land in the on-disk JSONL log.

use cosh_types::audit::Action;

const SENSITIVE_KEY_NEEDLES: &[&str] = &[
    "password",
    "secret",
    "token",
    "api_key",
    "apikey",
];

const PEM_HEADERS: &[&str] = &[
    "BEGIN PRIVATE KEY",
    "BEGIN RSA PRIVATE KEY",
    "BEGIN OPENSSH PRIVATE KEY",
    "BEGIN EC PRIVATE KEY",
    "BEGIN DSA PRIVATE KEY",
];

const REDACTED_VALUE: &str = "<redacted>";
const REDACTED_PEM: &str = "<redacted-pem>";

/// Redact sensitive content in `action`. Returns true if any change was made
/// — caller should set `LogEntry.redacted = true` accordingly.
pub fn redact_action(action: &mut Action) -> bool {
    let mut changed = false;

    for (k, v) in action.args.iter_mut() {
        let key_lower = k.to_ascii_lowercase();
        let is_sensitive = SENSITIVE_KEY_NEEDLES
            .iter()
            .any(|needle| key_lower.contains(needle));
        if is_sensitive && v != REDACTED_VALUE {
            *v = REDACTED_VALUE.to_string();
            changed = true;
        }
    }

    if let Some(raw) = &action.raw {
        if PEM_HEADERS.iter().any(|h| raw.contains(h)) {
            action.raw = Some(REDACTED_PEM.to_string());
            changed = true;
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosh_types::audit::ActionSubsystem;

    fn pkg_action_with_args(args: Vec<(&str, &str)>) -> Action {
        Action {
            subsystem: ActionSubsystem::Pkg,
            operation: "install".to_string(),
            target: Some("nginx".to_string()),
            args: args.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            raw: None,
        }
    }

    #[test]
    fn redacts_password_key() {
        let mut a = pkg_action_with_args(vec![("password", "hunter2")]);
        assert!(redact_action(&mut a));
        assert_eq!(a.args[0].1, "<redacted>");
    }

    #[test]
    fn redacts_api_key_case_insensitive() {
        let mut a = pkg_action_with_args(vec![("API_KEY", "abcdef")]);
        assert!(redact_action(&mut a));
        assert_eq!(a.args[0].1, "<redacted>");

        let mut a = pkg_action_with_args(vec![("apikey", "xyz")]);
        assert!(redact_action(&mut a));
        assert_eq!(a.args[0].1, "<redacted>");
    }

    #[test]
    fn redacts_token_substring() {
        let mut a = pkg_action_with_args(vec![("auth_token", "bearer-xyz")]);
        assert!(redact_action(&mut a));
        assert_eq!(a.args[0].1, "<redacted>");
    }

    #[test]
    fn does_not_change_unrelated_keys() {
        let mut a = pkg_action_with_args(vec![("name", "nginx"), ("version", "1.25")]);
        assert!(!redact_action(&mut a));
        assert_eq!(a.args[0].1, "nginx");
        assert_eq!(a.args[1].1, "1.25");
    }

    #[test]
    fn redacts_pem_in_raw() {
        let mut a = Action {
            subsystem: ActionSubsystem::Shell,
            operation: "echo".to_string(),
            target: None,
            args: vec![],
            raw: Some("echo -----BEGIN PRIVATE KEY-----\\nMIIE...\\n-----END PRIVATE KEY-----".to_string()),
        };
        assert!(redact_action(&mut a));
        assert_eq!(a.raw.as_deref(), Some("<redacted-pem>"));
    }

    #[test]
    fn idempotent_when_already_redacted() {
        let mut a = pkg_action_with_args(vec![("password", "<redacted>")]);
        assert!(!redact_action(&mut a));
    }
}
