use anolisa_core::state::{
    InstalledObject, InstalledState, ObjectKind, ObjectStatus, Ownership, RpmMetadata,
};
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError, PackageVersion};

use crate::commands::tier1::list::state_view::{self, LocalProjection};
use crate::resolution::{
    ComponentAliasEntry, ComponentBackendEntry, ComponentIndex, ComponentIndexEntry,
};

pub(super) fn sample_index() -> ComponentIndex {
    ComponentIndex {
        schema_version: 1,
        generated_at: None,
        publisher: Some("anolisa".to_string()),
        components: vec![
            ComponentIndexEntry {
                name: "agentsight".to_string(),
                display_name: Some("AgentSight".to_string()),
                summary: Some("eBPF-based AI agent observability tool".to_string()),
                backends: vec![
                    ComponentBackendEntry {
                        kind: "raw".to_string(),
                        package: "agentsight".to_string(),
                        provides: None,
                        legacy_adopt: false,
                    },
                    ComponentBackendEntry {
                        kind: "rpm".to_string(),
                        package: "agentsight".to_string(),
                        provides: Some("anolisa-component(agentsight)".to_string()),
                        legacy_adopt: true,
                    },
                ],
                aliases: Vec::new(),
            },
            ComponentIndexEntry {
                name: "tokenless".to_string(),
                display_name: Some("Tokenless".to_string()),
                summary: Some("LLM token optimization toolkit".to_string()),
                backends: vec![ComponentBackendEntry {
                    kind: "raw".to_string(),
                    package: "tokenless".to_string(),
                    provides: None,
                    legacy_adopt: false,
                }],
                aliases: Vec::new(),
            },
        ],
    }
}

pub(super) fn empty_state() -> InstalledState {
    InstalledState::default()
}

pub(super) fn state_with_object(
    kind: ObjectKind,
    name: &str,
    status: ObjectStatus,
) -> InstalledState {
    let mut state = InstalledState::default();
    state.objects.push(InstalledObject {
        kind,
        name: name.to_string(),
        version: "0.1.0".to_string(),
        status,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: None,
        ownership: None,
        rpm_metadata: None,
        installed_at: "2026-06-12T00:00:00Z".to_string(),
        last_operation_id: None,
        managed: true,
        adopted: false,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: Vec::new(),
        files: Vec::new(),
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
        provisioned_packages: Vec::new(),
    });
    state
}

pub(super) fn state_with_component_object(mut object: InstalledObject) -> InstalledState {
    let mut state = InstalledState::default();
    object.kind = ObjectKind::Component;
    state.objects.push(object);
    state
}

pub(super) fn component_object(
    name: &str,
    status: ObjectStatus,
    ownership: Ownership,
) -> InstalledObject {
    InstalledObject {
        kind: ObjectKind::Component,
        name: name.to_string(),
        version: "0.1.0".to_string(),
        status,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some(if ownership.is_rpm() { "rpm" } else { "raw" }.to_string()),
        ownership: Some(ownership),
        rpm_metadata: None,
        installed_at: "2026-06-12T00:00:00Z".to_string(),
        last_operation_id: None,
        managed: ownership.owns_removal(),
        adopted: ownership == Ownership::RpmObserved,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: Vec::new(),
        files: Vec::new(),
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
        provisioned_packages: Vec::new(),
    }
}

pub(super) fn rpm_component_object(
    name: &str,
    status: ObjectStatus,
    ownership: Ownership,
    package: &str,
    evr: &str,
) -> InstalledObject {
    let mut object = component_object(name, status, ownership);
    object.rpm_metadata = Some(RpmMetadata {
        package_name: package.to_string(),
        evr: Some(evr.to_string()),
        arch: Some("x86_64".to_string()),
        source_repo: Some("@System".to_string()),
    });
    object
}

#[derive(Default)]
pub(super) struct FakeRpmQuery {
    pub(super) installed: Vec<(String, PackageInfo)>,
    pub(super) command_missing: bool,
    pub(super) what_provides: Vec<(String, Vec<String>)>,
}

impl PackageQuery for FakeRpmQuery {
    fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
        if self.command_missing {
            return Err(PackageQueryError::CommandMissing {
                command: "rpm".to_string(),
            });
        }
        Ok(self
            .installed
            .iter()
            .find(|(name, _)| name == package)
            .map(|(_, info)| info.clone()))
    }

    fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
        Ok(Vec::new())
    }

    fn what_provides_installed(&self, capability: &str) -> Result<Vec<String>, PackageQueryError> {
        if self.command_missing {
            return Err(PackageQueryError::CommandMissing {
                command: "rpm".to_string(),
            });
        }
        Ok(self
            .what_provides
            .iter()
            .find(|(cap, _)| cap == capability)
            .map(|(_, names)| names.clone())
            .unwrap_or_default())
    }
}

pub(super) fn pkg_info(
    name: &str,
    version: &str,
    release: Option<&str>,
    arch: &str,
) -> PackageInfo {
    PackageInfo {
        name: name.to_string(),
        version: PackageVersion {
            epoch: None,
            version: version.to_string(),
            release: release.map(str::to_string),
        },
        arch: arch.to_string(),
        origin: Some("@System".to_string()),
    }
}

pub(super) fn projection_for(
    component: &str,
    state: &InstalledState,
    query: &dyn PackageQuery,
) -> LocalProjection {
    let index = sample_index();
    projection_for_index(&index, component, state, query)
}

pub(super) fn projection_for_index(
    index: &ComponentIndex,
    component: &str,
    state: &InstalledState,
    query: &dyn PackageQuery,
) -> LocalProjection {
    let entry = index
        .components
        .iter()
        .find(|entry| entry.name == component)
        .unwrap();
    state_view::project_component(entry, state, Some(query))
}

/// A component whose RPM backend package name differs from the component name,
/// with an rpm-package alias — mirrors the real `cosh` / `copilot-shell` mapping.
pub(super) fn sample_index_with_aliases() -> ComponentIndex {
    ComponentIndex {
        schema_version: 1,
        generated_at: None,
        publisher: Some("anolisa".to_string()),
        components: vec![ComponentIndexEntry {
            name: "cosh".to_string(),
            display_name: Some("Copilot Shell".to_string()),
            summary: Some("shell".to_string()),
            backends: vec![
                ComponentBackendEntry {
                    kind: "raw".to_string(),
                    package: "cosh".to_string(),
                    provides: None,
                    legacy_adopt: false,
                },
                ComponentBackendEntry {
                    kind: "rpm".to_string(),
                    package: "copilot-shell".to_string(),
                    provides: Some("anolisa-component(cosh)".to_string()),
                    legacy_adopt: true,
                },
            ],
            aliases: vec![ComponentAliasEntry {
                kind: "rpm-package".to_string(),
                name: "cosh-old".to_string(),
            }],
        }],
    }
}
