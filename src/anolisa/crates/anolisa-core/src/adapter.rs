//! Adapter detection and layout placeholder expansion.
//!
//! This module provides two core concerns for adapter management:
//!
//! 1. **Framework detection** — inspects the `detect` hints from an
//!    [`AdapterSpec`] to determine whether a framework (binary on PATH,
//!    well-known paths on disk) is present on the host.
//!
//! 2. **Placeholder expansion** — resolves layout placeholders such as
//!    `{datadir}`, `{bindir}`, and `{etc_dir}` in adapter `dest`/`source`
//!    paths against a concrete [`FsLayout`].
//!
//! All detection logic is side-effect-free: it never spawns processes or
//! writes to the filesystem.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anolisa_platform::fs_layout::FsLayout;

use crate::manifest::AdapterSpec;

pub mod claim;
pub mod contract;
pub mod driver;
pub mod hermes;
pub mod manager;
pub mod openclaw;
pub mod registry;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors produced by adapter operations.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// A layout placeholder in a template string is not recognized.
    #[error("unknown placeholder '{placeholder}' in template \"{template}\"")]
    UnknownPlaceholder {
        /// The unrecognized placeholder (without braces).
        placeholder: String,
        /// The full template string that contained it.
        template: String,
    },

    /// No driver is registered for the requested framework.
    #[error("no built-in driver for framework '{framework}'")]
    UnknownFramework {
        /// Framework name with no registered driver.
        framework: String,
    },

    /// The caller omitted the framework and the component ships adapters
    /// for more than one, so the choice is ambiguous.
    #[error(
        "component '{component}' has adapters for multiple frameworks ({}); specify one",
        .frameworks.join(", ")
    )]
    AmbiguousFramework {
        /// Component the caller asked about.
        component: String,
        /// Candidate frameworks discovered for it.
        frameworks: Vec<String>,
    },

    /// `enable` requires the framework to be usable on the host, but
    /// detection failed (e.g. the framework CLI is not installed).
    #[error("framework '{framework}' not detected: {reason}")]
    FrameworkNotDetected {
        /// Framework that could not be detected.
        framework: String,
        /// Human-readable detection failure reason.
        reason: String,
    },

    /// The component is not installed in any visible (user or system)
    /// state, so its adapters cannot be enabled.
    #[error("component '{component}' is not installed")]
    ComponentNotInstalled {
        /// Component the caller asked to enable.
        component: String,
    },

    /// The installed component manifest does not declare the requested
    /// framework adapter.
    #[error("component '{component}' does not declare an adapter for framework '{framework}'")]
    AdapterNotDeclared {
        /// Component the caller asked to enable.
        component: String,
        /// Framework absent from the installed component manifest.
        framework: String,
    },

    /// The manifest declares an `adapter_type` that is not yet supported
    /// by any built-in driver (e.g. `skill_bundle`, `extension`). Only
    /// `plugin` (or absent, defaulting to plugin) is implemented.
    #[error(
        "adapter type '{adapter_type}' for {component}/{framework} is not supported; only 'plugin' is implemented"
    )]
    UnsupportedAdapterType {
        /// Component whose adapter was requested.
        component: String,
        /// Framework the adapter targets.
        framework: String,
        /// The unsupported `adapter_type` value from the manifest.
        adapter_type: String,
    },

    /// A skill name or config key from the manifest failed validation
    /// (empty, contains path traversal, or has unsafe characters).
    #[error("invalid adapter input for {component}/{framework}: {reason}")]
    InvalidAdapterInput {
        /// Component whose manifest declared the invalid input.
        component: String,
        /// Framework the adapter targets.
        framework: String,
        /// What was wrong.
        reason: String,
    },

    /// The installed component manifest required by adapter enable is
    /// missing, unreadable, or inconsistent with state.
    #[error("invalid installed component manifest for '{component}' at {path}: {reason}")]
    AdapterManifest {
        /// Component whose installed manifest was read.
        component: String,
        /// Manifest path that failed.
        path: PathBuf,
        /// Human-readable failure detail.
        reason: String,
    },

    /// No resource directory was found for the component/framework under
    /// any visible `{datadir}/adapters/<component>/<framework>/` root.
    #[error("no adapter resources found for '{component}/{framework}' under any datadir root")]
    ResourceRootNotFound {
        /// Component name.
        component: String,
        /// Framework name.
        framework: String,
    },

    /// The component contract declares a `dest` for the adapter, but the
    /// expanded path does not exist on disk. Unlike [`ResourceRootNotFound`],
    /// this means the contract was explicit — the caller should **not**
    /// silently fall back to convention discovery.
    #[error(
        "adapter resource root from contract does not exist for '{component}/{framework}': {path}"
    )]
    ContractResourceRootNotFound {
        /// Component name.
        component: String,
        /// Framework name.
        framework: String,
        /// Expanded path that was expected to exist.
        path: PathBuf,
    },

    /// The resource bundle is missing required files or is otherwise
    /// unreadable by the driver.
    #[error("invalid adapter bundle at {root}: {reason}")]
    BundleInvalid {
        /// Resource root inspected.
        root: PathBuf,
        /// What was wrong.
        reason: String,
    },

    /// A framework CLI invocation failed to spawn, exited non-zero, or was
    /// killed after the timeout.
    #[error("framework CLI '{program}' failed: {reason}")]
    FrameworkCli {
        /// Program that was invoked.
        program: String,
        /// Failure detail (exit status, spawn error, or timeout).
        reason: String,
    },

    /// A receipt's claim resources failed re-validation.
    #[error("adapter claim validation failed: {0}")]
    ClaimValidation(#[from] claim::ClaimValidationError),

    /// Failed to take the install lock.
    #[error("install lock error: {0}")]
    Lock(#[from] crate::lock::LockError),

    /// Failed to read or write installed state.
    #[error("installed state error: {0}")]
    State(#[from] crate::state::StateError),

    /// Failed to append to the central log.
    #[error("central log error: {0}")]
    Log(#[from] crate::central_log::CentralLogError),

    /// Filesystem error while discovering resources or computing a digest.
    #[error("io error while accessing {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Structured output from [`detect_framework`].
#[derive(Debug, Clone)]
pub struct DetectResult {
    /// Whether the framework was detected on the host.
    pub detected: bool,
    /// Human-readable explanation of the detection outcome.
    pub reason: String,
}

/// Inspect the `detect` hints from an [`AdapterSpec`] and determine whether
/// the target framework is present on the host.
///
/// Detection rules:
///
/// * `binary = "<name>"` — scans `PATH` for the named executable (no process
///   is spawned).
/// * `paths = ["/opt/hermes", ...]` or `paths = "/single/path"` — checks
///   whether **any** listed path exists on the filesystem.
/// * When **both** `binary` and `paths` are present, both conditions must be
///   satisfied (AND logic).
/// * When `detect` is empty, detection is considered successful with a
///   reason explaining that no detection was configured.
pub fn detect_framework(spec: &AdapterSpec) -> DetectResult {
    let detect = &spec.detect;

    if detect.is_empty() {
        return DetectResult {
            detected: true,
            reason: "no detection configured".to_string(),
        };
    }

    let binary_result = detect.get("binary").map(|v| {
        let name = v.as_str().unwrap_or_default();
        if name.is_empty() {
            return (false, "binary detection key present but empty".to_string());
        }
        match find_binary_in_path(name) {
            Some(path) => (
                true,
                format!("binary '{}' found at {}", name, path.display()),
            ),
            None => (false, format!("binary '{name}' not found in PATH")),
        }
    });

    let paths_result = detect.get("paths").map(|v| {
        let paths = extract_string_list(v);
        if paths.is_empty() {
            return (false, "paths detection key present but empty".to_string());
        }
        for p in &paths {
            if Path::new(p).exists() {
                return (true, format!("path '{p}' exists"));
            }
        }
        (
            false,
            format!("none of the paths exist: {}", paths.join(", ")),
        )
    });

    match (binary_result, paths_result) {
        (Some((bin_ok, bin_reason)), Some((paths_ok, paths_reason))) => DetectResult {
            detected: bin_ok && paths_ok,
            reason: format!("{bin_reason}; {paths_reason}"),
        },
        (Some((ok, reason)), None) | (None, Some((ok, reason))) => DetectResult {
            detected: ok,
            reason,
        },
        // `detect` is non-empty but contains only keys we don't understand.
        // Fail-closed: treat as not-detected so a future `command` or
        // `version` key isn't silently accepted before its logic lands.
        (None, None) => {
            let keys: Vec<&str> = detect.keys().map(|k| k.as_str()).collect();
            DetectResult {
                detected: false,
                reason: format!("unsupported detect keys: {}", keys.join(", ")),
            }
        }
    }
}

/// Scan `PATH` directories for an executable named `name`.
///
/// On Unix the candidate must also have an executable bit set; on other
/// platforms a plain `is_file` check suffices.
fn find_binary_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

/// Extract a list of strings from a TOML value that is either a single
/// string or an array of strings.
fn extract_string_list(value: &toml::Value) -> Vec<String> {
    match value {
        toml::Value::String(s) => vec![s.clone()],
        toml::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Placeholder expansion
// ---------------------------------------------------------------------------

/// Replace layout placeholders in `template` with concrete paths from
/// `layout`.
///
/// Recognised placeholders (braces are literal in the template):
///
/// | Placeholder                    | Field              |
/// |-------------------------------|--------------------|
/// | `{bindir}`                    | `layout.bin_dir`   |
/// | `{libdir}`, `{lib_dir}`      | `layout.lib_dir`   |
/// | `{libexecdir}`, `{libexec_dir}` | `layout.libexec_dir` |
/// | `{datadir}`                   | `layout.datadir`   |
/// | `{etcdir}`, `{etc_dir}`      | `layout.etc_dir`   |
/// | `{statedir}`, `{state_dir}`  | `layout.state_dir` |
/// | `{logdir}`, `{log_dir}`      | `layout.log_dir`   |
/// | `{cachedir}`, `{cache_dir}`  | `layout.cache_dir` |
///
/// Additional variables can be supplied via `extra_vars` (e.g.
/// `("component", "tokenless")` to expand `{component}`).
///
/// Any `{...}` token that is neither a layout field nor an extra variable
/// produces an [`AdapterError::UnknownPlaceholder`].
pub fn expand_layout_placeholders(
    template: &str,
    layout: &FsLayout,
    extra_vars: &[(&str, &str)],
) -> Result<PathBuf, AdapterError> {
    let mut replacements: BTreeMap<&str, &Path> = BTreeMap::new();

    replacements.insert("bindir", &layout.bin_dir);
    replacements.insert("libdir", &layout.lib_dir);
    replacements.insert("lib_dir", &layout.lib_dir);
    replacements.insert("libexecdir", &layout.libexec_dir);
    replacements.insert("libexec_dir", &layout.libexec_dir);
    replacements.insert("datadir", &layout.datadir);
    replacements.insert("etcdir", &layout.etc_dir);
    replacements.insert("etc_dir", &layout.etc_dir);
    replacements.insert("statedir", &layout.state_dir);
    replacements.insert("state_dir", &layout.state_dir);
    replacements.insert("logdir", &layout.log_dir);
    replacements.insert("log_dir", &layout.log_dir);
    replacements.insert("cachedir", &layout.cache_dir);
    replacements.insert("cache_dir", &layout.cache_dir);

    let mut result = template.to_string();
    let mut search_from = 0;

    while let Some(rel_open) = result[search_from..].find('{') {
        let open = search_from + rel_open;
        let close = match result[open..].find('}') {
            Some(pos) => open + pos,
            None => break,
        };

        let key = &result[open + 1..close];

        if let Some(path) = replacements.get(key) {
            let path_str = path.to_string_lossy();
            result.replace_range(open..=close, &path_str);
            search_from = open + path_str.len();
        } else if let Some((_, val)) = extra_vars.iter().find(|(k, _)| *k == key) {
            result.replace_range(open..=close, val);
            search_from = open + val.len();
        } else {
            return Err(AdapterError::UnknownPlaceholder {
                placeholder: key.to_string(),
                template: template.to_string(),
            });
        }
    }

    Ok(PathBuf::from(result))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- detect_framework ---------------------------------------------------

    #[test]
    fn detect_empty_map_returns_detected() {
        let spec = AdapterSpec::default();
        let result = detect_framework(&spec);
        assert!(result.detected);
        assert_eq!(result.reason, "no detection configured");
    }

    #[test]
    fn detect_binary_found_in_path() {
        let mut detect = BTreeMap::new();
        detect.insert("binary".to_string(), toml::Value::String("sh".to_string()));
        let spec = AdapterSpec {
            detect,
            ..Default::default()
        };
        let result = detect_framework(&spec);
        assert!(
            result.detected,
            "expected sh to be found: {}",
            result.reason
        );
        assert!(
            result.reason.contains("found at"),
            "reason should mention path: {}",
            result.reason
        );
    }

    #[test]
    fn detect_binary_not_found() {
        let mut detect = BTreeMap::new();
        detect.insert(
            "binary".to_string(),
            toml::Value::String("nonexistent_binary_xyz_12345".to_string()),
        );
        let spec = AdapterSpec {
            detect,
            ..Default::default()
        };
        let result = detect_framework(&spec);
        assert!(!result.detected);
        assert!(result.reason.contains("not found in PATH"));
    }

    #[test]
    fn detect_paths_existing() {
        let mut detect = BTreeMap::new();
        detect.insert(
            "paths".to_string(),
            toml::Value::Array(vec![toml::Value::String("/tmp".to_string())]),
        );
        let spec = AdapterSpec {
            detect,
            ..Default::default()
        };
        let result = detect_framework(&spec);
        assert!(result.detected, "expected /tmp to exist: {}", result.reason);
        assert!(result.reason.contains("exists"));
    }

    #[test]
    fn detect_paths_single_string() {
        let mut detect = BTreeMap::new();
        detect.insert("paths".to_string(), toml::Value::String("/tmp".to_string()));
        let spec = AdapterSpec {
            detect,
            ..Default::default()
        };
        let result = detect_framework(&spec);
        assert!(
            result.detected,
            "single-string paths should work: {}",
            result.reason
        );
    }

    #[test]
    fn detect_paths_none_exist() {
        let mut detect = BTreeMap::new();
        detect.insert(
            "paths".to_string(),
            toml::Value::Array(vec![
                toml::Value::String("/nonexistent_path_xyz_1".to_string()),
                toml::Value::String("/nonexistent_path_xyz_2".to_string()),
            ]),
        );
        let spec = AdapterSpec {
            detect,
            ..Default::default()
        };
        let result = detect_framework(&spec);
        assert!(!result.detected);
        assert!(result.reason.contains("none of the paths exist"));
    }

    #[test]
    fn detect_binary_and_paths_both_required() {
        let mut detect = BTreeMap::new();
        detect.insert("binary".to_string(), toml::Value::String("sh".to_string()));
        detect.insert(
            "paths".to_string(),
            toml::Value::Array(vec![toml::Value::String(
                "/nonexistent_path_xyz_1".to_string(),
            )]),
        );
        let spec = AdapterSpec {
            detect,
            ..Default::default()
        };
        let result = detect_framework(&spec);
        assert!(
            !result.detected,
            "AND logic: paths missing should fail: {}",
            result.reason
        );
    }

    #[test]
    fn detect_binary_and_paths_both_present() {
        let mut detect = BTreeMap::new();
        detect.insert("binary".to_string(), toml::Value::String("sh".to_string()));
        detect.insert(
            "paths".to_string(),
            toml::Value::Array(vec![toml::Value::String("/tmp".to_string())]),
        );
        let spec = AdapterSpec {
            detect,
            ..Default::default()
        };
        let result = detect_framework(&spec);
        assert!(
            result.detected,
            "both binary and path present should succeed: {}",
            result.reason
        );
    }

    // -- expand_layout_placeholders -----------------------------------------

    fn test_layout() -> FsLayout {
        FsLayout::system(None)
    }

    #[test]
    fn expand_bindir() {
        let layout = test_layout();
        let result = expand_layout_placeholders("{bindir}/agentsight", &layout, &[]).unwrap();
        assert_eq!(result, PathBuf::from("/usr/local/bin/agentsight"));
    }

    #[test]
    fn expand_datadir() {
        let layout = test_layout();
        let result =
            expand_layout_placeholders("{datadir}/adapters/openclaw/", &layout, &[]).unwrap();
        assert_eq!(
            result,
            PathBuf::from("/usr/local/share/anolisa/adapters/openclaw/")
        );
    }

    #[test]
    fn expand_etcdir_alias() {
        let layout = test_layout();
        let r1 = expand_layout_placeholders("{etcdir}/conf.toml", &layout, &[]).unwrap();
        let r2 = expand_layout_placeholders("{etc_dir}/conf.toml", &layout, &[]).unwrap();
        assert_eq!(r1, r2);
        assert_eq!(r1, PathBuf::from("/etc/anolisa/conf.toml"));
    }

    #[test]
    fn expand_statedir_alias() {
        let layout = test_layout();
        let r1 = expand_layout_placeholders("{statedir}/data", &layout, &[]).unwrap();
        let r2 = expand_layout_placeholders("{state_dir}/data", &layout, &[]).unwrap();
        assert_eq!(r1, r2);
        assert_eq!(r1, PathBuf::from("/var/lib/anolisa/data"));
    }

    #[test]
    fn expand_logdir_alias() {
        let layout = test_layout();
        let r1 = expand_layout_placeholders("{logdir}/app.log", &layout, &[]).unwrap();
        let r2 = expand_layout_placeholders("{log_dir}/app.log", &layout, &[]).unwrap();
        assert_eq!(r1, r2);
        assert_eq!(r1, PathBuf::from("/var/log/anolisa/app.log"));
    }

    #[test]
    fn expand_libdir_alias() {
        let layout = test_layout();
        let r1 = expand_layout_placeholders("{libdir}/plugin.so", &layout, &[]).unwrap();
        let r2 = expand_layout_placeholders("{lib_dir}/plugin.so", &layout, &[]).unwrap();
        assert_eq!(r1, r2);
        assert_eq!(r1, PathBuf::from("/usr/local/lib/anolisa/plugin.so"));
    }

    #[test]
    fn expand_libexecdir_alias() {
        let layout = test_layout();
        let r1 = expand_layout_placeholders("{libexecdir}/helper", &layout, &[]).unwrap();
        let r2 = expand_layout_placeholders("{libexec_dir}/helper", &layout, &[]).unwrap();
        assert_eq!(r1, r2);
        assert_eq!(r1, PathBuf::from("/usr/local/libexec/anolisa/helper"));
    }

    #[test]
    fn expand_cachedir_alias() {
        let layout = test_layout();
        let r1 = expand_layout_placeholders("{cachedir}/tmp", &layout, &[]).unwrap();
        let r2 = expand_layout_placeholders("{cache_dir}/tmp", &layout, &[]).unwrap();
        assert_eq!(r1, r2);
        assert_eq!(r1, PathBuf::from("/var/cache/anolisa/tmp"));
    }

    #[test]
    fn expand_with_extra_vars() {
        let layout = test_layout();
        let result = expand_layout_placeholders(
            "{datadir}/adapters/{component}/openclaw/",
            &layout,
            &[("component", "tokenless")],
        )
        .unwrap();
        assert_eq!(
            result,
            PathBuf::from("/usr/local/share/anolisa/adapters/tokenless/openclaw/")
        );
    }

    #[test]
    fn expand_unknown_placeholder_errors() {
        let layout = test_layout();
        let err = expand_layout_placeholders("{datadir}/adapters/{unknown_thing}/", &layout, &[]);
        assert!(err.is_err());
        let err = err.unwrap_err();
        assert!(
            err.to_string().contains("unknown_thing"),
            "error should name the placeholder: {err}"
        );
    }

    #[test]
    fn detect_unknown_keys_fail_closed() {
        let mut detect = BTreeMap::new();
        detect.insert(
            "command".to_string(),
            toml::Value::String("openclaw --version".to_string()),
        );
        let spec = AdapterSpec {
            detect,
            ..Default::default()
        };
        let result = detect_framework(&spec);
        assert!(
            !result.detected,
            "unknown detect keys must fail-closed: {}",
            result.reason
        );
        assert!(
            result.reason.contains("unsupported detect keys"),
            "reason should mention unsupported: {}",
            result.reason
        );
    }

    #[test]
    fn expand_no_placeholders() {
        let layout = test_layout();
        let result = expand_layout_placeholders("/absolute/path/no/vars", &layout, &[]).unwrap();
        assert_eq!(result, PathBuf::from("/absolute/path/no/vars"));
    }
}
