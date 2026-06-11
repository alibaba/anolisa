use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::audit::AuditEntry;
use crate::error::{MemoryError, Result};
use crate::service::MemoryService;

const TOOL: &str = "memory_observe";

/// Closed 4-type memory taxonomy inspired by Dreaming V3 and Claude Code memdir.
///
/// Design rationale: maps to a 2×2 matrix (personal/project × subjective/objective).
/// Open taxonomies cause type explosion and classification ambiguity.
///
/// | Type       | Personal × Subjective | Project × Subjective |
/// |------------|----------------------|---------------------|
/// | User       | ✓ (who you are)      |                     |
/// | Feedback   | ✓ (how to behave)    |                     |
/// | Project    |                      | ✓ (what/why/when)   |
/// | Reference  |                      | ✓ (where to find)   |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    /// Personal × Subjective: user profile, preferences, knowledge background.
    /// "I'm a Rust developer", "I prefer concise responses"
    User,
    /// Personal × Objective: Agent behavior corrections and confirmations.
    /// "Don't use var, use const", "Always run tests before committing"
    Feedback,
    /// Project × Subjective: decisions, status, conventions, deadlines.
    /// "Auth uses JWT for mobile support", "Migration deadline is March 1"
    Project,
    /// Project × Objective: pointers to external resources.
    /// "Grafana dashboard at https://...", "API docs on Confluence"
    Reference,
}

impl MemoryType {
    /// Parse from string, case-insensitive.
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "user" => Ok(Self::User),
            "feedback" => Ok(Self::Feedback),
            "project" => Ok(Self::Project),
            "reference" => Ok(Self::Reference),
            _ => Err(MemoryError::InvalidArgument(format!(
                "unknown memory type '{s}'; expected user, feedback, project, or reference"
            ))),
        }
    }
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryType::User => write!(f, "user"),
            MemoryType::Feedback => write!(f, "feedback"),
            MemoryType::Project => write!(f, "project"),
            MemoryType::Reference => write!(f, "reference"),
        }
    }
}

// Non-derivable information principle:
// These categories should NOT be stored as memories because they can
// be obtained in real-time from the codebase or Git history:
//
// - Code patterns, architecture, file structure → `ls`, `grep`
// - Git history → `git log`, `git blame`
// - Debug solutions → commit messages
// - Third-party library versions → package manifests
//
// The `memory_observe` tool description should guide the Agent toward
// storing only non-derivable information.

/// Tier B: record an observation with optional type classification.
/// The OS picks a stable filename under `notes/observed/<ulid>.md` and
/// writes frontmatter (type + hint + created_at) + body.
pub fn memory_observe(
    svc: &MemoryService,
    content: &str,
    hint: Option<&str>,
    memory_type: Option<&str>,
) -> Result<String> {
    let ulid = ulid::Ulid::new();
    let path = format!("notes/observed/{ulid}.md");

    // Parse and validate memory type, defaulting to User
    let parsed_type = match memory_type {
        Some(t) => MemoryType::parse(t)?,
        None => MemoryType::User,
    };

    let mut body = String::new();
    body.push_str("---\n");

    // Always write type (defaults to "user")
    body.push_str(&format!("type: {parsed_type}\n"));

    if let Some(h) = hint {
        let safe = h.replace('\n', " ");
        body.push_str(&format!("hint: {safe}\n"));
    }

    // Mark source as manual-observe for sovereignty tracking
    body.push_str("source: manual-observe\n");
    body.push_str(&format!("created_at: {}\n", Utc::now().to_rfc3339()));
    body.push_str("---\n\n");
    body.push_str(content);
    if !content.ends_with('\n') {
        body.push('\n');
    }

    let n = svc.write(&path, &body, false)?;
    svc.audit_log(AuditEntry::new(TOOL).path(path.clone()).bytes(n));
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_type_parse_valid() {
        assert_eq!(MemoryType::parse("user").unwrap(), MemoryType::User);
        assert_eq!(MemoryType::parse("Feedback").unwrap(), MemoryType::Feedback);
        assert_eq!(MemoryType::parse("PROJECT").unwrap(), MemoryType::Project);
        assert_eq!(
            MemoryType::parse("reference").unwrap(),
            MemoryType::Reference
        );
    }

    #[test]
    fn memory_type_parse_invalid() {
        assert!(MemoryType::parse("architecture").is_err());
        assert!(MemoryType::parse("bug").is_err());
        assert!(MemoryType::parse("").is_err());
    }

    #[test]
    fn memory_type_display() {
        assert_eq!(MemoryType::User.to_string(), "user");
        assert_eq!(MemoryType::Feedback.to_string(), "feedback");
        assert_eq!(MemoryType::Project.to_string(), "project");
        assert_eq!(MemoryType::Reference.to_string(), "reference");
    }

    #[test]
    fn memory_type_serialize() {
        let json = serde_json::to_string(&MemoryType::Feedback).unwrap();
        assert_eq!(json, r#""feedback""#);
    }

    #[test]
    fn memory_type_deserialize() {
        let mt: MemoryType = serde_json::from_str(r#""project""#).unwrap();
        assert_eq!(mt, MemoryType::Project);
    }
}
