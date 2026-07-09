use anolisa_core::{
    InstalledObject, InstalledState, ObjectKind, ObjectStatus, Ownership, SubscriptionScope,
};
use anolisa_platform::fs_layout::FsLayout;
use std::path::PathBuf;

use super::super::INSTALLED_STATE_FILE;

pub(super) fn user_layout(home: PathBuf) -> FsLayout {
    FsLayout::user_with_overrides(home, None, None, None, None, None)
}

pub(super) fn write_state(layout: &FsLayout, objects: Vec<InstalledObject>) {
    let state = InstalledState {
        objects,
        ..InstalledState::default()
    };
    state
        .save(&layout.state_dir.join(INSTALLED_STATE_FILE))
        .expect("save state");
}

pub(super) fn component(name: &str) -> InstalledObject {
    InstalledObject {
        kind: ObjectKind::Component,
        name: name.to_string(),
        version: "1.0.0".to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("raw".to_string()),
        ownership: Some(Ownership::RawManaged),
        rpm_metadata: None,
        installed_at: "2026-01-01T00:00:00Z".to_string(),
        last_operation_id: None,
        managed: true,
        adopted: false,
        subscription_scope: SubscriptionScope::None,
        enabled_features: Vec::new(),
        component_refs: Vec::new(),
        files: Vec::new(),
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
        provisioned_packages: Vec::new(),
    }
}
