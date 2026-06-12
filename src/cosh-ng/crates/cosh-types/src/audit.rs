//! Types for the audit subsystem.
//!
//! See `docs/audit-design.md` for the full design. This module is the pure
//! type layer — no I/O, no policy evaluation, no log writing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// =====================================================================
// Action — structured input to the PDP.
// =====================================================================

/// A structured action submitted to the audit subsystem. Raw shell strings
/// must be parsed into an `Action` (with shell metacharacters rejected) by
/// `cosh_platform::audit::action`. Audit rules never match against
/// `raw` — it is preserved purely for log display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub subsystem: ActionSubsystem,
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// Subsystem identifier. Serialized as a lowercase string. Unknown values
/// (forward-compatibility for new command domains) round-trip through
/// `Other(name)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionSubsystem {
    Pkg,
    Svc,
    Checkpoint,
    Shell,
    Cosh,
    Other(String),
}

impl ActionSubsystem {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pkg => "pkg",
            Self::Svc => "svc",
            Self::Checkpoint => "checkpoint",
            Self::Shell => "shell",
            Self::Cosh => "cosh",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Parse from a textual token (case-insensitive for known variants).
    pub fn from_token(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "pkg" => Self::Pkg,
            "svc" => Self::Svc,
            "checkpoint" => Self::Checkpoint,
            "shell" => Self::Shell,
            "cosh" => Self::Cosh,
            _ => Self::Other(s.to_string()),
        }
    }
}

impl Serialize for ActionSubsystem {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ActionSubsystem {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Ok(Self::from_token(&s))
    }
}

// =====================================================================
// Decision — output of the PDP.
// =====================================================================

/// The decision produced by the PDP for a given Action under a given Policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub outcome: Outcome,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
    pub policy_version: String,
}

/// Three-state decision outcome.
///
/// `RequireApproval` is the third state — distinct from `Deny`. It exists to
/// model "safe to auto-run / needs human / never auto-run" without forcing
/// a binary collapse. PEPs can map `RequireApproval` to `Deny` in non-
/// interactive contexts, and to a confirmation prompt in interactive ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Allow,
    Deny,
    RequireApproval,
}

// =====================================================================
// Policy / Rule / Match — declarative ruleset.
// =====================================================================

/// A complete audit policy. The first matching rule wins; if none match,
/// `default` is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: String,
    pub default: Outcome,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    pub matches: Match,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Match conditions for a Rule. ALL specified fields must match for the rule
/// to fire (logical AND). A `Match` with all fields empty is rejected at
/// load time — see `Policy::from_toml_str`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subsystem: Option<ActionSubsystem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<StringMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<StringMatch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arg: Vec<ArgMatch>,
}

impl Match {
    /// True when no field is set — such a rule would match every action and
    /// is therefore rejected at load time.
    pub fn is_empty(&self) -> bool {
        self.subsystem.is_none()
            && self.operation.is_none()
            && self.target.is_none()
            && self.arg.is_empty()
    }
}

/// String match. TOML syntax is uniform across all match fields:
/// - `field = "value"`              → `Exact`
/// - `field = { one_of = [...] }`   → `OneOf`
/// - `field = { glob = "ng*" }`     → `Glob` (only `*` and `?`)
///
/// No regex by design — see `docs/audit-design.md` §3.3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringMatch {
    Exact(String),
    OneOf {
        one_of: Vec<String>,
    },
    Glob {
        glob: String,
    },
}

/// Match an entry in `Action.args`. An ArgMatch fires when the action has at
/// least one `(k, v)` pair where `key` matches `k` and (if specified) `value`
/// matches `v`. Multiple ArgMatch entries in a `Match` are combined with AND
/// (all must find a satisfying pair).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgMatch {
    pub key: StringMatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<StringMatch>,
}

// =====================================================================
// LogEntry — append-only audit record.
// =====================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub user: String,
    pub uid: u32,
    pub euid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sudo_user: Option<String>,
    pub pid: u32,
    pub action: Action,
    pub decision: Decision,
    pub source: LogSource,
    pub redacted: bool,
}

/// Origin of an audit call — useful for filtering logs by caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LogSource {
    Cli,
    Tui { tool_name: String },
    External { caller: String },
}

// =====================================================================
// Tests.
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsystem_serializes_as_lowercase() {
        let s = serde_json::to_string(&ActionSubsystem::Pkg).unwrap();
        assert_eq!(s, "\"pkg\"");
        let s = serde_json::to_string(&ActionSubsystem::Checkpoint).unwrap();
        assert_eq!(s, "\"checkpoint\"");
    }

    #[test]
    fn subsystem_from_token_is_case_insensitive() {
        assert_eq!(ActionSubsystem::from_token("pkg"), ActionSubsystem::Pkg);
        assert_eq!(ActionSubsystem::from_token("PKG"), ActionSubsystem::Pkg);
        assert_eq!(
            ActionSubsystem::from_token("custom"),
            ActionSubsystem::Other("custom".to_string())
        );
    }

    #[test]
    fn outcome_serializes_pascal_case() {
        assert_eq!(serde_json::to_string(&Outcome::Allow).unwrap(), "\"Allow\"");
        assert_eq!(
            serde_json::to_string(&Outcome::RequireApproval).unwrap(),
            "\"RequireApproval\""
        );
    }

    #[test]
    fn action_round_trip() {
        let act = Action {
            subsystem: ActionSubsystem::Pkg,
            operation: "install".to_string(),
            target: Some("nginx".to_string()),
            args: vec![("dry-run".to_string(), "true".to_string())],
            raw: Some("pkg install nginx --dry-run".to_string()),
        };
        let json = serde_json::to_string(&act).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(act, back);
    }

    #[test]
    fn decision_round_trip() {
        let dec = Decision {
            outcome: Outcome::Deny,
            reason: "destructive".to_string(),
            matched_rule: Some("shell-deny-destructive".to_string()),
            policy_version: "builtin-balanced@0.2.0+sha256:abc".to_string(),
        };
        let json = serde_json::to_string(&dec).unwrap();
        let back: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(dec, back);
    }

    #[test]
    fn string_match_untagged_round_trip() {
        // Exact via plain string
        let exact: StringMatch = serde_json::from_str("\"install\"").unwrap();
        assert_eq!(exact, StringMatch::Exact("install".to_string()));

        // OneOf
        let oo: StringMatch = serde_json::from_str(r#"{"one_of":["a","b"]}"#).unwrap();
        assert_eq!(
            oo,
            StringMatch::OneOf {
                one_of: vec!["a".to_string(), "b".to_string()]
            }
        );

        // Glob
        let g: StringMatch = serde_json::from_str(r#"{"glob":"ng*"}"#).unwrap();
        assert_eq!(
            g,
            StringMatch::Glob {
                glob: "ng*".to_string()
            }
        );
    }

    #[test]
    fn match_is_empty_detects_blank_match() {
        let m = Match::default();
        assert!(m.is_empty());

        let m = Match {
            operation: Some(StringMatch::Exact("install".to_string())),
            ..Match::default()
        };
        assert!(!m.is_empty());
    }

    // TOML-based load/validation tests live in `cosh-platform::audit::policy`,
    // which owns the TOML adapter and `Policy::from_toml_str` validation.

    #[test]
    fn log_entry_round_trip() {
        let entry = LogEntry {
            timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            session_id: "sess-1".to_string(),
            user: "alice".to_string(),
            uid: 1000,
            euid: 1000,
            sudo_user: None,
            pid: 1234,
            action: Action {
                subsystem: ActionSubsystem::Pkg,
                operation: "install".to_string(),
                target: Some("nginx".to_string()),
                args: vec![],
                raw: None,
            },
            decision: Decision {
                outcome: Outcome::Allow,
                reason: "default".to_string(),
                matched_rule: None,
                policy_version: "builtin-permissive@0.2.0+sha256:abc".to_string(),
            },
            source: LogSource::Cli,
            redacted: false,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn log_source_internally_tagged() {
        let s = serde_json::to_string(&LogSource::Cli).unwrap();
        assert!(s.contains("\"kind\":\"cli\""), "got {}", s);

        let s = serde_json::to_string(&LogSource::Tui {
            tool_name: "shell".to_string(),
        })
        .unwrap();
        assert!(s.contains("\"kind\":\"tui\""), "got {}", s);
        assert!(s.contains("\"tool_name\":\"shell\""), "got {}", s);
    }
}
