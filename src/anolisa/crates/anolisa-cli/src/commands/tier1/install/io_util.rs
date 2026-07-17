//! File-system and IO helpers: atomic writes, rollback, state snapshots,
//! and timestamp formatting for the `install` command.

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anolisa_core::central_log::CentralLog;
use anolisa_core::{ServiceManager, ServiceRunOutcome, deactivate_services};
use anolisa_platform::fs_layout::FsLayout;
use chrono::{SecondsFormat, Utc};

use crate::commands::common;
use crate::response::CliError;

/// Best-effort cleanup of installed files after a state-save failure.
pub(crate) fn rollback_installed_files(files: &[anolisa_core::InstalledFile]) {
    for f in files {
        let _ = std::fs::remove_file(&f.path);
    }
}

/// Best-effort cleanup for service side effects from an install that will
/// otherwise roll back.
pub(crate) fn rollback_activated_services(
    manager: &dyn ServiceManager,
    service_run: &ServiceRunOutcome,
    log: Option<&CentralLog>,
    component: &str,
    operation_id: &str,
    install_mode: &str,
) -> Vec<String> {
    let units: BTreeSet<String> = service_run
        .enabled_units
        .iter()
        .chain(service_run.started_units.iter())
        .cloned()
        .collect();
    if units.is_empty() {
        return Vec::new();
    }
    let units = units
        .into_iter()
        .map(|unit| (component.to_string(), unit))
        .collect::<Vec<_>>();
    deactivate_services(manager, &units, log, operation_id, "cli", install_mode).warnings
}

pub(crate) fn service_cleanup_suffix(warnings: &[String]) -> String {
    if warnings.is_empty() {
        String::new()
    } else {
        format!("; service cleanup warnings: {}", warnings.join("; "))
    }
}

pub(crate) fn write_installed_component_manifest(
    layout: &FsLayout,
    component: &str,
    toml: &str,
) -> Result<PathBuf, CliError> {
    let path = common::installed_component_manifest_path(layout, component, super::COMMAND)?;
    write_atomic_text(&path, toml).map_err(|err| CliError::Runtime {
        command: super::COMMAND.to_string(),
        reason: format!(
            "failed to write installed component manifest at {}: {err}",
            path.display()
        ),
    })?;
    Ok(path)
}

struct DatadirContract {
    content: String,
    source_path: PathBuf,
    datadir_root: PathBuf,
}

enum DatadirContractLookup {
    Found(DatadirContract),
    Missing(Vec<PathBuf>),
    Unreadable {
        source_path: PathBuf,
        source: std::io::Error,
    },
}

/// Read-only result for comparing an RPM-owned contract with its state snapshot.
pub(crate) struct ContractDriftInspection {
    /// Whether the state snapshot is absent, unreadable, or byte-different.
    pub(crate) drifted: bool,
    /// Non-fatal lookup or path-resolution failures.
    pub(crate) warnings: Vec<String>,
}

fn datadir_contract_roots(layout: &FsLayout) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(packaged) = crate::packaged::packaged_datadir_root(layout) {
        roots.push(packaged);
    }
    if let Some(package_datadir) = layout.package_datadir()
        && !roots.iter().any(|root| root == &package_datadir)
    {
        roots.push(package_datadir);
    }
    if !roots.iter().any(|root| root == &layout.datadir) {
        roots.push(layout.datadir.clone());
    }
    roots
}

fn lookup_datadir_contract(layout: &FsLayout, component: &str) -> DatadirContractLookup {
    let mut searched: Vec<PathBuf> = Vec::new();
    for datadir_root in datadir_contract_roots(layout) {
        let source_path = FsLayout::component_contract_path(&datadir_root, component);
        match std::fs::read_to_string(&source_path) {
            Ok(content) => {
                return DatadirContractLookup::Found(DatadirContract {
                    content,
                    source_path,
                    datadir_root,
                });
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                searched.push(source_path);
            }
            Err(source) => {
                return DatadirContractLookup::Unreadable {
                    source_path,
                    source,
                };
            }
        }
    }
    DatadirContractLookup::Missing(searched)
}

/// Compare the package-owned component contract with its state snapshot.
///
/// A missing package contract is not drift because there is no authoritative
/// content to copy. An unreadable snapshot is drift so a real reconciliation
/// can attempt to replace it and surface any write failure.
pub(crate) fn inspect_datadir_contract_drift(
    layout: &FsLayout,
    component: &str,
    command: &str,
) -> ContractDriftInspection {
    let contract = match lookup_datadir_contract(layout, component) {
        DatadirContractLookup::Found(contract) => contract,
        DatadirContractLookup::Missing(_) => {
            return ContractDriftInspection {
                drifted: false,
                warnings: Vec::new(),
            };
        }
        DatadirContractLookup::Unreadable {
            source_path,
            source,
        } => {
            return ContractDriftInspection {
                drifted: false,
                warnings: vec![format!(
                    "could not read datadir component contract at {}: {source}",
                    source_path.display()
                )],
            };
        }
    };
    let destination = match common::installed_component_manifest_path(layout, component, command) {
        Ok(path) => path,
        Err(err) => {
            return ContractDriftInspection {
                drifted: false,
                warnings: vec![format!(
                    "could not resolve snapshot path for component '{component}': {err}"
                )],
            };
        }
    };

    match std::fs::read_to_string(&destination) {
        Ok(snapshot) => ContractDriftInspection {
            drifted: snapshot != contract.content,
            warnings: Vec::new(),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => ContractDriftInspection {
            drifted: true,
            warnings: Vec::new(),
        },
        Err(err) => ContractDriftInspection {
            drifted: true,
            warnings: vec![format!(
                "could not read installed component manifest at {}: {err}",
                destination.display()
            )],
        },
    }
}

/// Best-effort snapshot of the datadir component contract for RPM paths.
///
/// After an RPM adopt or delegated install the package-owned contract lives
/// at `{datadir}/components/<component>/component.toml`. Real RPMs install
/// to `%{_datadir}` (`/usr/share/anolisa/`), which may differ from the CLI
/// install prefix (`/usr/local/share/anolisa/`). To handle both, this
/// function probes the packaged datadir root first (exe-sibling /
/// `ANOLISA_DATA_DIR` / `layout.datadir`), then falls back to
/// `layout.datadir` if the packaged root differs. The first existing
/// contract wins.
///
/// The contract is copied verbatim (no TOML parsing) to the state snapshot
/// at `{state_dir}/component-manifests/<component>/component.toml` so that
/// later `adapter enable` can discover the component's declared adapters.
///
/// Returns any warning messages that should be surfaced to the user.
/// Neither a missing contract nor a write failure is fatal — both produce
/// a warning instead of an error.
pub(crate) fn snapshot_datadir_contract(
    layout: &FsLayout,
    component: &str,
    command: &str,
) -> Vec<String> {
    snapshot_datadir_contract_with_missing_policy(layout, component, command, true)
}

/// Refresh an existing state snapshot when the RPM publishes a contract.
///
/// Unlike first-time install/adopt, an RPM without a contract is not a new
/// warning during upgrade or repair because there is no snapshot to refresh.
pub(crate) fn refresh_datadir_contract_snapshot(
    layout: &FsLayout,
    component: &str,
    command: &str,
) -> Vec<String> {
    snapshot_datadir_contract_with_missing_policy(layout, component, command, false)
}

fn snapshot_datadir_contract_with_missing_policy(
    layout: &FsLayout,
    component: &str,
    command: &str,
    warn_if_missing: bool,
) -> Vec<String> {
    let mut warnings: Vec<String> = Vec::new();
    let contract = match lookup_datadir_contract(layout, component) {
        DatadirContractLookup::Found(contract) => contract,
        DatadirContractLookup::Missing(searched) => {
            if warn_if_missing {
                let paths: Vec<String> = searched
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                warnings.push(format!(
                    "component '{component}' does not publish an ANOLISA component contract at {}",
                    paths.join(" or ")
                ));
            }
            return warnings;
        }
        DatadirContractLookup::Unreadable {
            source_path,
            source,
        } => {
            warnings.push(format!(
                "could not read datadir component contract at {}: {source}",
                source_path.display()
            ));
            return warnings;
        }
    };

    let dest = match common::installed_component_manifest_path(layout, component, command) {
        Ok(p) => p,
        Err(err) => {
            warnings.push(format!(
                "could not resolve snapshot path for component '{component}': {err}"
            ));
            return warnings;
        }
    };

    if let Err(err) = write_atomic_text(&dest, &contract.content) {
        let msg = format!(
            "failed to snapshot component contract to {}: {err}",
            dest.display()
        );
        eprintln!("warning: {msg}");
        warnings.push(msg);
        return warnings;
    }

    // Best-effort provenance sidecar so adapter operations can resolve
    // {datadir} without content-matching against scoped datadir roots.
    use anolisa_core::adapter::contract::{
        ContractProvenance, ContractSourceKind, write_snapshot_provenance,
    };
    let provenance = ContractProvenance {
        schema_version: 1,
        source_kind: ContractSourceKind::Datadir,
        source_path: contract.source_path,
        datadir_root: contract.datadir_root,
    };
    if let Err(err) = write_snapshot_provenance(&dest, &provenance) {
        let msg = format!("failed to write contract provenance for component '{component}': {err}");
        eprintln!("warning: {msg}");
        warnings.push(msg);
    }

    warnings
}

pub(crate) fn write_atomic_text(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("component.toml");
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let tmp = parent.join(format!(".{name}.tmp-{}-{nanos}", std::process::id()));

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(content.as_bytes())?;
    drop(file);
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

pub(crate) fn rollback_installed_manifest(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// ISO 8601 UTC timestamp with second precision.
pub(crate) fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_core::{FakeServiceManager, ServiceOp, ServiceRunOutcome};

    #[test]
    fn rollback_activated_services_only_cleans_touched_units() {
        let manager = FakeServiceManager::new();
        let service_run = ServiceRunOutcome {
            enabled_units: vec!["enabled.service".to_string()],
            started_units: vec!["started.service".to_string(), "enabled.service".to_string()],
            warnings: Vec::new(),
        };

        let warnings =
            rollback_activated_services(&manager, &service_run, None, "agentsight", "op", "system");

        assert!(warnings.is_empty());
        assert_eq!(
            manager.calls(),
            vec![
                (ServiceOp::Stop, "enabled.service".to_string()),
                (ServiceOp::Disable, "enabled.service".to_string()),
                (ServiceOp::Stop, "started.service".to_string()),
                (ServiceOp::Disable, "started.service".to_string()),
            ]
        );
    }
}
