//! The compiled OS-adapter transform stage and its `TransformStage` impl.
//!
//! The stage is built once at mount startup (rules parsed, validated, and
//! specialized for the resolved target). Its `apply` runs only in-memory string
//! substitution — no YAML parsing, OS detection, or I/O on the per-read path.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::transform::TransformStage;

use super::detect::{OsTarget, TargetSelector, resolve_target};
use super::error::OsAdapterError;
use super::rules;

/// The compiled OS-adapter stage: an ordered list of literal `(from, to)`
/// substitutions specialized for one resolved [`OsTarget`].
#[derive(Debug)]
pub struct OsAdapterStage {
    target: OsTarget,
    /// Ordered literal substitutions; applied sequentially per read.
    rules: Vec<(String, String)>,
    /// Content-free digest of the rule artifact bytes, for diagnostics.
    digest: String,
    /// Number of rules parsed from the artifact (before direction filtering).
    total_rules: usize,
}

impl OsAdapterStage {
    /// Load, validate, and compile the rule artifact at `rules_path` for the
    /// given `selector`.
    ///
    /// # Errors
    ///
    /// Returns [`OsAdapterError`] when the file is missing/unreadable, the YAML
    /// is malformed, a rule has an invalid `direction`/`auto_apply`/empty
    /// pattern, the resolved target has duplicate/ambiguous source patterns, or
    /// `target_os = auto` cannot map `/etc/os-release` to a supported target.
    pub fn load(rules_path: &Path, selector: TargetSelector) -> Result<Self, OsAdapterError> {
        let bytes = std::fs::read(rules_path).map_err(|source| OsAdapterError::ReadRules {
            path: rules_path.to_path_buf(),
            source,
        })?;
        let target = resolve_target(selector)?;
        Self::from_bytes(&bytes, target, rules_path)
    }

    /// Compile a stage from raw artifact `bytes` for an already-resolved
    /// `target`. Split out from [`Self::load`] so target detection and file I/O
    /// stay testable independently.
    pub(crate) fn from_bytes(
        bytes: &[u8],
        target: OsTarget,
        path: &Path,
    ) -> Result<Self, OsAdapterError> {
        let compiled = rules::compile(bytes, target, path)?;
        Ok(Self {
            target,
            rules: compiled.rules,
            digest: digest_hex(bytes),
            total_rules: compiled.total_rules,
        })
    }

    /// The resolved target OS this stage converts content toward.
    pub fn target(&self) -> OsTarget {
        self.target
    }

    /// Stable, content-free SHA-256 digest (hex) of the rule artifact bytes.
    ///
    /// Suitable for initialization diagnostics and, being algorithm-stable
    /// across Rust versions and hosts, as a future cache-identity key.
    pub fn rule_digest(&self) -> &str {
        &self.digest
    }

    /// Number of rules parsed from the artifact (before direction filtering).
    pub fn total_rules(&self) -> usize {
        self.total_rules
    }

    /// Number of literal substitutions active for the resolved target.
    pub fn active_rules(&self) -> usize {
        self.rules.len()
    }
}

impl TransformStage for OsAdapterStage {
    fn name(&self) -> &'static str {
        "os_adapter"
    }

    fn apply(&self, input: &str) -> String {
        let mut current = input.to_string();
        for (from, to) in &self.rules {
            if current.contains(from.as_str()) {
                current = current.replace(from.as_str(), to);
            }
        }
        current
    }
}

/// Stable, content-free SHA-256 digest (hex) of the rule artifact bytes.
///
/// Unlike the standard-library hasher, SHA-256 is algorithm-stable across Rust
/// versions and hosts, so the digest can identify a specific rule artifact for
/// diagnostics and future persistent-cache keys. It never carries Skill content.
fn digest_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    let mut hex = String::with_capacity(out.len() * 2);
    for byte in out {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn stage_from(yaml: &str, target: OsTarget) -> Result<OsAdapterStage, OsAdapterError> {
        OsAdapterStage::from_bytes(yaml.as_bytes(), target, Path::new("test-rules.yaml"))
    }

    const SAMPLE: &str = r#"
- ubuntu: "sudo apt-get install -y "
  alinux: "sudo dnf install -y "
  direction: bidirectional
  auto_apply: always
  confidence: high
- ubuntu: "libssl-dev"
  alinux: "openssl-devel"
  direction: bidirectional
  auto_apply: always
- ubuntu: "apache2.service"
  alinux: "httpd.service"
  direction: bidirectional
  auto_apply: always
- ubuntu: "/etc/apt/sources.list.d/"
  alinux: "/etc/yum.repos.d/"
  direction: bidirectional
  auto_apply: always
- ubuntu: "build-essential"
  alinux: '@"Development Tools"'
  direction: ubuntu_to_alinux_only
  auto_apply: always
- ubuntu: "ufw"
  alinux: "firewalld"
  direction: ubuntu_to_alinux_only
  auto_apply: never
"#;

    #[test]
    fn ubuntu_to_alinux_converts_representative_rules() {
        let stage = stage_from(SAMPLE, OsTarget::Alinux).unwrap();
        let input = "Run `sudo apt-get install -y libssl-dev` then enable apache2.service.\n\
                     Config lives in /etc/apt/sources.list.d/. Install build-essential.\n";
        let out = stage.apply(input);
        assert!(out.contains("sudo dnf install -y "));
        assert!(out.contains("openssl-devel"));
        assert!(out.contains("httpd.service"));
        assert!(out.contains("/etc/yum.repos.d/"));
        assert!(out.contains("@\"Development Tools\""));
        assert!(!out.contains("apt-get install"));
        assert!(!out.contains("libssl-dev"));
    }

    #[test]
    fn alinux_to_ubuntu_reverses_bidirectional_only() {
        let stage = stage_from(SAMPLE, OsTarget::Ubuntu).unwrap();
        let input =
            "sudo dnf install -y openssl-devel; unit httpd.service; @\"Development Tools\"\n";
        let out = stage.apply(input);
        assert!(out.contains("sudo apt-get install -y "));
        assert!(out.contains("libssl-dev"));
        assert!(out.contains("apache2.service"));
        // ubuntu_to_alinux_only rule does NOT reverse: the Alinux literal stays.
        assert!(out.contains("@\"Development Tools\""));
        assert!(!out.contains("build-essential"));
    }

    #[test]
    fn auto_apply_never_is_loaded_but_not_applied() {
        let stage = stage_from(SAMPLE, OsTarget::Alinux).unwrap();
        assert_eq!(stage.apply("enable ufw now"), "enable ufw now");
        assert_eq!(stage.total_rules(), 6);
    }

    #[test]
    fn digest_is_stable_sha256_and_content_free() {
        let a = stage_from(SAMPLE, OsTarget::Alinux).unwrap();
        let b = stage_from(SAMPLE, OsTarget::Alinux).unwrap();
        // Deterministic across builds and independent of the resolved target.
        assert_eq!(a.rule_digest(), b.rule_digest());
        // SHA-256 hex is 64 lowercase hex chars and never embeds rule content.
        assert_eq!(a.rule_digest().len(), 64);
        assert!(a.rule_digest().bytes().all(|c| c.is_ascii_hexdigit()));
        assert!(!a.rule_digest().contains("apt"));

        // Pin the exact SHA-256 of the artifact bytes so a hashing-algorithm
        // regression (e.g. reverting to a non-stable hasher) is caught. The
        // digest is over the raw artifact, independent of the resolved target.
        let one = "- ubuntu: a\n  alinux: b\n  direction: bidirectional\n  auto_apply: always\n";
        let expected: String = Sha256::digest(one.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            stage_from(one, OsTarget::Alinux).unwrap().rule_digest(),
            expected
        );
    }
}
