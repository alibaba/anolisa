//! DistributionIndex: typed view over the artifact registry.
//!
//! ANOLISA component manifests declare *what* a component is; the
//! `DistributionIndex` declares *where* concrete pre-built artifacts live
//! (URL, checksum, signature, backend, os/arch/libc/pkg_base selectors).
//!
//! This module is a pure metadata layer:
//!   * NO network IO,
//!   * NO file download,
//!   * NO signature verification.
//!
//! It only loads TOML and resolves a query to a single matching entry.

use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;

/// Top-level DistributionIndex document.
///
/// This is the in-memory shape used by the resolver. The on-disk TOML uses
/// `[[entries]]` array-of-tables so each entry is self-describing.
///
/// Optional top-level meta fields (`channel`, `generated_at`, `expires_at`,
/// `publisher`, `signature`) are descriptive: they document the index as a
/// whole and may default values per `[[entries]]` rows (today only `channel`
/// participates in resolver matching when explicitly set on a row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionIndex {
    /// On-disk schema version for distribution index parsing.
    pub schema_version: u32,
    /// Default channel for this index. Entries with an explicit
    /// `channel` override take precedence over this default.
    #[serde(default)]
    pub channel: Option<String>,
    /// ISO-8601 timestamp when this index was published.
    #[serde(default)]
    pub generated_at: Option<String>,
    /// ISO-8601 timestamp after which this index should be considered stale.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// Publishing party (e.g. `"anolisa"`, `"internal-mirror"`).
    #[serde(default)]
    pub publisher: Option<String>,
    /// Index-level signature scheme (e.g. `"cosign"`).
    #[serde(default)]
    pub signature: Option<String>,
    /// Concrete artifact rows available to the resolver.
    #[serde(default)]
    pub entries: Vec<DistributionEntry>,
}

/// One concrete artifact binding for a (component, version, channel, target).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DistributionEntry {
    /// Component this artifact installs.
    pub component: String,
    /// Artifact version.
    pub version: String,
    /// Release channel: "stable" | "beta" | "experimental".
    pub channel: String,
    /// Artifact packaging format.
    pub artifact_type: ArtifactType,
    /// Backend hint for the install runner: "rpm" | "deb" | "tar" | "oci" | "file" | ...
    pub backend: String,
    /// Fetch URL. Resolved rows become live downloads during execute.
    ///
    /// Optional when the published artifact follows the raw repository
    /// layout convention. Empty means "derive from the consumer's repo
    /// root"; consumers without such a root must treat empty as a hard
    /// error rather than guessing.
    #[serde(default)]
    pub url: String,
    /// OS selector: "linux" | "darwin" | ...
    pub os: String,
    /// CPU arch selector: "x86_64" | "aarch64" | "any".
    pub arch: String,
    /// libc selector: "glibc" | "musl" | None (any).
    #[serde(default)]
    pub libc: Option<String>,
    /// OS base selector: "anolis23" | "anolis8" | None (any).
    #[serde(default)]
    pub pkg_base: Option<String>,
    /// Allowed install modes: e.g. ["system", "user"].
    #[serde(default)]
    pub install_modes: Vec<String>,
    /// Expected SHA256 for downloaded bytes. Execute refuses missing checksums
    /// for installable artifacts.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Inline signature metadata, when an index carries it directly.
    #[serde(default)]
    pub signature: Option<String>,
    /// Stable artifact identifier (e.g. `"agentsight-0.5.0-alinux4-x86_64-rpm"`).
    #[serde(default)]
    pub artifact_id: Option<String>,
    /// Digest of the component manifest this artifact was built from.
    #[serde(default)]
    pub manifest_digest: Option<String>,
    /// Artifact size in bytes (purely informational).
    #[serde(default)]
    pub size: Option<u64>,
    /// External signature URL (e.g. `*.sig` companion file).
    #[serde(default)]
    pub signature_url: Option<String>,
    /// OS version constraint (e.g. `">=4"`, `"22.04"`).
    #[serde(default)]
    pub os_version: Option<String>,
    /// Sibling components this artifact depends on (by component name).
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// Supported on-the-wire artifact types.
///
/// Wire form is snake_case (`rpm`, `deb`, `tar_gz`, `zip`, `oci`, `file`,
/// `binary`). The custom `Deserialize` impl is lenient: it accepts the
/// legacy spellings `tar.gz` and `tar` and normalizes them to
/// [`ArtifactType::TarGz`].
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    /// RPM package artifact.
    Rpm,
    /// Debian package artifact.
    Deb,
    /// Gzipped tar archive artifact.
    TarGz,
    /// Zip archive artifact.
    Zip,
    /// OCI image artifact.
    Oci,
    /// Raw file artifact.
    File,
    /// Single executable/binary artifact.
    Binary,
}

impl<'de> Deserialize<'de> for ArtifactType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "rpm" => Ok(Self::Rpm),
            "deb" => Ok(Self::Deb),
            // Accept `tar_gz`, `tar.gz`, and the legacy `tar` spelling.
            "tar_gz" | "tar.gz" | "tar" => Ok(Self::TarGz),
            "zip" => Ok(Self::Zip),
            "oci" => Ok(Self::Oci),
            "file" => Ok(Self::File),
            "binary" => Ok(Self::Binary),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["rpm", "deb", "tar_gz", "zip", "oci", "file", "binary"],
            )),
        }
    }
}

/// Resolver query. Borrowed so callers can build it without allocating.
#[derive(Debug, Clone)]
pub struct ResolveQuery<'a> {
    /// Component name to resolve.
    pub component: &'a str,
    /// None => pick highest version in the channel.
    pub version: Option<&'a str>,
    /// None => "stable".
    pub channel: Option<&'a str>,
    /// Requested install mode.
    pub install_mode: &'a str,
    /// Target operating system.
    pub os: &'a str,
    /// Target CPU architecture.
    pub arch: &'a str,
    /// Target libc, when known.
    pub libc: Option<&'a str>,
    /// Target OS package base, when known.
    pub pkg_base: Option<&'a str>,
    /// Ordered tiebreaker. When more than one entry survives version
    /// selection, the first listed type that matches *any* candidate is
    /// preferred. An empty slice preserves legacy ambiguity behavior.
    pub preferred_types: &'a [ArtifactType],
}

/// Resolver errors. These are vocabulary errors — IO and parse errors live in
/// `DistributionError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// No row matched the requested target tuple.
    #[error("no distribution entry matches the query")]
    NotFound,
    /// More than one row survived resolver filtering.
    #[error("multiple distribution entries match the query ({} candidates)", .0.len())]
    Ambiguous(Vec<DistributionEntry>),
    /// Rows exist for the component but none support the requested mode.
    #[error("install mode is not supported by any candidate entry")]
    UnsupportedMode,
    /// A matching row is missing checksum metadata.
    #[error("matching entry has no sha256 but checksum was requested")]
    ChecksumMissing,
}

/// IO / parse errors when loading an index.
#[derive(Debug, thiserror::Error)]
pub enum DistributionError {
    /// Index file could not be read.
    #[error("cannot read distribution index '{0}': {1}")]
    Io(String, std::io::Error),
    /// Index TOML could not be parsed.
    #[error("cannot parse distribution index '{0}': {1}")]
    Parse(String, String),
}

impl DistributionIndex {
    /// Load a `DistributionIndex` from a TOML file on disk.
    pub fn load(path: &Path) -> Result<Self, DistributionError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| DistributionError::Io(path.display().to_string(), e))?;
        Self::from_toml_str(&content)
            .map_err(|e| DistributionError::Parse(path.display().to_string(), e))
    }

    /// Parse from a TOML string. Returned error is the raw `toml` message.
    pub fn from_toml_str(s: &str) -> Result<Self, String> {
        toml::from_str(s).map_err(|e| e.to_string())
    }

    /// Serialize to TOML. Useful for tests and tooling.
    pub fn to_toml_string(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| e.to_string())
    }

    /// Resolve a query to a single matching entry.
    ///
    /// Filter rules (in order):
    ///   1. `component` exact match.
    ///   2. `channel` exact match (query default "stable").
    ///   3. `install_mode` must appear in the entry's `install_modes`.
    ///   4. `os` exact match.
    ///   5. `arch` exact match OR entry arch == "any".
    ///   6. `libc` and `pkg_base`: if entry has Some, query must match.
    ///      If entry has None, accepted for any query value.
    ///   7. `version`: if Some, exact match. If None, keep only entries with
    ///      the highest semver version (lexicographic fallback).
    ///   8. Tiebreaker: if `preferred_types` is non-empty and more than one
    ///      candidate remains, the first type in `preferred_types` that
    ///      matches any candidate wins; non-matching entries are dropped.
    ///   9. Exactly one candidate -> Ok; zero -> NotFound; more -> Ambiguous.
    pub fn resolve(&self, q: &ResolveQuery<'_>) -> Result<DistributionEntry, ResolveError> {
        let want_channel = q.channel.unwrap_or("stable");

        // 1-6: filter without considering version.
        let mut candidates: Vec<&DistributionEntry> = self
            .entries
            .iter()
            .filter(|e| e.component == q.component)
            .filter(|e| e.channel == want_channel)
            .filter(|e| e.os == q.os)
            .filter(|e| e.arch == q.arch || e.arch == "any")
            .filter(|e| matches_optional(e.libc.as_deref(), q.libc))
            .filter(|e| matches_optional(e.pkg_base.as_deref(), q.pkg_base))
            .collect();

        if candidates.is_empty() {
            return Err(ResolveError::NotFound);
        }

        // 7a: install_mode filter — track separately so we can distinguish
        // "would have matched but the install mode is wrong" from a generic
        // NotFound.
        let before_mode = candidates.len();
        candidates.retain(|e| e.install_modes.iter().any(|m| m.as_str() == q.install_mode));
        if candidates.is_empty() {
            return if before_mode > 0 {
                Err(ResolveError::UnsupportedMode)
            } else {
                Err(ResolveError::NotFound)
            };
        }

        // 7b: version selection — narrow `candidates` rather than picking
        // eagerly, so the preferred-type tiebreaker can run afterwards.
        match q.version {
            Some(v) => {
                candidates.retain(|e| e.version == v);
                if candidates.is_empty() {
                    return Err(ResolveError::NotFound);
                }
            }
            None => {
                retain_highest_version(&mut candidates);
            }
        }

        // 8: preferred-type tiebreaker. Empty preferences keep legacy
        // behavior — multiple candidates surface as Ambiguous below.
        if candidates.len() > 1 && !q.preferred_types.is_empty() {
            for preferred in q.preferred_types {
                if candidates.iter().any(|e| e.artifact_type == *preferred) {
                    candidates.retain(|e| e.artifact_type == *preferred);
                    break;
                }
            }
        }

        // 9: final cardinality check.
        match candidates.len() {
            0 => Err(ResolveError::NotFound),
            1 => Ok(candidates[0].clone()),
            _ => Err(ResolveError::Ambiguous(
                candidates.into_iter().cloned().collect(),
            )),
        }
    }

    /// All distinct versions that pass the same component / channel / os / arch
    /// / libc / pkg_base / install_mode filters as [`resolve`](Self::resolve),
    /// highest-first by semver (lexicographic fallback). Ignores `q.version`
    /// and the preferred-type tiebreaker — it answers "what could this query
    /// resolve to", for dry-run previews that must agree with `resolve`.
    pub fn matching_versions(&self, q: &ResolveQuery<'_>) -> Vec<String> {
        let want_channel = q.channel.unwrap_or("stable");
        let mut versions: Vec<String> = self
            .entries
            .iter()
            .filter(|e| e.component == q.component)
            .filter(|e| e.channel == want_channel)
            .filter(|e| e.os == q.os)
            .filter(|e| e.arch == q.arch || e.arch == "any")
            .filter(|e| matches_optional(e.libc.as_deref(), q.libc))
            .filter(|e| matches_optional(e.pkg_base.as_deref(), q.pkg_base))
            .filter(|e| e.install_modes.iter().any(|m| m.as_str() == q.install_mode))
            .map(|e| e.version.clone())
            .collect();
        // Highest-first so the head of the list is the version `resolve` picks.
        versions.sort_by(|a, b| match (Version::parse(a), Version::parse(b)) {
            (Ok(va), Ok(vb)) => vb.cmp(&va),
            _ => b.cmp(a),
        });
        versions.dedup();
        versions
    }
}

/// Optional selector match: entry None => wildcard accept; entry Some =>
/// query must be Some and equal.
fn matches_optional(entry_val: Option<&str>, query_val: Option<&str>) -> bool {
    match entry_val {
        None => true,
        Some(ev) => query_val.is_some_and(|qv| qv == ev),
    }
}

/// Narrow `candidates` to the entries that share the highest version. Uses
/// semver when every candidate version parses; otherwise falls back to
/// lexicographic comparison. `candidates` is mutated in place and is
/// guaranteed non-empty on input.
fn retain_highest_version(candidates: &mut Vec<&DistributionEntry>) {
    if candidates.len() <= 1 {
        return;
    }

    let parsed: Option<Vec<Version>> = candidates
        .iter()
        .map(|e| Version::parse(&e.version).ok())
        .collect();

    if let Some(versions) = parsed {
        let mut best = versions[0].clone();
        for v in versions.iter().skip(1) {
            if *v > best {
                best = v.clone();
            }
        }
        let best_str = best.to_string();
        candidates.retain(|e| e.version == best_str);
    } else {
        let best = candidates
            .iter()
            .map(|e| e.version.clone())
            .max()
            .unwrap_or_default();
        candidates.retain(|e| e.version == best);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn sample_entry() -> DistributionEntry {
        DistributionEntry {
            component: "agentsight".into(),
            version: "0.1.0".into(),
            channel: "stable".into(),
            artifact_type: ArtifactType::Rpm,
            backend: "rpm".into(),
            url: "https://example.invalid/agentsight-0.1.0.rpm".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            libc: Some("glibc".into()),
            pkg_base: Some("anolis23".into()),
            install_modes: vec!["system".into()],
            sha256: Some("0".repeat(64)),
            signature: None,
            artifact_id: None,
            manifest_digest: None,
            size: None,
            signature_url: None,
            os_version: None,
            dependencies: vec!["kernel-headers".into()],
        }
    }

    fn linux_x86_query<'a>(component: &'a str, mode: &'a str) -> ResolveQuery<'a> {
        ResolveQuery {
            component,
            version: None,
            channel: None,
            install_mode: mode,
            os: "linux",
            arch: "x86_64",
            libc: Some("glibc"),
            pkg_base: Some("anolis23"),
            preferred_types: &[],
        }
    }

    #[test]
    fn toml_roundtrip_preserves_entries() {
        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: vec![sample_entry()],
        };

        let serialized = index.to_toml_string().expect("serialize");
        let parsed: DistributionIndex =
            DistributionIndex::from_toml_str(&serialized).expect("deserialize");

        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0], index.entries[0]);
    }

    /// The bundled distribution-index may ship reviewed release entries,
    /// but it must not contain template placeholders. `example.invalid`
    /// rows became a footgun once `download::Download` graduated to
    /// HTTP(S), because a resolved row becomes a real fetch on execute.
    ///
    /// This test pins the current dev-tree contract: `cosh` resolves from
    /// the built-in index, while unreleased template URLs remain excluded.
    #[test]
    fn bundled_distribution_index_contains_only_reviewed_entries() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../manifests/distribution-index/index.toml");
        let index = DistributionIndex::load(&fixture).expect("load fixture");
        assert!(
            index
                .entries
                .iter()
                .all(|entry| !entry.url.contains("example.invalid")),
            "bundled distribution-index must not ship template placeholder URLs",
        );

        let q = linux_x86_query("cosh", "system");
        let resolved = index.resolve(&q).expect("cosh entry resolves");
        assert_eq!(resolved.component, "cosh");
        assert_eq!(resolved.artifact_type, ArtifactType::TarGz);
        assert!(
            resolved
                .sha256
                .as_deref()
                .is_some_and(|sha| sha.len() == 64),
            "bundled release entries must carry concrete sha256 values",
        );
    }

    #[test]
    fn resolve_wrong_arch_returns_not_found() {
        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: vec![sample_entry()],
        };
        let mut q = linux_x86_query("agentsight", "system");
        q.arch = "aarch64";

        assert_eq!(index.resolve(&q), Err(ResolveError::NotFound));
    }

    #[test]
    fn resolve_without_version_picks_highest_semver() {
        let mut newer = sample_entry();
        newer.version = "0.2.0".into();
        newer.url = "https://example.invalid/agentsight-0.2.0.rpm".into();

        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: vec![sample_entry(), newer.clone()],
        };

        let q = linux_x86_query("agentsight", "system");
        let entry = index.resolve(&q).expect("resolve");
        assert_eq!(entry.version, "0.2.0");
        assert_eq!(entry.url, newer.url);
    }

    #[test]
    fn resolve_ambiguous_when_two_entries_share_version_query() {
        // Two entries with the same component/channel/os/arch/version but
        // differing libc=None (wildcard) — both match a query with libc=Some.
        let a = sample_entry();
        let mut b = sample_entry();
        b.libc = None;
        b.url = "https://example.invalid/agentsight-0.1.0.alt.rpm".into();

        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: vec![a, b],
        };

        let mut q = linux_x86_query("agentsight", "system");
        q.version = Some("0.1.0");

        match index.resolve(&q) {
            Err(ResolveError::Ambiguous(list)) => assert_eq!(list.len(), 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn resolve_unsupported_mode_distinguishes_from_not_found() {
        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: vec![sample_entry()],
        };
        let q = linux_x86_query("agentsight", "user");
        assert_eq!(index.resolve(&q), Err(ResolveError::UnsupportedMode));
    }

    #[test]
    fn load_from_tempfile_roundtrips() {
        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: vec![sample_entry()],
        };
        let toml_str = index.to_toml_string().expect("serialize");

        let mut tmp = NamedTempFile::new().expect("tempfile");
        tmp.write_all(toml_str.as_bytes()).expect("write");
        let loaded = DistributionIndex::load(tmp.path()).expect("load");

        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0], index.entries[0]);
    }

    #[test]
    fn template_distribution_index_loads_with_expected_entries() {
        let template = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/distribution-index.toml");
        let index = DistributionIndex::load(&template).expect("load template");

        assert_eq!(index.schema_version, 1);
        assert_eq!(index.channel.as_deref(), Some("stable"));
        assert_eq!(index.publisher.as_deref(), Some("anolisa"));
        assert_eq!(index.signature.as_deref(), Some("cosign"));
        assert_eq!(
            index.entries.len(),
            3,
            "template should ship 3 example entries"
        );
        // All template entries should belong to the agentsight component.
        assert!(index.entries.iter().all(|e| e.component == "agentsight"));
    }

    fn rpm_and_targz_entries() -> Vec<DistributionEntry> {
        let rpm = DistributionEntry {
            component: "agentsight".into(),
            version: "0.1.0".into(),
            channel: "stable".into(),
            artifact_type: ArtifactType::Rpm,
            backend: "rpm".into(),
            url: "https://example.invalid/agentsight-0.1.0.rpm".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            libc: Some("glibc".into()),
            pkg_base: Some("anolis23".into()),
            install_modes: vec!["system".into()],
            sha256: Some("0".repeat(64)),
            signature: None,
            artifact_id: None,
            manifest_digest: None,
            size: None,
            signature_url: None,
            os_version: None,
            dependencies: vec![],
        };
        let mut tar = rpm.clone();
        tar.artifact_type = ArtifactType::TarGz;
        tar.backend = "tar".into();
        tar.url = "https://example.invalid/agentsight-0.1.0.tar.gz".into();
        vec![rpm, tar]
    }

    #[test]
    fn resolve_preferred_types_prefers_rpm_when_listed_first() {
        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: rpm_and_targz_entries(),
        };
        let prefs = [ArtifactType::Rpm, ArtifactType::TarGz];
        let mut q = linux_x86_query("agentsight", "system");
        q.version = Some("0.1.0");
        q.preferred_types = &prefs;

        let entry = index.resolve(&q).expect("resolve");
        assert_eq!(entry.artifact_type, ArtifactType::Rpm);
    }

    #[test]
    fn resolve_preferred_types_prefers_tar_gz_when_listed_first() {
        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: rpm_and_targz_entries(),
        };
        let prefs = [ArtifactType::TarGz, ArtifactType::Rpm];
        let mut q = linux_x86_query("agentsight", "system");
        q.version = Some("0.1.0");
        q.preferred_types = &prefs;

        let entry = index.resolve(&q).expect("resolve");
        assert_eq!(entry.artifact_type, ArtifactType::TarGz);
    }

    #[test]
    fn resolve_empty_preferred_types_keeps_ambiguous() {
        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries: rpm_and_targz_entries(),
        };
        let mut q = linux_x86_query("agentsight", "system");
        q.version = Some("0.1.0");

        match index.resolve(&q) {
            Err(ResolveError::Ambiguous(list)) => assert_eq!(list.len(), 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn resolve_picks_highest_version_then_preferred() {
        // Two versions (0.1.0 and 0.2.0), each with rpm + tar_gz. With no
        // explicit version, the resolver must first narrow to 0.2.0, then
        // apply preferred_types.
        let mut entries = rpm_and_targz_entries();
        for e in rpm_and_targz_entries() {
            let mut newer = e;
            newer.version = "0.2.0".into();
            newer.url = newer.url.replace("0.1.0", "0.2.0");
            entries.push(newer);
        }

        let index = DistributionIndex {
            schema_version: 1,
            channel: None,
            generated_at: None,
            expires_at: None,
            publisher: None,
            signature: None,
            entries,
        };

        let prefs = [ArtifactType::TarGz, ArtifactType::Rpm];
        let mut q = linux_x86_query("agentsight", "system");
        q.preferred_types = &prefs;

        let entry = index.resolve(&q).expect("resolve");
        assert_eq!(entry.version, "0.2.0");
        assert_eq!(entry.artifact_type, ArtifactType::TarGz);
    }

    #[test]
    fn artifact_type_deserialize_accepts_legacy_spellings() {
        // `tar.gz` and `tar` must both normalize to TarGz.
        let toml_str = r#"
            schema_version = 1
            [[entries]]
            component = "x"
            version = "0.1.0"
            channel = "stable"
            artifact_type = "tar.gz"
            backend = "tar"
            url = "https://example.invalid/x.tar.gz"
            os = "linux"
            arch = "x86_64"
            install_modes = ["user"]
        "#;
        let index = DistributionIndex::from_toml_str(toml_str).expect("parse");
        assert_eq!(index.entries[0].artifact_type, ArtifactType::TarGz);
    }

    #[test]
    fn entry_without_url_parses_as_empty_for_convention_layout() {
        let toml_str = r#"
            schema_version = 1
            [[entries]]
            component = "tokenless"
            version = "0.5.0"
            channel = "stable"
            artifact_type = "tar_gz"
            backend = "raw"
            os = "linux"
            arch = "x86_64"
            install_modes = ["system"]
        "#;
        let index = DistributionIndex::from_toml_str(toml_str).expect("parse");
        assert_eq!(index.entries[0].url, "");
    }
}
