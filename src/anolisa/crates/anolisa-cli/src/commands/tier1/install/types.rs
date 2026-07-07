//! Shared types for the `install` command: resolution shapes, wire payloads,
//! and plan/preview structs.

use serde::Serialize;
use std::path::PathBuf;

use anolisa_core::{
    CapabilityRequest, ComponentManifest, DependencyKind, DependencyResolution, DependencyStatus,
    DistributionEntry, ProvisionPlan, ResolvedInstallFile, ServiceRequest,
};

/// Raw backend resolution shared by dry-run preview and real execution.
///
/// `pub(crate)` so the `update` command can reuse the same resolution shape
/// when refreshing a raw-managed component to the latest published version.
pub(crate) struct RawResolution {
    pub(crate) component: String,
    pub(crate) package: String,
    pub(crate) backend: String,
    pub(crate) base_url: String,
    pub(crate) entry: DistributionEntry,
    pub(crate) artifact_url: String,
    pub(crate) warnings: Vec<String>,
}

/// Dry-run preview after optional lightweight metadata expansion.
pub(crate) struct InstallPreview {
    pub(crate) resolution: RawResolution,
    pub(crate) files: Vec<ResolvedInstallFile>,
    pub(crate) services: Vec<ServiceRequest>,
    pub(crate) capabilities: Vec<CapabilityRequest>,
    /// Runtime-dependency preflight outcomes. Empty when the artifact was not
    /// downloaded (file/service details unavailable) or the component declares
    /// none.
    pub(crate) dependencies: Vec<DependencyResolution>,
    /// Provisioner classification for dry-run display.
    pub(crate) provision_plan: Option<ProvisionPlan>,
}

/// Execution input after the artifact has been verified and its install
/// contract has been resolved.
///
/// `pub(crate)` so the `update` command can drive the same download-verify
/// step and then replace the on-disk files transactionally.
pub(crate) struct PreparedInstall {
    pub(crate) resolution: RawResolution,
    pub(crate) artifact_path: PathBuf,
    pub(crate) files: Vec<ResolvedInstallFile>,
    /// Declared service activations (unit + scope + enable/start), applied
    /// after files land. Carried resolved with template instances expanded.
    pub(crate) services: Vec<ServiceRequest>,
    /// Linux file capabilities to apply after files land (raw, system mode
    /// only). Carried resolved — path already layout-expanded and bounded.
    pub(crate) capabilities: Vec<CapabilityRequest>,
    pub(crate) manifest_toml: String,
}

/// Parsed install contract plus the TOML persisted as the local install fact.
pub(crate) struct LoadedInstallContract {
    pub(crate) manifest: ComponentManifest,
    pub(crate) source: InstallContractSource,
    pub(crate) toml: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallContractSource {
    EmbeddedArtifact,
    SidecarMeta,
    LocalCatalog,
}

#[derive(Serialize)]
pub(crate) struct ArtifactInfo {
    pub(crate) r#type: String,
    pub(crate) url: String,
    pub(crate) sha256: Option<String>,
}

/// Wire shape for `--dry-run`: the resolution result without downloading
/// the install artifact.
#[derive(Serialize)]
pub(crate) struct InstallPlanPayload {
    pub(crate) component: String,
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) backend: String,
    pub(crate) base_url: String,
    pub(crate) install_mode: String,
    pub(crate) artifact: ArtifactInfo,
    pub(crate) files: Vec<String>,
    pub(crate) services: Vec<String>,
    /// Human-readable `path: cap,cap` lines for the capabilities install
    /// would apply. Rendered for `--dry-run`; setcap is never run here.
    pub(crate) capabilities: Vec<String>,
    /// Runtime-dependency preflight rows the real install would enforce.
    /// Reported only; `--dry-run` never fails on a missing dependency.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) dependencies: Vec<DependencyPlanRow>,
    pub(crate) dry_run: bool,
    pub(crate) warnings: Vec<String>,
}

/// Flat preflight status for the dry-run wire. Projects the data-carrying
/// [`DependencyStatus`] onto a serializable tag; its payload moves to
/// [`DependencyPlanRow::note`].
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DependencyPlanStatus {
    Resolved,
    Unresolved,
    Unresolvable,
}

impl DependencyPlanStatus {
    /// Display spelling, matching the serde representation.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            DependencyPlanStatus::Resolved => "resolved",
            DependencyPlanStatus::Unresolved => "unresolved",
            DependencyPlanStatus::Unresolvable => "unresolvable",
        }
    }
}

/// One dependency row in the `--dry-run` plan, mirroring a
/// [`DependencyResolution`] onto the wire.
#[derive(Serialize)]
pub(crate) struct DependencyPlanRow {
    /// Logical dependency name.
    pub(crate) name: String,
    /// Dependency kind; serializes kebab-case (e.g. `system-package`).
    pub(crate) kind: DependencyKind,
    /// Preflight outcome.
    pub(crate) status: DependencyPlanStatus,
    /// Provisioner action the real install would take.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) action: Option<DependencyPlanAction>,
    /// Remediation command (`unresolved`) or reason (`unresolvable`); absent
    /// when resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    /// Optional human note (e.g. an unverified version constraint).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
}

/// What the provisioner would do with an unresolved dependency.
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DependencyPlanAction {
    /// Will be auto-installed via system package manager.
    AutoInstall,
    /// Must be installed manually by the user.
    Manual,
}

impl DependencyPlanRow {
    /// Project a resolver outcome onto the dry-run wire row.
    pub(crate) fn from_resolution(r: &DependencyResolution) -> Self {
        let (status, note) = match &r.status {
            DependencyStatus::Resolved => (DependencyPlanStatus::Resolved, None),
            DependencyStatus::Unresolved { remediation } => {
                (DependencyPlanStatus::Unresolved, Some(remediation.clone()))
            }
            DependencyStatus::Unresolvable { reason } => {
                (DependencyPlanStatus::Unresolvable, Some(reason.clone()))
            }
        };
        DependencyPlanRow {
            name: r.name.clone(),
            kind: r.kind,
            status,
            action: None,
            note,
            detail: r.detail.clone(),
        }
    }

    /// Annotate with the provisioner action based on the ProvisionPlan.
    pub(crate) fn with_provision_action(mut self, provision: &ProvisionPlan) -> Self {
        if matches!(self.status, DependencyPlanStatus::Resolved) {
            return self;
        }
        if provision.installable.iter().any(|p| p.name == self.name) {
            self.action = Some(DependencyPlanAction::AutoInstall);
        } else if provision.manual.iter().any(|m| m.name == self.name) {
            self.action = Some(DependencyPlanAction::Manual);
        }
        self
    }
}

/// Wire shape for a completed install.
#[derive(Serialize)]
pub(crate) struct InstallResultPayload {
    pub(crate) component: String,
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) backend: String,
    pub(crate) base_url: String,
    pub(crate) install_mode: String,
    pub(crate) operation_id: String,
    pub(crate) artifact_url: String,
    pub(crate) files_installed: Vec<String>,
    pub(crate) services: Vec<String>,
    pub(crate) provisioned_packages: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

/// What `handle_one` did, so `--all` can distinguish a fresh install from an
/// RPM adopt in its batch summary (§7.5). The dry-run vs real distinction is
/// layered on by the caller from `CliContext::dry_run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallOutcome {
    /// A raw install (downloaded + placed files, or its dry-run preview).
    Installed,
    /// An existing system RPM recorded as `rpm-observed` (or its dry-run
    /// preview); no bytes fetched, no owned files written.
    Adopted,
}

/// Source that decided the backend name in layer 1 (§4). Only used to phrase
/// conflict errors; the action is chosen by layer 2 from `(backend, rpmdb,
/// mode)`, independent of how the name was picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendSource {
    /// Explicit `--backend`.
    Explicit,
    /// Component already in state; backend follows its recorded provenance.
    ExistingState,
    /// State miss; system mode + rpmdb hit selected `rpm`.
    SystemRpm,
    /// None of the above; fell back to `default_backend`.
    Default,
}

/// Caller-side inputs to `resolve_raw`, grouped to keep the signature flat.
pub(crate) struct ResolveInputs<'a> {
    pub(crate) component: String,
    pub(crate) package: String,
    pub(crate) backend: String,
    pub(crate) base_url: String,
    pub(crate) version: Option<&'a str>,
    pub(crate) warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_core::{DependencyKind, DependencyResolution, DependencyStatus};

    #[test]
    fn dependency_plan_row_projects_each_status() {
        let resolved = DependencyResolution {
            name: "btrfs-progs".to_string(),
            kind: DependencyKind::SystemPackage,
            status: DependencyStatus::Resolved,
            detail: None,
        };
        let row = DependencyPlanRow::from_resolution(&resolved);
        assert!(matches!(row.kind, DependencyKind::SystemPackage));
        assert_eq!(row.status.as_str(), "resolved");
        assert!(row.note.is_none());

        let missing = DependencyResolution {
            name: "btrfs-progs".to_string(),
            kind: DependencyKind::SystemPackage,
            status: DependencyStatus::Unresolved {
                remediation: "sudo dnf install btrfs-progs".to_string(),
            },
            detail: None,
        };
        let row = DependencyPlanRow::from_resolution(&missing);
        assert_eq!(row.status.as_str(), "unresolved");
        assert_eq!(row.note.as_deref(), Some("sudo dnf install btrfs-progs"));

        let cap = DependencyResolution {
            name: "btrfs".to_string(),
            kind: DependencyKind::PlatformCapability,
            status: DependencyStatus::Unresolvable {
                reason: "requires kernel >= 5.4, host is 3.10".to_string(),
            },
            detail: None,
        };
        let row = DependencyPlanRow::from_resolution(&cap);
        assert_eq!(row.status.as_str(), "unresolvable");
        assert!(row.note.unwrap().contains("kernel >= 5.4"));
    }

    #[test]
    fn dependency_plan_row_serializes_kind_and_status_kebab_case() {
        // The enum-typed `kind`/`status` must reach the wire as kebab-case so
        // the JSON contract is unchanged by using enums instead of strings.
        let row = DependencyPlanRow::from_resolution(&DependencyResolution {
            name: "btrfs-progs".to_string(),
            kind: DependencyKind::SystemPackage,
            status: DependencyStatus::Resolved,
            detail: None,
        });
        let json = serde_json::to_string(&row).expect("serialize");
        assert!(json.contains("\"kind\":\"system-package\""), "{json}");
        assert!(json.contains("\"status\":\"resolved\""), "{json}");
    }
}
