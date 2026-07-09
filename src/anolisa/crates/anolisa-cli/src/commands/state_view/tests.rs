use anolisa_platform::fs_layout::FsLayout;
use tempfile::tempdir;

use crate::context::{CliContext, InstallMode};

use super::*;
use support::{component, user_layout, write_state};

mod support;

#[test]
fn user_plus_system_orders_user_before_system() {
    let tmp = tempdir().expect("tempdir");
    let user_layout = user_layout(tmp.path().join("home"));
    let system_layout = FsLayout::system(Some(tmp.path().join("system")));
    write_state(&user_layout, vec![component("user-tool")]);
    write_state(&system_layout, vec![component("system-tool")]);

    let view = StateView::from_layouts(
        "test",
        vec![
            (
                user_layout,
                RootSpec {
                    scope: StateScope::User,
                    writable: true,
                },
            ),
            (
                system_layout,
                RootSpec {
                    scope: StateScope::System,
                    writable: false,
                },
            ),
        ],
    )
    .expect("state view");

    assert_eq!(view.visible_roots[0].scope, StateScope::User);
    assert_eq!(view.visible_roots[1].scope, StateScope::System);
    assert!(view.visible_components().iter().any(|record| {
        record.scope() == StateScope::System && record.object.name == "system-tool"
    }));
}

#[test]
fn user_record_shadows_system_record() {
    let tmp = tempdir().expect("tempdir");
    let user_layout = user_layout(tmp.path().join("home"));
    let system_layout = FsLayout::system(Some(tmp.path().join("system")));
    write_state(&user_layout, vec![component("shared")]);
    write_state(&system_layout, vec![component("shared")]);

    let view = StateView::from_layouts(
        "test",
        vec![
            (
                user_layout,
                RootSpec {
                    scope: StateScope::User,
                    writable: true,
                },
            ),
            (
                system_layout,
                RootSpec {
                    scope: StateScope::System,
                    writable: false,
                },
            ),
        ],
    )
    .expect("state view");
    let records = view.visible_components();

    assert!(records.iter().any(|record| {
        record.scope() == StateScope::User && record.object.name == "shared" && record.active
    }));
    assert!(records.iter().any(|record| {
        record.scope() == StateScope::System
            && record.object.name == "shared"
            && !record.active
            && record.shadowed_by == Some(StateScope::User)
    }));
}

#[test]
fn mutability_uses_writable_flag_not_state_path() {
    let tmp = tempdir().expect("tempdir");
    let layout = user_layout(tmp.path().join("home"));
    write_state(&layout, vec![component("shared-path")]);

    let view = StateView::from_layouts(
        "test",
        vec![
            (
                layout.clone(),
                RootSpec {
                    scope: StateScope::System,
                    writable: false,
                },
            ),
            (
                layout,
                RootSpec {
                    scope: StateScope::User,
                    writable: true,
                },
            ),
        ],
    )
    .expect("state view");
    let records = view.visible_components();

    let system_record = records
        .iter()
        .find(|record| record.scope() == StateScope::System)
        .expect("system record");
    assert!(!system_record.mutable_by_current_invocation);

    let user_record = records
        .iter()
        .find(|record| record.scope() == StateScope::User)
        .expect("user record");
    assert!(user_record.mutable_by_current_invocation);
}

#[test]
fn system_mode_visibility_does_not_load_user_state() {
    let tmp = tempdir().expect("tempdir");
    let system_prefix = tmp.path().join("system");
    let system_layout = FsLayout::system(Some(system_prefix.clone()));
    write_state(&system_layout, vec![component("system-tool")]);

    let home = tmp.path().join("home");
    let xdg_state = tmp.path().join("xdg-state");
    let user_layout = FsLayout::user_with_overrides(home, None, None, Some(xdg_state), None, None);
    write_state(&user_layout, vec![component("user-only")]);
    let ctx = CliContext {
        install_mode: InstallMode::System,
        prefix: Some(system_prefix),
        json: true,
        dry_run: false,
        verbose: false,
        quiet: true,
        no_color: true,
    };

    let view =
        StateView::load(&ctx, "test", StateVisibility::UserPlusSystem).expect("system state view");
    let records = view.visible_components();

    assert_eq!(view.visible_roots.len(), 1);
    assert_eq!(view.visible_roots[0].scope, StateScope::System);
    assert!(
        records
            .iter()
            .any(|record| record.object.name == "system-tool")
    );
    assert!(
        records
            .iter()
            .all(|record| record.object.name != "user-only"),
        "system mode must not inspect ordinary user state"
    );
}

#[test]
fn malformed_visible_system_state_records_warning() {
    let tmp = tempdir().expect("tempdir");
    let user_layout = user_layout(tmp.path().join("home"));
    let system_layout = FsLayout::system(Some(tmp.path().join("system")));
    write_state(&user_layout, vec![component("user-tool")]);
    std::fs::create_dir_all(&system_layout.state_dir).expect("state dir");
    std::fs::write(
        system_layout.state_dir.join(INSTALLED_STATE_FILE),
        "not toml = [",
    )
    .expect("write malformed state");

    let view = StateView::from_layouts(
        "test",
        vec![
            (
                user_layout,
                RootSpec {
                    scope: StateScope::User,
                    writable: true,
                },
            ),
            (
                system_layout,
                RootSpec {
                    scope: StateScope::System,
                    writable: false,
                },
            ),
        ],
    )
    .expect("state view");

    assert_eq!(view.visible_roots.len(), 1);
    assert_eq!(view.warnings.len(), 1);
    assert!(view.warnings[0].contains("visible system state"));
}

#[test]
fn malformed_writable_state_is_fatal() {
    let tmp = tempdir().expect("tempdir");
    let user_layout = user_layout(tmp.path().join("home"));
    std::fs::create_dir_all(&user_layout.state_dir).expect("state dir");
    std::fs::write(
        user_layout.state_dir.join(INSTALLED_STATE_FILE),
        "not toml = [",
    )
    .expect("write malformed state");

    let err = StateView::from_layouts(
        "test",
        vec![(
            user_layout,
            RootSpec {
                scope: StateScope::User,
                writable: true,
            },
        )],
    )
    .expect_err("writable malformed state must fail");

    assert!(err.to_string().contains("failed to load installed state"));
}
