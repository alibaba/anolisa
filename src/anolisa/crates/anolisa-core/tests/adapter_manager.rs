//! End-to-end adapter manager tests driving a fake OpenClaw CLI.
//!
//! These exercise the full enable → status → disable lifecycle through the
//! real [`AdapterManager`] and [`OpenClawDriver`], using a shell script as
//! a stand-in for the `openclaw` binary. They cover the P3 acceptance
//! cases: install/list/uninstall success and failure, "CLI missing must
//! not clean up arbitrary paths", and forged-receipt rejection.
//!
//! The fake CLI is controlled entirely through the same env contract the
//! real driver uses (`OPENCLAW_BIN`, `OPENCLAW_HOME`, plus a test-only
//! `FAKE_OPENCLAW_FAIL` knob). Because those are process-global, every test
//! serializes on [`ENV_LOCK`].
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anolisa_core::adapter::AdapterError;
use anolisa_core::adapter::claim::{ClaimResourceKind, ClaimStatus};
use anolisa_core::adapter::driver::{AdapterSummary, ConditionStatus};
use anolisa_core::adapter::manager::{AdapterManager, EnableOutcome};
use anolisa_core::state::InstalledState;
use anolisa_platform::fs_layout::FsLayout;

/// Serializes the process-global env mutation across tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const COMPONENT: &str = "tokenless";
const FRAMEWORK: &str = "openclaw";

/// A staged test world: a prefix-rooted layout, openclaw home, fake CLI,
/// and a seeded `installed.toml`.
struct World {
    _root: tempfile::TempDir,
    layout: FsLayout,
    user_home: PathBuf,
    openclaw_home: PathBuf,
    fake_bin: PathBuf,
    resource_root: PathBuf,
}

impl World {
    fn manager(&self) -> AdapterManager {
        AdapterManager::new(
            self.layout.clone(),
            Some(self.user_home.clone()),
            "tester".to_string(),
        )
    }

    /// Apply this world's env contract. Must be called while holding
    /// [`ENV_LOCK`].
    fn apply_env(&self, fail: Option<&str>) {
        apply_openclaw_env(&self.fake_bin, &self.openclaw_home, fail);
    }

    fn load_state(&self) -> InstalledState {
        InstalledState::load(&self.layout.state_dir.join("installed.toml")).expect("load state")
    }
}

fn apply_openclaw_env(fake_bin: &Path, openclaw_home: &Path, fail: Option<&str>) {
    // SAFETY: callers hold ENV_LOCK, so no other test thread in this
    // binary reads these vars concurrently.
    unsafe {
        std::env::set_var("OPENCLAW_BIN", fake_bin);
        std::env::set_var("OPENCLAW_HOME", openclaw_home);
        match fail {
            Some(stage) => std::env::set_var("FAKE_OPENCLAW_FAIL", stage),
            None => std::env::remove_var("FAKE_OPENCLAW_FAIL"),
        }
    }
}

/// Build a fully staged world: layout under a temp prefix, an openclaw
/// home, a fake CLI, the adapter resource bundle, and a seeded state file
/// recording the component as installed.
fn stage() -> World {
    let root = tempfile::tempdir().expect("tempdir");
    let prefix = root.path().to_path_buf();
    let layout = FsLayout::system(Some(prefix.clone()));

    let user_home = prefix.join("home");
    std::fs::create_dir_all(&user_home).expect("home");

    let openclaw_home = prefix.join("openclaw-home");
    std::fs::create_dir_all(&openclaw_home).expect("openclaw home");

    // Adapter resource bundle with the same native manifest shape shipped by
    // tokenless' OpenClaw plugin.
    let resource_root = layout
        .datadir
        .join("adapters")
        .join(COMPONENT)
        .join(FRAMEWORK);
    std::fs::create_dir_all(&resource_root).expect("resource root");
    std::fs::write(
        resource_root.join("openclaw.plugin.json"),
        format!(r#"{{"id":"{COMPONENT}","name":"Tokenless"}}"#),
    )
    .expect("plugin manifest");

    let fake_bin = write_fake_openclaw(&prefix);
    seed_state(&layout, &prefix);

    World {
        _root: root,
        layout,
        user_home,
        openclaw_home,
        fake_bin,
        resource_root,
    }
}

/// Write a fake `openclaw` CLI honoring the driver's argv/env contract.
///
/// - `plugins install <root> ...` reads `<root>/openclaw.plugin.json` and
///   touches a marker in `$OPENCLAW_STATE_DIR/registry/<id>`.
/// - `plugins uninstall <id> ...` removes that marker.
/// - `plugins list` prints the registry markers, one id per line.
/// - `FAKE_OPENCLAW_FAIL=install|install_after_register|uninstall`
///   forces that verb to exit non-zero.
fn write_fake_openclaw(dir: &Path) -> PathBuf {
    let script = r#"#!/bin/sh
sub="$1"; action="$2"; arg3="$3"
reg="$OPENCLAW_STATE_DIR/registry"
mkdir -p "$reg" 2>/dev/null
if [ "$sub" != "plugins" ]; then echo "unknown subcommand: $sub" >&2; exit 2; fi
case "$action" in
  install)
    if [ "${FAKE_OPENCLAW_FAIL:-}" = "install" ]; then echo "boom-install" >&2; exit 7; fi
    id=$(sed -n 's/.*"id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$arg3/openclaw.plugin.json" | head -n 1)
    if [ -z "$id" ]; then echo "missing plugin id" >&2; exit 9; fi
    : > "$reg/$id"
    if [ "${FAKE_OPENCLAW_FAIL:-}" = "install_after_register" ]; then echo "boom-after-register" >&2; exit 10; fi
    echo "installed $id"
    ;;
  uninstall)
    if [ "${FAKE_OPENCLAW_FAIL:-}" = "uninstall" ]; then echo "boom-uninstall" >&2; exit 8; fi
    rm -f "$reg/$arg3"
    echo "uninstalled $arg3"
    ;;
  list)
    ls "$reg" 2>/dev/null || true
    ;;
  *)
    echo "unknown action: $action" >&2; exit 2 ;;
esac
exit 0
"#;
    let path = dir.join("openclaw");
    std::fs::write(&path, script).expect("write fake cli");
    let mut perms = std::fs::metadata(&path).expect("meta").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

/// Seed `installed.toml` with the component recorded as installed so
/// `enable`'s precondition passes.
fn seed_state(layout: &FsLayout, prefix: &Path) {
    let state_path = layout.state_dir.join("installed.toml");
    std::fs::create_dir_all(state_path.parent().unwrap()).expect("state dir");
    let toml = format!(
        r#"schema_version = 2
updated_at = "2026-06-15T00:00:00Z"
install_mode = "system"
prefix = "{prefix}"
anolisa_version = "0.1.7"

[[objects]]
kind = "component"
name = "{COMPONENT}"
version = "0.1.0"
status = "installed"
installed_at = "2026-06-15T00:00:00Z"
"#,
        prefix = prefix.display(),
    );
    std::fs::write(&state_path, toml).expect("seed state");
    write_installed_manifest(layout, FRAMEWORK);
}

fn write_installed_manifest(layout: &FsLayout, framework: &str) {
    let manifest_path = layout
        .state_dir
        .join("component-manifests")
        .join(COMPONENT)
        .join("component.toml");
    std::fs::create_dir_all(manifest_path.parent().unwrap()).expect("manifest dir");
    std::fs::write(
        manifest_path,
        format!(
            r#"[component]
name = "{COMPONENT}"
version = "0.1.0"

[component.layout]
modes = ["system"]

[[adapters]]
framework = "{framework}"
source = "adapters/{COMPONENT}/{framework}"
dest = "{{datadir}}/adapters/{{component}}/{framework}/"
"#
        ),
    )
    .expect("seed component manifest");
}

#[test]
fn enable_status_disable_happy_path() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let world = stage();
    world.apply_env(None);
    let manager = world.manager();

    // enable
    let outcome = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let claim = match outcome {
        EnableOutcome::Enabled(c) => *c,
        EnableOutcome::Planned(_) => panic!("expected enabled, got plan"),
    };
    assert_eq!(claim.component, COMPONENT);
    assert_eq!(claim.framework, FRAMEWORK);
    assert_eq!(claim.plugin_id.as_deref(), Some(COMPONENT));
    assert_eq!(claim.status, ClaimStatus::Enabled);
    // Receipt records the external home + the plugin, no owned paths.
    assert!(claim.resources.iter().any(|r| matches!(
        &r.kind,
        ClaimResourceKind::FrameworkPlugin { plugin_id, .. } if plugin_id == COMPONENT
    )));

    // Persisted to state.
    let state = world.load_state();
    assert!(state.find_adapter_claim(COMPONENT, FRAMEWORK).is_some());

    // The framework CLI invocation reached the central log.
    let log = std::fs::read_to_string(&world.layout.central_log).expect("central log");
    assert!(
        log.contains("framework cli"),
        "central log should record the CLI invocation: {log}"
    );

    // status → healthy (framework detected + plugin registered).
    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries.len(), 1);
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Healthy);
    // The plugin-registered condition must be verified True.
    assert!(status.entries[0].report.conditions.iter().any(|c| matches!(
        c.kind,
        anolisa_core::adapter::driver::AdapterConditionKind::PluginRegistered
    ) && c.status
        == ConditionStatus::True));

    // disable → removes receipt.
    let disabled = manager
        .disable(COMPONENT, Some(FRAMEWORK))
        .expect("disable");
    assert!(disabled.claim_removed);
    assert!(disabled.report.cleanup_complete);
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_none(),
        "receipt must be gone after successful disable"
    );
}

#[test]
fn user_layout_enable_accepts_system_installed_component() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().expect("tempdir");
    let prefix = root.path().to_path_buf();
    let system_prefix = prefix.join("system");
    let system_layout = FsLayout::system(Some(system_prefix.clone()));
    let user_home = prefix.join("home");
    std::fs::create_dir_all(&user_home).expect("home");
    let user_layout = FsLayout::user(user_home.clone());

    let openclaw_home = prefix.join("openclaw-home");
    std::fs::create_dir_all(&openclaw_home).expect("openclaw home");
    let resource_root = system_layout
        .datadir
        .join("adapters")
        .join(COMPONENT)
        .join(FRAMEWORK);
    std::fs::create_dir_all(&resource_root).expect("resource root");
    std::fs::write(
        resource_root.join("openclaw.plugin.json"),
        format!(r#"{{"id":"{COMPONENT}","name":"Tokenless"}}"#),
    )
    .expect("plugin manifest");
    seed_state(&system_layout, &system_prefix);
    let fake_bin = write_fake_openclaw(&prefix);
    apply_openclaw_env(&fake_bin, &openclaw_home, None);

    let mut manager =
        AdapterManager::new(user_layout.clone(), Some(user_home), "tester".to_string());
    manager.push_visible_root(anolisa_core::adapter::manager::VisibleRoot {
        state_dir: system_layout.state_dir.clone(),
        contract_datadir_roots: vec![system_layout.datadir.clone()],
    });

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable system component from user layout");

    let user_state =
        InstalledState::load(&user_layout.state_dir.join("installed.toml")).expect("user state");
    assert!(
        user_state
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_some(),
        "receipt is written to the invoking user's state"
    );
    let system_state = InstalledState::load(&system_layout.state_dir.join("installed.toml"))
        .expect("system state");
    assert!(
        system_state
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_none(),
        "system install state is read as a source, not used for user receipts"
    );
}

#[test]
fn enable_rejects_resource_directory_not_declared_by_manifest() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let world = stage();
    write_installed_manifest(&world.layout, "hermes");
    world.apply_env(None);
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("directory discovery alone must not authorize enable");
    assert!(
        matches!(err, AdapterError::AdapterNotDeclared { .. }),
        "got {err:?}"
    );
    assert!(
        !world
            .openclaw_home
            .join("registry")
            .join(COMPONENT)
            .exists(),
        "framework driver must not run when manifest does not declare it"
    );
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_none(),
        "no receipt should be created for an undeclared adapter"
    );
}

#[test]
fn failed_enable_keeps_cleanup_receipt_for_retry() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let world = stage();
    world.apply_env(Some("install"));
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("install failure must surface");
    assert!(
        matches!(err, AdapterError::FrameworkCli { .. }),
        "got {err:?}"
    );

    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("failed enable receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

#[test]
fn failed_enable_after_framework_side_effect_keeps_visible_receipt() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let world = stage();
    world.apply_env(Some("install_after_register"));
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("install failure must surface");
    assert!(
        matches!(err, AdapterError::FrameworkCli { .. }),
        "got {err:?}"
    );

    assert!(
        world
            .openclaw_home
            .join("registry")
            .join(COMPONENT)
            .exists(),
        "fake framework registered the plugin before returning failure"
    );
    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("receipt must remain visible for disable/status");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

#[test]
fn dry_run_enable_does_not_register_or_persist() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let world = stage();
    world.apply_env(None);
    let manager = world.manager();

    let outcome = manager
        .enable(COMPONENT, Some(FRAMEWORK), true)
        .expect("dry-run enable");
    match outcome {
        EnableOutcome::Planned(plan) => {
            assert_eq!(plan.component, COMPONENT);
            assert!(plan.register_command.is_some());
        }
        EnableOutcome::Enabled(_) => panic!("dry-run must not enable"),
    }

    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_none(),
        "dry-run must not persist a receipt"
    );
    // Nothing should have been written into the openclaw registry.
    assert!(
        !world
            .openclaw_home
            .join("registry")
            .join(COMPONENT)
            .exists()
    );
}

#[test]
fn disable_keeps_receipt_when_uninstall_fails() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let world = stage();
    world.apply_env(None);
    let manager = world.manager();
    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");

    // Now force uninstall to fail.
    world.apply_env(Some("uninstall"));
    let disabled = manager
        .disable(COMPONENT, Some(FRAMEWORK))
        .expect("disable runs");
    assert!(
        !disabled.claim_removed,
        "receipt must be kept on cleanup failure"
    );
    assert!(!disabled.report.cleanup_complete);

    // Receipt is kept and marked cleanup_failed for retry.
    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

#[test]
fn disable_without_cli_keeps_receipt_for_retry() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let world = stage();
    world.apply_env(None);
    let manager = world.manager();
    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");

    // Point OPENCLAW_BIN at a path that does not exist: disable cannot run
    // the CLI, so it must keep the receipt for a later retry instead of
    // pretending cleanup completed.
    let missing = world._root.path().join("no-such-openclaw");
    // SAFETY: ENV_LOCK held.
    unsafe {
        std::env::set_var("OPENCLAW_BIN", &missing);
    }
    let disabled = manager
        .disable(COMPONENT, Some(FRAMEWORK))
        .expect("disable");
    assert!(!disabled.claim_removed, "receipt kept when CLI absent");
    assert!(!disabled.report.cleanup_complete);
    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

#[test]
fn forged_external_path_receipt_is_rejected_by_status() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let world = stage();
    world.apply_env(None);
    let manager = world.manager();
    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");

    // Tamper with the persisted receipt: repoint the external-path resource
    // at /etc, outside the driver's allowed roots.
    let state_path = world.layout.state_dir.join("installed.toml");
    let mut state = world.load_state();
    {
        let claim = state
            .adapter_claims
            .iter_mut()
            .find(|c| c.component == COMPONENT)
            .expect("claim");
        for res in &mut claim.resources {
            if let ClaimResourceKind::ExternalPath { path } = &mut res.kind {
                *path = PathBuf::from("/etc/cron.d/evil");
            }
        }
    }
    state.save(&state_path).expect("save tampered state");

    let err = manager
        .status(Some(COMPONENT))
        .expect_err("forged receipt must be rejected");
    assert!(
        matches!(err, AdapterError::ClaimValidation(_)),
        "got {err:?}"
    );
}

#[test]
fn scan_includes_manifest_declaration_without_resource_directory() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().expect("tempdir");
    let prefix = root.path().to_path_buf();
    let layout = FsLayout::system(Some(prefix.clone()));
    seed_state(&layout, &prefix);
    let manager = AdapterManager::new(
        layout.clone(),
        Some(prefix.join("home")),
        "tester".to_string(),
    );

    let report = manager.scan().expect("scan");
    let entry = report
        .entries
        .iter()
        .find(|e| e.component == COMPONENT && e.framework == FRAMEWORK)
        .expect("manifest declaration entry");
    assert!(entry.declared);
    assert!(entry.resource_root.is_none());
    assert!(entry.driver_available);
    assert!(!entry.enabled);
}

#[test]
fn user_scan_includes_system_state_declaration() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().expect("tempdir");
    let prefix = root.path().to_path_buf();
    let system_prefix = prefix.join("system");
    let system_layout = FsLayout::system(Some(system_prefix.clone()));
    seed_state(&system_layout, &system_prefix);

    let user_home = prefix.join("home");
    std::fs::create_dir_all(&user_home).expect("home");
    let user_layout = FsLayout::user(user_home.clone());
    let mut manager = AdapterManager::new(user_layout, Some(user_home), "tester".to_string());
    manager.push_visible_root(anolisa_core::adapter::manager::VisibleRoot {
        state_dir: system_layout.state_dir.clone(),
        contract_datadir_roots: vec![system_layout.datadir.clone()],
    });

    let report = manager.scan().expect("scan");
    let entry = report
        .entries
        .iter()
        .find(|e| e.component == COMPONENT && e.framework == FRAMEWORK)
        .expect("system declaration entry");
    assert!(entry.declared);
    assert!(entry.resource_root.is_none());
    assert!(!entry.enabled);
}

#[test]
fn scan_lists_resource_with_detection_and_receipt_state() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let world = stage();
    world.apply_env(None);
    let manager = world.manager();

    // Before enable: discovered, driver available, detected, not enabled.
    let report = manager.scan().expect("scan");
    let entry = report
        .entries
        .iter()
        .find(|e| e.component == COMPONENT && e.framework == FRAMEWORK)
        .expect("entry");
    assert!(entry.driver_available);
    assert!(entry.framework_detected);
    assert!(!entry.enabled);
    assert!(entry.declared);
    assert_eq!(entry.resource_root.as_ref(), Some(&world.resource_root));

    // After enable: reported as enabled.
    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let report = manager.scan().expect("scan again");
    let entry = report
        .entries
        .iter()
        .find(|e| e.component == COMPONENT)
        .expect("entry");
    assert!(entry.enabled);
    assert_eq!(entry.claim_status, Some(ClaimStatus::Enabled));
}
