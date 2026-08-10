//! Built-in YAML rule definitions for the L1 rule engine.
//!
//! The YAML files live in this crate's `rules/` directory and are
//! embedded at compile time, so the engine carries its rule set with no
//! runtime file lookup.

use serde::Deserialize;

use crate::error::ScannerError;
use crate::result::Severity;

/// Built-in injection rules.
const INJECTION_YAML: &str = include_str!("../rules/injection.yaml");
/// Built-in jailbreak rules.
const JAILBREAK_YAML: &str = include_str!("../rules/jailbreak.yaml");

/// A single detection rule used by the L1 rule engine.
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    /// Unique identifier, e.g. "INJ-001".
    pub id: String,
    /// Human-readable rule name.
    pub name: String,
    /// "direct_injection" / "indirect_injection" / "jailbreak".
    pub category: String,
    /// e.g. "instruction_override".
    pub subcategory: String,
    pub severity: Severity,
    /// Regex patterns.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Fast pre-filter tokens (currently unused by the engine).
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// True: '.' does not match newlines for this rule.
    #[serde(default)]
    pub single_line: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct RuleFile {
    rules: Vec<Rule>,
}

/// Parse a YAML document with a top-level `rules:` list.
///
/// # Errors
///
/// Returns [`ScannerError::Config`] when the YAML is malformed or a rule is
/// missing a mandatory field.
fn parse_rules(yaml: &str, origin: &str) -> Result<Vec<Rule>, ScannerError> {
    let file: RuleFile = serde_yaml::from_str(yaml)
        .map_err(|exc| ScannerError::Config(format!("Invalid rule file {origin}: {exc}")))?;
    Ok(file.rules)
}

/// Load the built-in injection + jailbreak rules.
///
/// # Errors
///
/// Returns [`ScannerError::Config`] when an embedded YAML file is
/// malformed — indicates a packaging bug, not a runtime condition.
pub fn builtin_rules() -> Result<Vec<Rule>, ScannerError> {
    let mut rules = parse_rules(INJECTION_YAML, "injection.yaml")?;
    rules.extend(parse_rules(JAILBREAK_YAML, "jailbreak.yaml")?);
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rules_parse() {
        let rules = builtin_rules().expect("built-in YAML must parse");
        assert!(!rules.is_empty());
        // Every rule carries the mandatory fields.
        for rule in &rules {
            assert!(!rule.id.is_empty());
            assert!(
                !rule.patterns.is_empty(),
                "rule {} has no patterns",
                rule.id
            );
        }
        // Spot-check a known rule from each file.
        assert!(rules.iter().any(|r| r.id == "INJ-001"));
        assert!(rules.iter().any(|r| r.id == "JB-001"));
    }

    #[test]
    fn rule_ids_are_unique_across_builtin_files() {
        // The L1 engine collects at most one hit per rule by iterating the
        // set once, which only yields per-rule uniqueness if the ids
        // themselves are unique.  Lock that invariant here.
        let rules = builtin_rules().unwrap();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let duplicates: Vec<&str> = rules
            .iter()
            .filter(|rule| !seen.insert(rule.id.as_str()))
            .map(|rule| rule.id.as_str())
            .collect();
        assert!(duplicates.is_empty(), "duplicate rule ids: {duplicates:?}");
    }

    #[test]
    fn single_line_flag_is_parsed() {
        let rules = builtin_rules().unwrap();
        // INJ-015 is the only rule using single_line: true (multi-line
        // false-positive fix); make sure the flag survives parsing.
        let single_line: Vec<_> = rules.iter().filter(|r| r.single_line).collect();
        assert!(!single_line.is_empty());
    }
}
