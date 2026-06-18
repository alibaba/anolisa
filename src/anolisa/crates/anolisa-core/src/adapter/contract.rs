//! Component contract discovery.
//!
//! Resolves a [`ComponentManifest`] for an installed component by searching
//! a caller-supplied list of candidate paths in priority order. The typical
//! ordering is:
//!
//! 1. **State snapshots** — one per state root
//!    (`{state_dir}/component-manifests/<component>/component.toml`).
//! 2. **Datadir contracts** — one per datadir root
//!    (`{datadir}/components/<component>/component.toml`).
//!
//! The first file found wins. A TOML parse error is surfaced immediately
//! (it must not be masked as "unavailable").

use std::path::{Path, PathBuf};

use anolisa_platform::fs_layout::FsLayout;

use crate::manifest::{ComponentManifest, ManifestError};

/// Errors from component contract resolution.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    /// No contract file was found under any searched root.
    #[error(
        "component contract unavailable for '{component}': no file found at any of {searched:?}"
    )]
    Unavailable {
        /// Component whose contract was requested.
        component: String,
        /// Paths that were tried, in search order.
        searched: Vec<PathBuf>,
    },

    /// A contract file exists but its TOML content could not be parsed.
    #[error("malformed component contract at {path}: {reason}")]
    ParseError {
        /// Path of the file that failed to parse.
        path: PathBuf,
        /// Human-readable parse failure detail.
        reason: String,
    },

    /// A filesystem error occurred while reading a contract file (other
    /// than "not found", which is handled by the search loop).
    #[error("io error reading component contract at {path}: {source}")]
    Io {
        /// Path that triggered the error.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
}

/// Build the ordered list of candidate contract paths for `component`
/// across `state_roots` (snapshot priority) then `datadir_roots` (package
/// contract fallback). Path computation is delegated to [`FsLayout`] so
/// the segment constants live in one place.
pub fn candidate_paths(
    component: &str,
    state_roots: &[PathBuf],
    datadir_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(state_roots.len() + datadir_roots.len());
    for state_root in state_roots {
        paths.push(FsLayout::component_manifest_snapshot_path(
            state_root, component,
        ));
    }
    for datadir_root in datadir_roots {
        paths.push(FsLayout::component_contract_path(datadir_root, component));
    }
    paths
}

/// Resolve the component contract for `component` by searching state roots
/// then datadir roots in the supplied order.
///
/// Internally builds the candidate list via [`candidate_paths`] and
/// delegates to [`resolve_from_candidates`].
pub fn resolve_component_contract(
    component: &str,
    state_roots: &[PathBuf],
    datadir_roots: &[PathBuf],
) -> Result<ComponentManifest, ContractError> {
    let candidates = candidate_paths(component, state_roots, datadir_roots);
    resolve_from_candidates(component, &candidates)
}

/// Try each candidate path in order and return the first valid manifest.
///
/// An IO error other than `NotFound` (e.g. permission denied) is returned
/// as [`ContractError::Io`]; a present-but-malformed TOML file is returned
/// as [`ContractError::ParseError`] — it is never silently skipped.
pub fn resolve_from_candidates(
    component: &str,
    candidates: &[PathBuf],
) -> Result<ComponentManifest, ContractError> {
    let mut searched = Vec::new();

    for path in candidates {
        match try_load_contract(path) {
            TryLoad::Loaded(manifest) => return Ok(*manifest),
            TryLoad::NotFound => {
                searched.push(path.clone());
            }
            TryLoad::Error(err) => return Err(err),
        }
    }

    Err(ContractError::Unavailable {
        component: component.to_string(),
        searched,
    })
}

/// Three-way outcome of trying to load a single candidate path.
enum TryLoad {
    Loaded(Box<ComponentManifest>),
    NotFound,
    Error(ContractError),
}

/// Attempt to load a contract from `path`, distinguishing "file absent" from
/// "file present but broken" from "file present and valid".
fn try_load_contract(path: &Path) -> TryLoad {
    match ComponentManifest::from_file(path) {
        Ok(manifest) => TryLoad::Loaded(Box::new(manifest)),
        Err(ManifestError::Io(_, ref io_err)) if io_err.kind() == std::io::ErrorKind::NotFound => {
            TryLoad::NotFound
        }
        Err(ManifestError::Io(_, source)) => TryLoad::Error(ContractError::Io {
            path: path.to_path_buf(),
            source,
        }),
        Err(ManifestError::Parse(_, reason)) => TryLoad::Error(ContractError::ParseError {
            path: path.to_path_buf(),
            reason,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Minimal valid component TOML for testing.
    fn valid_toml(name: &str) -> String {
        format!(
            r#"
[component]
name = "{name}"
version = "0.1.0"
layer = "runtime"
"#
        )
    }

    fn write_snapshot(state_root: &Path, component: &str, content: &str) {
        let path = FsLayout::component_manifest_snapshot_path(state_root, component);
        fs::create_dir_all(path.parent().unwrap()).expect("create dir");
        fs::write(&path, content).expect("write");
    }

    fn write_datadir(datadir_root: &Path, component: &str, content: &str) {
        let path = FsLayout::component_contract_path(datadir_root, component);
        fs::create_dir_all(path.parent().unwrap()).expect("create dir");
        fs::write(&path, content).expect("write");
    }

    /// Minimal valid component TOML with a specific version, used to
    /// distinguish which file was loaded.
    fn valid_toml_versioned(name: &str, version: &str) -> String {
        format!(
            r#"
[component]
name = "{name}"
version = "{version}"
layer = "runtime"
"#
        )
    }

    #[test]
    fn snapshot_preferred_over_datadir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let data = tmp.path().join("data");

        write_snapshot(&state, "mycomp", &valid_toml_versioned("mycomp", "1.0.0"));
        write_datadir(&data, "mycomp", &valid_toml_versioned("mycomp", "2.0.0"));

        let manifest =
            resolve_component_contract("mycomp", &[state], &[data]).expect("should resolve");
        assert_eq!(manifest.component.name, "mycomp");
        assert_eq!(manifest.component.version, "1.0.0");
    }

    #[test]
    fn datadir_found_when_snapshot_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let data = tmp.path().join("data");

        write_datadir(&data, "mycomp", &valid_toml("mycomp"));

        let manifest =
            resolve_component_contract("mycomp", &[state], &[data]).expect("should resolve");
        assert_eq!(manifest.component.name, "mycomp");
    }

    #[test]
    fn both_absent_returns_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let data = tmp.path().join("data");

        let err = resolve_component_contract("mycomp", &[state], &[data])
            .expect_err("should be unavailable");
        assert!(
            matches!(err, ContractError::Unavailable { .. }),
            "expected Unavailable, got: {err}"
        );
    }

    #[test]
    fn malformed_toml_returns_parse_error_not_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let data = tmp.path().join("data");

        write_snapshot(&state, "mycomp", "this is not valid toml = [[[");

        let err = resolve_component_contract("mycomp", &[state], &[data])
            .expect_err("should be parse error");
        assert!(
            matches!(err, ContractError::ParseError { .. }),
            "expected ParseError, got: {err}"
        );
    }

    #[test]
    fn malformed_snapshot_not_masked_by_valid_datadir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let data = tmp.path().join("data");

        write_snapshot(&state, "mycomp", "bad toml {{{{");
        write_datadir(&data, "mycomp", &valid_toml("mycomp"));

        let err = resolve_component_contract("mycomp", &[state], &[data])
            .expect_err("should be parse error");
        assert!(
            matches!(err, ContractError::ParseError { .. }),
            "expected ParseError, got: {err}"
        );
    }

    #[test]
    fn multiple_state_roots_searched_in_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state1 = tmp.path().join("state1");
        let state2 = tmp.path().join("state2");
        let data = tmp.path().join("data");

        write_snapshot(&state2, "mycomp", &valid_toml("mycomp"));

        let manifest = resolve_component_contract("mycomp", &[state1, state2], &[data])
            .expect("should resolve from state2");
        assert_eq!(manifest.component.name, "mycomp");
    }

    #[test]
    fn multiple_datadir_roots_searched_in_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let data1 = tmp.path().join("data1");
        let data2 = tmp.path().join("data2");

        write_datadir(&data2, "mycomp", &valid_toml("mycomp"));

        let manifest = resolve_component_contract("mycomp", &[state], &[data1, data2])
            .expect("should resolve from data2");
        assert_eq!(manifest.component.name, "mycomp");
    }
}
