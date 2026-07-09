use std::path::PathBuf;

use anolisa_core::state::{InstalledState, ObjectStatus, Ownership};

use crate::commands::state_view::{ScopedStateRoot, StateScope, StateView};
use crate::commands::tier1::list::{ListArgs, build_rows_from_view};

use super::support::{component_object, sample_index, state_with_component_object};

#[test]
fn user_view_installed_filter_keeps_system_provided_component() {
    let index = sample_index();
    let args = ListArgs { installed: true };
    let view = user_plus_system_view(
        InstalledState::default(),
        state_with_component_object(component_object(
            "agentsight",
            ObjectStatus::Installed,
            Ownership::RawManaged,
        )),
    );

    let rows = build_rows_from_view(&index, &args, &view, None);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "agentsight");
    assert_eq!(rows[0].local_state, "installed");
    assert_eq!(rows[0].scope, "system");
    assert_eq!(
        rows[0].state_path.as_deref(),
        Some("/tmp/anolisa-system-state/installed.toml")
    );
    assert!(rows[0].active);
    assert!(!rows[0].mutable_by_current_invocation);
}

#[test]
fn user_record_shadows_system_record_in_list_rows() {
    let index = sample_index();
    let args = ListArgs { installed: true };
    let view = user_plus_system_view(
        state_with_component_object(component_object(
            "agentsight",
            ObjectStatus::Installed,
            Ownership::RawManaged,
        )),
        state_with_component_object(component_object(
            "agentsight",
            ObjectStatus::Installed,
            Ownership::RpmManaged,
        )),
    );

    let rows = build_rows_from_view(&index, &args, &view, None);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "agentsight");
    assert_eq!(rows[0].scope, "user");
    assert!(rows[0].mutable_by_current_invocation);
}

fn user_plus_system_view(user_state: InstalledState, system_state: InstalledState) -> StateView {
    let user_root = ScopedStateRoot {
        scope: StateScope::User,
        state_path: PathBuf::from("/tmp/anolisa-user-state/installed.toml"),
        writable: true,
        state: user_state,
    };
    let system_root = ScopedStateRoot {
        scope: StateScope::System,
        state_path: PathBuf::from("/tmp/anolisa-system-state/installed.toml"),
        writable: false,
        state: system_state,
    };
    StateView {
        writable: user_root.clone(),
        visible_roots: vec![user_root, system_root],
        warnings: Vec::new(),
    }
}
