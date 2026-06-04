//! Execution policy: the CLI scope-gate for `anolisa enable`.
//!
//! Background
//! ----------
//! Up through P1-G1 the `enable` handler carried a single hard-coded
//! `SUPPORTED_CAPABILITY = "agent-observability"` constant. That gate had
//! to be removed so a second capability (`token-optimization`) could be
//! validated against the same general-purpose planner / executor, while
//! still keeping every other capability behind an explicit
//! `NOT_IMPLEMENTED` boundary until each one is reviewed end-to-end.
//!
//! The replacement is a small declarative TOML file
//! (`templates/execution-policy.toml`) parsed at CLI start-up. The policy
//! lives in `anolisa-cli` rather than `anolisa-core` because the underlying
//! `plan_enable` / `execute_enable` libraries are deliberately
//! general-purpose — they do not need to know which capabilities the *CLI*
//! has graduated to "real" execution. Keeping the gate at the CLI boundary
//! also matches where the existing scope guards (`--feature`,
//! `--with-adapter`, `--from-source`) already live.
//!
//! Schema (v1)
//! -----------
//! ```toml
//! schema_version = 1
//!
//! [[capabilities]]
//! name = "agent-observability"
//! enabled = true            # optional, defaults to true
//! allow_execute = true      # required: dry-run + real-execute allowed when true
//! supported_backends = ["binary", "tar_gz"]
//! notes = "..."             # optional, free-form
//! ```
//!
//! Lookup is by exact capability name. Missing capability → treated as
//! "policy disallows" → caller surfaces `NOT_IMPLEMENTED`.
//!
//! Discovery
//! ---------
//! The loader probes three sources in order so the same binary works in
//! every distribution form:
//!
//!   1. **Packaged** — `<datadir>/templates/execution-policy.toml` (e.g.
//!      `/usr/local/share/anolisa/templates/execution-policy.toml` for a
//!      system install). This is the authoritative location for a
//!      packaged build and lets distros patch the policy without
//!      rebuilding the binary.
//!   2. **Dev-tree** — `<crate>/../../templates/execution-policy.toml`,
//!      resolved via `CARGO_MANIFEST_DIR`. This is what `cargo run` /
//!      `cargo test` see when invoked from inside the workspace.
//!   3. **Embedded** — the same template file is baked into the binary
//!      with `include_str!`. This guarantees a standalone copy of
//!      `anolisa` works even when neither the packaged datadir nor the
//!      dev-tree path is reachable (the canonical case being
//!      `cargo install --path` of a release build run outside the
//!      source tree). Bumping the embedded copy requires a rebuild —
//!      that is the intended trade-off: a binary always ships with a
//!      known-good baseline policy.
//!
//! [`PolicyError::NotFound`] is therefore only reachable in synthetic
//! tests that disable every source. Production binaries always succeed
//! at step 3 if 1 and 2 are absent.

use std::path::{Path, PathBuf};

use anolisa_platform::fs_layout::FsLayout;
use serde::Deserialize;

use crate::packaged;

/// Subdirectory under `datadir` where the packaged execution policy
/// lives. Matches the layout shipped by the install scripts.
const POLICY_SUBDIR: &str = "templates";
/// Filename of the execution policy.
const POLICY_FILE: &str = "execution-policy.toml";

/// Baseline policy baked into the binary. Acts as the final fallback
/// when neither a packaged copy nor a dev-tree copy is reachable —
/// e.g. a `cargo install --path` build run from outside the source
/// tree. Bumping requires a rebuild on purpose: every shipped binary
/// must boot with a known-good gate even on a stripped-down host.
const EMBEDDED_POLICY: &str = include_str!("../../../templates/execution-policy.toml");

/// Wire-format schema version. Bump together with [`ExecutionPolicy`] when
/// the on-disk shape changes in a way consumers must observe.
pub const EXECUTION_POLICY_SCHEMA_VERSION: u32 = 1;

/// Errors surfaced while loading or parsing the execution policy file.
///
/// All variants are mapped at the CLI boundary into `CliError` (today as
/// `EXECUTION_FAILED` — the policy file is an internal asset, so a load
/// failure is a "the machine refused" error rather than caller-fixable
/// input).
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// No policy file was found at any candidate path AND the embedded
    /// copy is missing. This is effectively unreachable in production
    /// builds — `include_str!` guarantees an embedded copy ships with
    /// every binary — but is kept as a variant for synthetic tests
    /// that disable all three sources.
    #[error("execution policy not found (searched packaged datadir, dev-tree, and embedded copy)")]
    NotFound,

    /// Disk read failed (e.g. permission denied).
    #[error("failed to read execution policy at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// TOML parse failed.
    #[error("failed to parse execution policy at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// Schema version on disk does not match what this binary understands.
    /// Surfaced eagerly so downstream lookups never operate on a
    /// mis-deserialized shape.
    #[error("unsupported execution policy schema_version {actual} (expected {expected})")]
    UnsupportedSchema { actual: u32, expected: u32 },
}

/// Parsed execution policy.
///
/// `capabilities` ordering follows the file; lookup is O(n) by name. For
/// the current handful of entries that is fine and keeps the loader
/// allocation-free beyond the `Vec`.
#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionPolicy {
    pub schema_version: u32,
    #[serde(default)]
    pub capabilities: Vec<CapabilityPolicy>,
}

/// Per-capability policy entry. See module docs for field semantics.
///
/// `supported_backends` and `notes` are deserialized for forward
/// compatibility (the policy file is the single declarative record of what
/// is executable end-to-end) but the scope gate currently only consults
/// `enabled` and `allow_execute`. `#[allow(dead_code)]` keeps the schema
/// fields without tripping the dead-code lint in non-test builds; the
/// fields are exposed to tests and to future code that surfaces backend /
/// notes in diagnostics.
#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityPolicy {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allow_execute: bool,
    #[serde(default)]
    #[allow(dead_code)]
    pub supported_backends: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub notes: Option<String>,
}

fn default_true() -> bool {
    true
}

impl ExecutionPolicy {
    /// Load the execution policy. Tries packaged datadir → dev-tree →
    /// embedded baseline, in that order. See module docs for the
    /// rationale.
    pub fn load() -> Result<Self, PolicyError> {
        Self::load_with_sources(PolicySources::default())
    }

    /// Same as [`Self::load`] but lets tests disable individual sources
    /// to pin the fallback order without depending on the host
    /// environment.
    pub(crate) fn load_with_sources(sources: PolicySources) -> Result<Self, PolicyError> {
        if let Some(path) = sources.packaged.as_deref()
            && path.is_file()
        {
            return Self::from_path(path);
        }
        if let Some(path) = sources.dev_tree.as_deref()
            && path.is_file()
        {
            return Self::from_path(path);
        }
        if let Some(embedded) = sources.embedded {
            return Self::parse_with_path(embedded, Path::new("<embedded>"));
        }
        Err(PolicyError::NotFound)
    }

    /// Load the execution policy from an explicit path. Test-friendly hook.
    pub fn from_path(path: &Path) -> Result<Self, PolicyError> {
        let bytes = std::fs::read_to_string(path).map_err(|source| PolicyError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse_with_path(&bytes, path)
    }

    /// Parse a TOML body. Validates the schema version eagerly.
    ///
    /// Used by unit tests today; kept as a public API surface so future
    /// callers (e.g. CLI integration tests, embedded smoke harnesses) can
    /// exercise the parser without going through the disk.
    #[allow(dead_code)]
    pub fn from_toml_str(s: &str) -> Result<Self, PolicyError> {
        Self::parse_with_path(s, Path::new("<memory>"))
    }

    fn parse_with_path(s: &str, path: &Path) -> Result<Self, PolicyError> {
        let parsed: ExecutionPolicy = toml::from_str(s).map_err(|source| PolicyError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        if parsed.schema_version != EXECUTION_POLICY_SCHEMA_VERSION {
            return Err(PolicyError::UnsupportedSchema {
                actual: parsed.schema_version,
                expected: EXECUTION_POLICY_SCHEMA_VERSION,
            });
        }
        Ok(parsed)
    }

    /// Look up a capability entry by exact name. Returns `None` when the
    /// capability is absent from the policy (caller treats as "not
    /// allowed"). Disabled entries (`enabled = false`) are returned as-is
    /// so callers can render diagnostic detail; use [`Self::allows_execute`]
    /// when the caller only needs the gate decision.
    pub fn lookup(&self, capability: &str) -> Option<&CapabilityPolicy> {
        self.capabilities.iter().find(|c| c.name == capability)
    }

    /// Convenience: returns `true` iff the capability is present, the entry
    /// is enabled, and `allow_execute = true`. Anything else is the closed
    /// `NOT_IMPLEMENTED` state.
    pub fn allows_execute(&self, capability: &str) -> bool {
        match self.lookup(capability) {
            Some(p) => p.enabled && p.allow_execute,
            None => false,
        }
    }
}

/// Bundle of policy source candidates. Default carries the production
/// discovery chain (packaged + dev-tree + embedded); tests construct
/// trimmed variants to pin individual fallback steps.
pub(crate) struct PolicySources {
    /// Packaged path under the active layout's `datadir`. `None` when
    /// no layout is reachable.
    pub packaged: Option<PathBuf>,
    /// Dev-tree path resolved from `CARGO_MANIFEST_DIR`.
    pub dev_tree: Option<PathBuf>,
    /// Embedded TOML body. `None` only in synthetic tests that want to
    /// prove [`PolicyError::NotFound`] is reachable.
    pub embedded: Option<&'static str>,
}

impl Default for PolicySources {
    fn default() -> Self {
        // We don't know which install mode the binary was invoked
        // under at this point — `enable` resolves that later. Use the
        // shared packaged-datadir probe (`ANOLISA_DATA_DIR` →
        // binary-location → system FHS default) so the policy file is
        // discoverable regardless of whether the binary was installed
        // to `/usr/local/`, a user prefix, or a smoke-test tmpdir.
        // Fall back to the system FHS path so the lookup still
        // produces a candidate when none of the probes find an
        // existing dir (the `is_file()` check inside
        // `load_with_sources` then misses and we fall through to the
        // dev-tree / embedded copy).
        let system_layout = FsLayout::system(None);
        let packaged_root = packaged::packaged_datadir_root(&system_layout)
            .unwrap_or_else(|| system_layout.datadir.clone());
        let packaged = Some(packaged_root.join(POLICY_SUBDIR).join(POLICY_FILE));
        let dev_tree = Some(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join(POLICY_SUBDIR)
                .join(POLICY_FILE),
        );
        Self {
            packaged,
            dev_tree,
            embedded: Some(EMBEDDED_POLICY),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Happy path: the bundled dev-tree template parses, has the expected
    /// schema, and graduates exactly the two capabilities P1-H scoped in.
    /// Both flips here would break the CLI scope-gate, so we want a loud
    /// failure if either drifts.
    #[test]
    fn dev_tree_policy_parses_and_graduates_two_capabilities() {
        let policy = ExecutionPolicy::load().expect("dev-tree policy must load");
        assert_eq!(policy.schema_version, EXECUTION_POLICY_SCHEMA_VERSION);
        assert!(
            policy.allows_execute("agent-observability"),
            "agent-observability must remain executable",
        );
        assert!(
            policy.allows_execute("token-optimization"),
            "token-optimization must be executable per P1-H scope",
        );
    }

    /// Capabilities absent from the policy file must be treated as the
    /// closed-gate default — even if they live in the catalog.
    #[test]
    fn unknown_capability_is_not_allowed() {
        let policy = ExecutionPolicy::load().expect("policy");
        assert!(!policy.allows_execute("agent-memory"));
        assert!(!policy.allows_execute("definitely-not-a-capability"));
    }

    /// `allow_execute = false` closes the gate even when the entry exists.
    /// We construct an in-memory policy so this test does not depend on the
    /// dev-tree file.
    #[test]
    fn allow_execute_false_closes_the_gate() {
        let toml = r#"schema_version = 1

[[capabilities]]
name = "agent-observability"
allow_execute = true

[[capabilities]]
name = "token-optimization"
allow_execute = false
notes = "scoped out for this build"
"#;
        let policy = ExecutionPolicy::from_toml_str(toml).expect("parse");
        assert!(policy.allows_execute("agent-observability"));
        assert!(!policy.allows_execute("token-optimization"));
        // Lookup still returns the entry so callers can show diagnostics.
        let entry = policy
            .lookup("token-optimization")
            .expect("entry present even when allow_execute is false");
        assert!(!entry.allow_execute);
        assert_eq!(entry.notes.as_deref(), Some("scoped out for this build"));
    }

    /// `enabled = false` must also close the gate. We default `enabled` to
    /// `true` in the deserialize hook so this only takes effect when set
    /// explicitly — which is the documented behaviour.
    #[test]
    fn enabled_false_closes_the_gate_even_when_allow_execute_true() {
        let toml = r#"schema_version = 1

[[capabilities]]
name = "agent-observability"
enabled = false
allow_execute = true
"#;
        let policy = ExecutionPolicy::from_toml_str(toml).expect("parse");
        assert!(!policy.allows_execute("agent-observability"));
    }

    /// Schema-version mismatch must fail loudly so we never deserialize a
    /// future shape with the wrong field set.
    #[test]
    fn schema_version_mismatch_is_rejected() {
        let toml = r#"schema_version = 999

[[capabilities]]
name = "agent-observability"
allow_execute = true
"#;
        let err = ExecutionPolicy::from_toml_str(toml).expect_err("must reject");
        match err {
            PolicyError::UnsupportedSchema { actual, expected } => {
                assert_eq!(actual, 999);
                assert_eq!(expected, EXECUTION_POLICY_SCHEMA_VERSION);
            }
            other => panic!("expected UnsupportedSchema, got {other:?}"),
        }
    }

    /// Production path #3: when neither the packaged datadir nor the
    /// dev-tree file is reachable, the embedded baseline (compiled in
    /// via `include_str!`) must still produce a working policy. This
    /// is the contract that makes `cargo install --path` of a
    /// release binary usable outside the source tree — without it,
    /// `enable` would surface `EXECUTION_FAILED` on the first invocation.
    #[test]
    fn load_falls_back_to_embedded_policy_when_disk_sources_missing() {
        let tmp = tempfile::tempdir().expect("tmp");
        // Point both disk sources at paths inside an empty tempdir so
        // we know they cannot resolve, regardless of the host's actual
        // datadir / dev-tree layout.
        let sources = PolicySources {
            packaged: Some(tmp.path().join("does-not-exist-packaged.toml")),
            dev_tree: Some(tmp.path().join("does-not-exist-dev.toml")),
            embedded: Some(EMBEDDED_POLICY),
        };
        let policy = ExecutionPolicy::load_with_sources(sources)
            .expect("embedded fallback must produce a working policy");
        assert_eq!(policy.schema_version, EXECUTION_POLICY_SCHEMA_VERSION);
        // Embedded copy mirrors the shipped template, so the same
        // graduated capabilities must be present.
        assert!(
            policy.allows_execute("agent-observability"),
            "embedded policy must keep agent-observability gated open",
        );
        assert!(
            policy.allows_execute("token-optimization"),
            "embedded policy must keep token-optimization gated open",
        );
    }

    /// Defensive: if a test deliberately disables every source (no
    /// packaged file, no dev-tree file, no embedded body), the loader
    /// must surface `NotFound` instead of e.g. panicking on the
    /// embedded `include_str!`. Pins that the error branch is wired.
    #[test]
    fn load_with_no_sources_returns_not_found() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sources = PolicySources {
            packaged: Some(tmp.path().join("nope.toml")),
            dev_tree: Some(tmp.path().join("also-nope.toml")),
            embedded: None,
        };
        let err = ExecutionPolicy::load_with_sources(sources).expect_err("must error");
        assert!(matches!(err, PolicyError::NotFound), "got: {err:?}");
    }

    /// Loading from an explicit path mirrors the [`Self::load`] entry point
    /// and is used by tests that want isolation from the dev-tree file.
    #[test]
    fn from_path_round_trips_through_tempfile() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("policy.toml");
        std::fs::write(
            &path,
            r#"schema_version = 1

[[capabilities]]
name = "agent-observability"
allow_execute = true
"#,
        )
        .expect("write");
        let policy = ExecutionPolicy::from_path(&path).expect("load");
        assert!(policy.allows_execute("agent-observability"));
        assert!(!policy.allows_execute("token-optimization"));
    }
}
