//! Tests for the `VisibleInstalledView` merge model.

use std::path::PathBuf;

use anolisa_core::state::{InstalledObject, InstalledState, ObjectKind, ObjectStatus, Ownership};

use crate::commands::visible_view::{
    MutationOperation, MutationTarget, Scope, VisibleInstalledView, resolve_mutation_target,
};
use crate::context::{CliContext, InstallMode};

fn user_ctx() -> CliContext {
    CliContext {
        install_mode: InstallMode::User,
        prefix: None,
        json: false,
        dry_run: false,
        verbose: false,
        quiet: true,
        no_color: true,
    }
}

fn system_ctx() -> CliContext {
    CliContext {
        install_mode: InstallMode::System,
        prefix: None,
        json: false,
        dry_run: false,
        verbose: false,
        quiet: true,
        no_color: true,
    }
}

fn component_object(name: &str) -> InstalledObject {
    InstalledObject {
        kind: ObjectKind::Component,
        name: name.to_string(),
        version: "0.1.0".to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("raw".to_string()),
        ownership: Some(Ownership::RawManaged),
        rpm_metadata: None,
        installed_at: "2026-07-01T00:00:00Z".to_string(),
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
    }
}

fn state_with(name: &str) -> InstalledState {
    let mut state = InstalledState::default();
    state.upsert_object(component_object(name));
    state
}

const USER_PATH: &str = "/test/user/installed.toml";
const SYSTEM_PATH: &str = "/test/system/installed.toml";

fn view(
    user: &InstalledState,
    system: &InstalledState,
    system_readable: bool,
    ctx: &CliContext,
) -> VisibleInstalledView {
    VisibleInstalledView::from_states(
        user,
        system,
        PathBuf::from(USER_PATH),
        PathBuf::from(SYSTEM_PATH),
        system_readable,
        ctx,
    )
}

/// Test 1: Only user state exists — records contain only user scope, all active.
#[test]
fn only_user_state_exists() {
    let user = state_with("tokenless");
    let system = InstalledState::default();
    let v = view(&user, &system, false, &user_ctx());

    assert_eq!(v.records().len(), 1);
    let rec = &v.records()[0];
    assert_eq!(rec.component, "tokenless");
    assert_eq!(rec.scope, Scope::User);
    assert!(rec.active);
    assert_eq!(rec.shadowed_by, None);
    assert!(rec.mutable_by_current_user);
    assert_eq!(rec.state_path, PathBuf::from(USER_PATH));
    assert!(!v.system_state_readable());
}

/// Test 2: Only system state exists (user mode ctx) — system scope, active,
/// mutable_by_current_user = false.
#[test]
fn only_system_state_user_mode_ctx() {
    let user = InstalledState::default();
    let system = state_with("ws-ckpt");
    let v = view(&user, &system, true, &user_ctx());

    assert_eq!(v.records().len(), 1);
    let rec = &v.records()[0];
    assert_eq!(rec.component, "ws-ckpt");
    assert_eq!(rec.scope, Scope::System);
    assert!(rec.active);
    assert_eq!(rec.shadowed_by, None);
    assert!(!rec.mutable_by_current_user);
    assert!(v.system_state_readable());
}

/// Test 3: User + system same component — user is active, system is shadowed.
#[test]
fn user_and_system_same_component() {
    let user = state_with("tokenless");
    let system = state_with("tokenless");
    let v = view(&user, &system, true, &user_ctx());

    assert_eq!(v.records().len(), 2);

    let user_rec = v.active("tokenless").expect("active record");
    assert_eq!(user_rec.scope, Scope::User);
    assert!(user_rec.active);
    assert_eq!(user_rec.shadowed_by, None);

    let records = v.records_for("tokenless");
    assert_eq!(records.len(), 2);
    let shadowed = records.iter().find(|r| !r.active).expect("shadowed record");
    assert_eq!(shadowed.scope, Scope::System);
    assert_eq!(shadowed.shadowed_by, Some(Scope::User));
}

/// Test 4: System state not readable — view has only user records, no panic.
#[test]
fn system_state_not_readable() {
    let user = state_with("tokenless");
    let system = InstalledState::default();
    let v = view(&user, &system, false, &user_ctx());

    assert!(!v.system_state_readable());
    assert!(v.system_state_path().is_none());
    assert_eq!(v.records().len(), 1);
    assert_eq!(v.records()[0].scope, Scope::User);
}

/// Test 5: System state file does not exist — equivalent to empty system state.
#[test]
fn system_state_file_missing() {
    let user = state_with("tokenless");
    let system = InstalledState::default();
    let v = view(&user, &system, false, &user_ctx());

    assert!(!v.system_state_readable());
    assert_eq!(v.records().len(), 1);
}

/// Test 6: mutable_by_current_user in system mode — system record is mutable,
/// user record is not.
#[test]
fn mutable_by_current_user_system_mode() {
    let user = state_with("tokenless");
    let system = state_with("agentsight");
    let v = view(&user, &system, true, &system_ctx());

    let user_rec = v.records().iter().find(|r| r.scope == Scope::User).unwrap();
    assert!(!user_rec.mutable_by_current_user);

    let sys_rec = v
        .records()
        .iter()
        .find(|r| r.scope == Scope::System)
        .unwrap();
    assert!(sys_rec.mutable_by_current_user);
}

/// Test 7: active() returns None for unknown component.
#[test]
fn active_returns_none_for_unknown() {
    let user = InstalledState::default();
    let system = InstalledState::default();
    let v = view(&user, &system, false, &user_ctx());

    assert!(v.active("nonexistent").is_none());
}

/// Test 8: records_for returns active first, then shadowed.
#[test]
fn records_for_orders_active_first() {
    let user = state_with("tokenless");
    let system = state_with("tokenless");
    let v = view(&user, &system, true, &user_ctx());

    let records = v.records_for("tokenless");
    assert_eq!(records.len(), 2);
    assert!(records[0].active);
    assert!(!records[1].active);
}

/// Test 9: Multiple components across scopes.
#[test]
fn multiple_components_across_scopes() {
    let mut user = InstalledState::default();
    user.upsert_object(component_object("tokenless"));
    let mut system = InstalledState::default();
    system.upsert_object(component_object("agentsight"));
    system.upsert_object(component_object("os-skills"));

    let v = view(&user, &system, true, &user_ctx());

    assert_eq!(v.records().len(), 3);
    let active: Vec<_> = v.active_records().collect();
    assert_eq!(active.len(), 3);
    assert!(
        active
            .iter()
            .any(|r| r.component == "tokenless" && r.scope == Scope::User)
    );
    assert!(
        active
            .iter()
            .any(|r| r.component == "agentsight" && r.scope == Scope::System)
    );
    assert!(
        active
            .iter()
            .any(|r| r.component == "os-skills" && r.scope == Scope::System)
    );
}

// ── resolve_mutation_target tests ────────────────────────────────────

#[test]
fn mutation_target_current_scope_when_component_only_in_user_scope() {
    let user = state_with("tokenless");
    let v = view(&user, &InstalledState::default(), true, &user_ctx());

    let target = resolve_mutation_target(MutationOperation::Uninstall, "tokenless", &v);
    assert!(matches!(target, MutationTarget::CurrentScope(_)));
}

#[test]
fn mutation_target_wrong_scope_when_user_mode_views_system_only() {
    let system = state_with("tokenless");
    let v = view(&InstalledState::default(), &system, true, &user_ctx());

    let target = resolve_mutation_target(MutationOperation::Uninstall, "tokenless", &v);
    assert!(matches!(target, MutationTarget::WrongScope(_)));
}

#[test]
fn mutation_target_create_when_component_absent() {
    let v = view(
        &InstalledState::default(),
        &InstalledState::default(),
        true,
        &user_ctx(),
    );

    let target = resolve_mutation_target(MutationOperation::Install, "tokenless", &v);
    assert!(matches!(target, MutationTarget::CreateInCurrentScope));
}

/// When the same component exists in both user and system scope,
/// the active record is the user one. A system-mode user should still
/// resolve to `CurrentScope` — the system record is mutable by them,
/// even though it is shadowed.
#[test]
fn mutation_target_current_scope_for_system_mode_with_shadowed_record() {
    let user = state_with("tokenless");
    let system = state_with("tokenless");
    let v = view(&user, &system, true, &system_ctx());

    let target = resolve_mutation_target(MutationOperation::Uninstall, "tokenless", &v);
    match target {
        MutationTarget::CurrentScope(rec) => {
            assert_eq!(rec.scope, Scope::System);
            assert!(!rec.active);
        }
        other => panic!("expected CurrentScope for system-mode, got {other:?}"),
    }
}

/// Conversely, a user-mode user with the same dual-scope setup should
/// resolve to the active user record.
#[test]
fn mutation_target_current_scope_for_user_mode_with_dual_scope() {
    let user = state_with("tokenless");
    let system = state_with("tokenless");
    let v = view(&user, &system, true, &user_ctx());

    let target = resolve_mutation_target(MutationOperation::Forget, "tokenless", &v);
    match target {
        MutationTarget::CurrentScope(rec) => {
            assert_eq!(rec.scope, Scope::User);
            assert!(rec.active);
        }
        other => panic!("expected CurrentScope for user-mode, got {other:?}"),
    }
}
