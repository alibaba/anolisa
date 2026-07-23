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
//! serializes on [`ENV_LOCK`], starts from a clean env contract, and
//! restores the prior environment on exit.
#![cfg(unix)]

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anolisa_core::adapter::AdapterError;
use anolisa_core::adapter::claim::{
    ClaimResourceKind, ClaimStatus, ConfigApplyState, DriverPayload,
};
use anolisa_core::adapter::driver::{AdapterSummary, ConditionStatus};
use anolisa_core::adapter::manager::{AdapterManager, EnableOptions, EnableOutcome};
use anolisa_core::state::InstallMode as StateInstallMode;
use anolisa_core::state_store::StateStore;
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

    /// Apply this world's env contract through the process-env guard.
    fn apply_env(&self, guard: &OpenClawEnvGuard, fail: Option<&str>) {
        guard.apply(&self.fake_bin, &self.openclaw_home, fail);
    }

    fn load_state(&self) -> StateStore {
        load_state_at(&self.layout.state_dir.join("installed.toml"))
    }

    /// Path the fake CLI appends each invocation's argv to (test-only).
    fn argv_log(&self) -> PathBuf {
        self.openclaw_home
            .parent()
            .expect("prefix")
            .join("argv.log")
    }

    /// Whether the openclaw registry marker for the component exists.
    fn registry_marker_exists(&self) -> bool {
        self.openclaw_home.join("registry").join(COMPONENT).exists()
    }

    fn config_marker_exists(&self, key: &str) -> bool {
        self.openclaw_home.join("config").join(key).exists()
    }

    fn has_claim(&self) -> bool {
        self.load_state()
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_some()
    }
}

/// Lines the fake CLI recorded, in invocation order (empty when unset/absent).
fn argv_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The real `plugins install` invocation (excluding the `--help` probe).
fn install_argv(lines: &[String]) -> Option<&String> {
    lines
        .iter()
        .find(|l| l.starts_with("plugins install ") && !l.contains("--help"))
}

/// The real `plugins inspect` invocation (excluding the `--help` probe).
fn inspect_argv(lines: &[String]) -> Option<&String> {
    lines
        .iter()
        .find(|l| l.starts_with("plugins inspect ") && !l.contains("--help"))
}

/// Overwrite the component's installed manifest with a custom `[[adapters]]`
/// block, keeping the component recorded as installed. `adapters_block` is a
/// substituted string, so `{datadir}`/`{component}` placeholders inside it
/// reach the manifest verbatim.
fn write_openclaw_manifest(layout: &FsLayout, adapters_block: &str) {
    let manifest_path = layout
        .state_dir
        .join("component-manifests")
        .join(COMPONENT)
        .join("component.toml");
    std::fs::create_dir_all(manifest_path.parent().unwrap()).expect("manifest dir");
    let toml = format!(
        r#"[component]
name = "{COMPONENT}"
version = "0.1.0"

[component.layout]
modes = ["system"]

{adapters_block}
"#
    );
    std::fs::write(&manifest_path, toml).expect("seed component manifest");
}

/// A plain OpenClaw plugin adapter block with an optional adapter-level
/// framework version requirement.
fn plugin_adapter_block(compat_req: Option<&str>) -> String {
    let compat = compat_req
        .map(|r| format!("\n[adapters.compat]\nframework_version = \"{r}\"\n"))
        .unwrap_or_default();
    format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"
{compat}"#
    )
}

/// Every environment variable this test binary's fake OpenClaw contract
/// owns. The guard saves and restores exactly these so no test leaks state
/// into another. `FAKE_OC_*` are the capability knobs the fake CLI reads.
const OWNED_ENV: &[&str] = &[
    "OPENCLAW_BIN",
    "OPENCLAW_HOME",
    "FAKE_OPENCLAW_FAIL",
    "FAKE_OC_VERSION",
    "FAKE_OC_INSTALL_FORCE",
    "FAKE_OC_INSTALL_UNSAFE",
    "FAKE_OC_INSTALL_UNSAFE_NOOP",
    "FAKE_OC_INSPECT_JSON",
    "FAKE_OC_INSPECT_RUNTIME",
    "FAKE_OC_RUNTIME_STATUS",
    "FAKE_OC_INSPECT_DIAG",
    "FAKE_OC_ARGV_LOG",
    "FAKE_OC_PROBE_FAIL",
    "FAKE_OC_VERSION_PREAMBLE",
    "FAKE_OC_CONFIG_FAIL_KEY",
    "FAKE_OC_CONFIG_FAIL_AFTER_KEY",
];

struct OpenClawEnvGuard {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl OpenClawEnvGuard {
    fn acquire() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = OWNED_ENV
            .iter()
            .map(|&k| (k, std::env::var_os(k)))
            .collect();
        let guard = Self { _lock: lock, saved };
        guard.clear();
        guard
    }

    fn clear(&self) {
        // SAFETY: this guard holds ENV_LOCK, so tests in this binary cannot
        // observe a half-mutated OpenClaw env contract.
        unsafe {
            for &key in OWNED_ENV {
                std::env::remove_var(key);
            }
        }
    }

    fn apply(&self, fake_bin: &Path, openclaw_home: &Path, fail: Option<&str>) {
        // SAFETY: this guard holds ENV_LOCK, so no other test thread in this
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

    fn set_openclaw_bin(&self, value: &Path) {
        // SAFETY: this guard holds ENV_LOCK.
        unsafe {
            std::env::set_var("OPENCLAW_BIN", value);
        }
    }

    /// Set one of the owned fake-CLI knobs (or `OsStr`-valued path).
    fn set(&self, key: &str, value: impl AsRef<std::ffi::OsStr>) {
        assert!(
            OWNED_ENV.contains(&key),
            "env key {key} must be guard-owned"
        );
        // SAFETY: this guard holds ENV_LOCK.
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

impl Drop for OpenClawEnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            restore_env(key, value.as_ref());
        }
    }
}

fn restore_env(key: &str, value: Option<&OsString>) {
    // SAFETY: callers hold ENV_LOCK until after the saved values are restored.
    unsafe {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
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
/// Read-only probes, defaulting to a modern, force-capable, JSON-capable host:
/// - `--version` prints `openclaw $FAKE_OC_VERSION` (default `2026.4.14`) and
///   creates NO registry/config/state.
/// - `plugins install --help` lists `--force` unless `FAKE_OC_INSTALL_FORCE=0`
///   and `--dangerously-force-unsafe-install` when `FAKE_OC_INSTALL_UNSAFE=1`;
///   `FAKE_OC_INSTALL_UNSAFE_NOOP=1` marks that option as a deprecated no-op.
/// - `plugins inspect --help` lists `--json` unless `FAKE_OC_INSPECT_JSON=0`
///   and `--runtime` when `FAKE_OC_INSPECT_RUNTIME=1`.
///
/// Mutations / runtime state:
/// - `plugins install <root> ...` reads `<root>/openclaw.plugin.json` and
///   touches a marker in `$OPENCLAW_STATE_DIR/registry/<id>`.
/// - `plugins inspect <id> [--runtime] --json` prints an optional legacy
///   diagnostic line (when `FAKE_OC_INSPECT_DIAG` is set) followed by the JSON
///   `{"plugin":{"id":..,"status":"$FAKE_OC_RUNTIME_STATUS"}}` (default
///   `loaded`).
/// - `plugins uninstall <id> ...` removes the marker; `plugins list` prints
///   markers; `config set` echoes.
/// - `FAKE_OPENCLAW_FAIL=install|install_after_register|uninstall` forces that
///   verb to exit non-zero; `FAKE_OC_CONFIG_FAIL_KEY` fails `config set`
///   before mutation, while `FAKE_OC_CONFIG_FAIL_AFTER_KEY` fails after
///   writing a marker for one exact key.
///
/// When `FAKE_OC_ARGV_LOG` names a file, every invocation appends its full
/// argv (one line) — test instrumentation, not OpenClaw state.
fn write_fake_openclaw(dir: &Path) -> PathBuf {
    let script = r#"#!/bin/sh
if [ -n "${FAKE_OC_ARGV_LOG:-}" ]; then printf '%s\n' "$*" >> "$FAKE_OC_ARGV_LOG"; fi

ver="${FAKE_OC_VERSION:-2026.4.14}"
if [ "$1" = "--version" ]; then
  [ -n "${FAKE_OC_VERSION_PREAMBLE:-}" ] && echo "$FAKE_OC_VERSION_PREAMBLE"
  echo "openclaw $ver"
  [ "${FAKE_OC_PROBE_FAIL:-}" = "version" ] && exit 3
  exit 0
fi

sub="$1"; action="$2"; arg3="$3"

if [ "$sub" = "config" ] && [ "$action" = "set" ]; then
  if [ -n "${FAKE_OC_CONFIG_FAIL_KEY:-}" ] && [ "$arg3" = "$FAKE_OC_CONFIG_FAIL_KEY" ]; then
    echo "boom-config-$arg3" >&2
    exit 13
  fi
  config_dir="$OPENCLAW_STATE_DIR/config"; mkdir -p "$config_dir" 2>/dev/null
  printf '%s' "$4" > "$config_dir/$arg3"
  if [ -n "${FAKE_OC_CONFIG_FAIL_AFTER_KEY:-}" ] && [ "$arg3" = "$FAKE_OC_CONFIG_FAIL_AFTER_KEY" ]; then
    echo "boom-after-config-$arg3" >&2
    exit 14
  fi
  echo "config set $arg3 $4"
  exit 0
fi
if [ "$sub" != "plugins" ]; then echo "unknown subcommand: $sub" >&2; exit 2; fi

case "$action" in
  install)
    if [ "$arg3" = "--help" ]; then
      echo "Usage: openclaw plugins install <path> [options]"
      [ "${FAKE_OC_INSTALL_FORCE:-1}" = "1" ] && echo "  --force                             overwrite an existing plugin"
      if [ "${FAKE_OC_INSTALL_UNSAFE:-0}" = "1" ]; then
        if [ "${FAKE_OC_INSTALL_UNSAFE_NOOP:-0}" = "1" ]; then
          echo "  --dangerously-force-unsafe-install  Deprecated no-op; security.installPolicy may still block"
        else
          echo "  --dangerously-force-unsafe-install  bypass plugin safety checks"
        fi
      fi
      [ "${FAKE_OC_PROBE_FAIL:-}" = "install_help" ] && exit 4
      exit 0
    fi
    reg="$OPENCLAW_STATE_DIR/registry"; mkdir -p "$reg" 2>/dev/null
    if [ "${FAKE_OPENCLAW_FAIL:-}" = "install" ]; then echo "boom-install" >&2; exit 7; fi
    if [ "${FAKE_OPENCLAW_FAIL:-}" = "install_unsafe_policy" ]; then
      echo "refusing install: plugin failed safety checks (pass --dangerously-force-unsafe-install to override)" >&2
      exit 11
    fi
    if [ "${FAKE_OPENCLAW_FAIL:-}" = "install_unsafe_policy_stdout" ]; then
      echo "SECURITY FINDING: plugin failed safety review; pass --dangerously-force-unsafe-install to override"
      exit 12
    fi
    id=$(sed -n 's/.*"id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$arg3/openclaw.plugin.json" | head -n 1)
    if [ -z "$id" ]; then echo "missing plugin id" >&2; exit 9; fi
    : > "$reg/$id"
    if [ "${FAKE_OPENCLAW_FAIL:-}" = "install_after_register" ]; then echo "boom-after-register" >&2; exit 10; fi
    echo "installed $id"
    ;;
  inspect)
    if [ "$arg3" = "--help" ]; then
      echo "Usage: openclaw plugins inspect <id> [options]"
      [ "${FAKE_OC_INSPECT_JSON:-1}" = "1" ] && echo "  --json      machine-readable output"
      [ "${FAKE_OC_INSPECT_RUNTIME:-0}" = "1" ] && echo "  --runtime   include live runtime status"
      [ "${FAKE_OC_PROBE_FAIL:-}" = "inspect_help" ] && exit 5
      exit 0
    fi
    status="${FAKE_OC_RUNTIME_STATUS:-loaded}"
    [ -n "${FAKE_OC_INSPECT_DIAG:-}" ] && echo "legacy: reading plugin registry for $arg3 ..."
    echo "{\"plugin\":{\"id\":\"$arg3\",\"status\":\"$status\"}}"
    ;;
  uninstall)
    reg="$OPENCLAW_STATE_DIR/registry"; mkdir -p "$reg" 2>/dev/null
    if [ "${FAKE_OPENCLAW_FAIL:-}" = "uninstall" ]; then echo "boom-uninstall" >&2; exit 8; fi
    rm -f "$reg/$arg3"
    echo "uninstalled $arg3"
    ;;
  list)
    reg="$OPENCLAW_STATE_DIR/registry"
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
fn load_state_at(path: &Path) -> StateStore {
    StateStore::load(path, anolisa_platform::privilege::effective_uid()).expect("load state")
}

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
install_backend = "raw"
ownership = "raw_managed"
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
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
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
        .disable(COMPONENT, Some(FRAMEWORK), false)
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
    let guard = OpenClawEnvGuard::acquire();
    let root = tempfile::tempdir().expect("tempdir");
    let prefix = root.path().to_path_buf();
    let system_prefix = prefix.join("system");
    let system_layout = FsLayout::system(Some(system_prefix.clone()));
    let user_home = prefix.join("home");
    std::fs::create_dir_all(&user_home).expect("home");
    let user_layout =
        FsLayout::user_with_overrides(user_home.clone(), None, None, None, None, None);

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
    guard.apply(&fake_bin, &openclaw_home, None);

    let mut manager =
        AdapterManager::new(user_layout.clone(), Some(user_home), "tester".to_string());
    manager.push_visible_root(anolisa_core::adapter::manager::VisibleRoot {
        state_dir: system_layout.state_dir.clone(),
        contract_datadir_roots: vec![system_layout.datadir.clone()],
    });

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable system component from user layout");

    let user_state = load_state_at(&user_layout.state_dir.join("installed.toml"));
    assert_eq!(user_state.install_mode, StateInstallMode::User);
    assert_eq!(user_state.prefix, user_layout.prefix);
    assert!(
        user_state
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_some(),
        "receipt is written to the invoking user's state"
    );
    let system_state = load_state_at(&system_layout.state_dir.join("installed.toml"));
    assert!(
        system_state
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_none(),
        "system install state is read as a source, not used for user receipts"
    );
}

#[test]
fn enable_rejects_resource_directory_not_declared_by_manifest() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    write_installed_manifest(&world.layout, "hermes");
    world.apply_env(&guard, None);
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
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, Some("install"));
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
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, Some("install_after_register"));
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
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
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
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    let manager = world.manager();
    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");

    // Now force uninstall to fail.
    world.apply_env(&guard, Some("uninstall"));
    let disabled = manager
        .disable(COMPONENT, Some(FRAMEWORK), false)
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
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    let manager = world.manager();
    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");

    // Point OPENCLAW_BIN at a path that does not exist: disable cannot run
    // the CLI, so it must keep the receipt for a later retry instead of
    // pretending cleanup completed.
    let missing = world._root.path().join("no-such-openclaw");
    guard.set_openclaw_bin(&missing);
    let disabled = manager
        .disable(COMPONENT, Some(FRAMEWORK), false)
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
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
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
    let _guard = OpenClawEnvGuard::acquire();
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
    let _guard = OpenClawEnvGuard::acquire();
    let root = tempfile::tempdir().expect("tempdir");
    let prefix = root.path().to_path_buf();
    let system_prefix = prefix.join("system");
    let system_layout = FsLayout::system(Some(system_prefix.clone()));
    seed_state(&system_layout, &system_prefix);

    let user_home = prefix.join("home");
    std::fs::create_dir_all(&user_home).expect("home");
    let user_layout =
        FsLayout::user_with_overrides(user_home.clone(), None, None, None, None, None);
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
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
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

// ---------------------------------------------------------------------------
// dry-run disable regression tests (#1251)
// ---------------------------------------------------------------------------

/// Dry-run disable must leave `InstalledState` completely unchanged and
/// must not invoke framework CLI operations. A following real disable
/// must still clean up exactly once.
#[test]
fn dry_run_disable_leaves_state_unchanged() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    let manager = world.manager();

    // Enable the adapter for real.
    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let state_path = world.layout.state_dir.join("installed.toml");
    let state_bytes_before = std::fs::read(&state_path).expect("read state file");
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_some(),
        "pre-condition: receipt must exist after enable"
    );
    // The fake OpenClaw CLI wrote a registry marker for the plugin —
    // framework-side state we must prove dry-run does not touch.
    let registry_marker = world.openclaw_home.join("registry").join(COMPONENT);
    assert!(
        registry_marker.exists(),
        "pre-condition: openclaw registry marker must exist after enable"
    );

    // Dry-run disable.
    let outcome = manager
        .disable(COMPONENT, Some(FRAMEWORK), true)
        .expect("dry-run disable");
    assert!(outcome.dry_run, "outcome must be flagged dry-run");
    assert!(
        !outcome.claim_removed,
        "dry-run must not remove the receipt"
    );
    assert!(
        outcome.report.cleanup_complete,
        "dry-run plan reports as complete"
    );
    assert!(
        !outcome.report.messages.is_empty(),
        "dry-run must describe planned actions"
    );

    // State file must be byte-identical — no writes at all.
    let state_bytes_after = std::fs::read(&state_path).expect("read state file after dry-run");
    assert_eq!(
        state_bytes_before, state_bytes_after,
        "installed.toml must be byte-identical after dry-run disable"
    );
    // Double-check: receipt still present and status unchanged.
    let state_after = world.load_state();
    let claim_after = state_after
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("receipt must still exist after dry-run disable");
    assert_eq!(
        claim_after.status,
        anolisa_core::adapter::claim::ClaimStatus::Enabled,
        "receipt status must remain Enabled, not cleanup_failed"
    );
    // Framework state must be untouched: the plugin registry marker must
    // still exist (a real disable would have unregistered it).
    assert!(
        registry_marker.exists(),
        "openclaw registry marker must still exist after dry-run disable"
    );

    // Following real disable cleans up exactly once.
    let real = manager
        .disable(COMPONENT, Some(FRAMEWORK), false)
        .expect("real disable");
    assert!(!real.dry_run);
    assert!(real.claim_removed, "real disable must remove receipt");
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, FRAMEWORK)
            .is_none(),
        "receipt must be gone after real disable"
    );
}

/// Dry-run disable must report meaningful planned actions for a plugin
/// adapter (one with a `FrameworkPlugin` resource).
#[test]
fn dry_run_disable_reports_plugin_unregister() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");

    let outcome = manager
        .disable(COMPONENT, Some(FRAMEWORK), true)
        .expect("dry-run disable");
    assert!(outcome.dry_run);

    let has_unregister = outcome
        .report
        .messages
        .iter()
        .any(|m| m.contains("would unregister"));
    assert!(
        has_unregister,
        "dry-run must describe the plugin unregister: {:?}",
        outcome.report.messages
    );

    let has_receipt = outcome
        .report
        .messages
        .iter()
        .any(|m| m.contains("would remove adapter receipt"));
    assert!(
        has_receipt,
        "dry-run must note receipt removal: {:?}",
        outcome.report.messages
    );
}

/// Dry-run disable of a component with no receipt is a no-op, same as
/// a real disable, and the outcome carries the dry_run flag.
#[test]
fn dry_run_disable_no_receipt_is_noop() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    let manager = world.manager();

    let outcome = manager
        .disable(COMPONENT, Some(FRAMEWORK), true)
        .expect("dry-run disable no receipt");
    assert!(outcome.dry_run);
    assert!(!outcome.claim_removed);
    assert!(outcome.report.cleanup_complete);
    assert!(
        outcome
            .report
            .messages
            .iter()
            .any(|m| m.contains("no receipt")),
        "must report no receipt: {:?}",
        outcome.report.messages
    );
}

// ---------------------------------------------------------------------------
// Issue #1534: version gating, install policy, and runtime verification
// ---------------------------------------------------------------------------

/// 1. Host below the adapter minimum: no plugin install and no receipt.
#[test]
fn host_below_adapter_minimum_blocks_enable() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    write_openclaw_manifest(&world.layout, &plugin_adapter_block(Some(">=2026.5.0")));
    world.apply_env(&guard, None); // FAKE_OC_VERSION defaults to 2026.4.14
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("host below minimum must block enable");
    assert!(
        matches!(err, AdapterError::FrameworkVersionMismatch { .. }),
        "got {err:?}"
    );
    assert!(
        !world.registry_marker_exists(),
        "no plugin install before the version gate"
    );
    assert!(
        !world.has_claim(),
        "no receipt persisted on version mismatch"
    );
}

/// 2. Host version cannot be parsed: fail before any mutation.
#[test]
fn unparseable_host_version_blocks_enable() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    write_openclaw_manifest(&world.layout, &plugin_adapter_block(Some(">=2026.4.14")));
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_VERSION", "unreleased-nightly");
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("unparseable version must block enable");
    assert!(
        matches!(err, AdapterError::FrameworkCli { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// 3. Install help does not expose `--force`: fail before mutation and before
///    the receipt is persisted.
#[test]
fn missing_install_force_blocks_before_mutation() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_INSTALL_FORCE", "0");
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("missing --force must block enable");
    assert!(
        matches!(err, AdapterError::FrameworkCli { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(
        !world.has_claim(),
        "the force-capability gate runs before the receipt is persisted"
    );
}

/// 4. Unsafe flag supported but not authorized: the install argv omits it.
#[test]
fn unsafe_supported_without_authorization_omits_flag() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_INSTALL_UNSAFE", "1");
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let lines = argv_lines(&world.argv_log());
    let install = install_argv(&lines).expect("install argv recorded");
    assert!(
        install.contains("--force"),
        "install must pass --force: {install}"
    );
    assert!(
        !install.contains("--dangerously-force-unsafe-install"),
        "unsafe flag must be absent without authorization: {install}"
    );
}

/// 5. Unsafe flag supported and explicitly authorized: the single install
///    argv carries it exactly once.
#[test]
fn authorized_unsafe_supported_includes_flag_once() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_INSTALL_UNSAFE", "1");
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    manager
        .enable_with_options(
            COMPONENT,
            Some(FRAMEWORK),
            false,
            EnableOptions {
                allow_unsafe_plugin_install: true,
            },
        )
        .expect("authorized unsafe enable");
    let lines = argv_lines(&world.argv_log());
    let install = install_argv(&lines).expect("install argv recorded");
    assert_eq!(
        install
            .matches("--dangerously-force-unsafe-install")
            .count(),
        1,
        "unsafe flag must appear exactly once in the single install argv: {install}"
    );
    // No second install invocation.
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.starts_with("plugins install ") && !l.contains("--help"))
            .count(),
        1,
        "exactly one real install must run"
    );
}

/// 6. Unsafe authorized but the host does not expose the flag: fail before
///    mutation, no receipt.
#[test]
fn authorized_unsafe_unsupported_blocks() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None); // FAKE_OC_INSTALL_UNSAFE defaults to 0
    let manager = world.manager();

    let err = manager
        .enable_with_options(
            COMPONENT,
            Some(FRAMEWORK),
            false,
            EnableOptions {
                allow_unsafe_plugin_install: true,
            },
        )
        .expect_err("authorized-but-unsupported unsafe must block");
    assert!(
        matches!(err, AdapterError::FrameworkCli { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// An advertised unsafe option that is a deprecated no-op is not an effective
/// capability. Explicit authorization fails before mutation and points the
/// operator at OpenClaw's policy configuration instead.
#[test]
fn authorized_unsafe_deprecated_noop_blocks() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_INSTALL_UNSAFE", "1");
    guard.set("FAKE_OC_INSTALL_UNSAFE_NOOP", "1");
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    let err = manager
        .enable_with_options(
            COMPONENT,
            Some(FRAMEWORK),
            false,
            EnableOptions {
                allow_unsafe_plugin_install: true,
            },
        )
        .expect_err("a deprecated no-op cannot satisfy unsafe authorization");
    match err {
        AdapterError::FrameworkCli { reason, .. } => {
            assert!(reason.contains("deprecated no-op"), "{reason}");
            assert!(reason.contains("security.installPolicy"), "{reason}");
        }
        other => panic!("expected FrameworkCli, got {other:?}"),
    }
    assert!(
        install_argv(&argv_lines(&world.argv_log())).is_none(),
        "preflight must block before a real install"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// 7. Config entry whose version condition is unmet: not set, and left out
///    of the receipt.
#[test]
fn config_version_mismatch_skips_config_and_claim() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "plugins.entries.tokenless.hooks.allowConversationAccess"
value = true
framework_version = ">=2026.5.0"
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None); // host 2026.4.14 < 2026.5.0
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let lines = argv_lines(&world.argv_log());
    assert!(
        !lines.iter().any(|l| l.starts_with("config set")),
        "a config entry with an unmet version condition must not be applied: {lines:?}"
    );
    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("claim");
    assert!(
        !claim
            .resources
            .iter()
            .any(|r| matches!(r.kind, ClaimResourceKind::FrameworkConfig { .. })),
        "skipped config must not appear in the receipt"
    );
}

/// 8. Config entry whose version condition is met: set, and recorded in the
///    receipt.
#[test]
fn config_version_match_applies_and_records() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let key = "plugins.entries.tokenless.hooks.allowConversationAccess";
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "{key}"
value = true
framework_version = ">=2026.4.0"
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None); // host 2026.4.14 satisfies >=2026.4.0
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let lines = argv_lines(&world.argv_log());
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("config set") && l.contains(key)),
        "a config entry with a met version condition must be applied: {lines:?}"
    );
    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("claim");
    assert!(
        claim.resources.iter().any(|r| matches!(
            &r.kind,
            ClaimResourceKind::FrameworkConfig { key: k, .. } if k == key
        )),
        "applied config must be recorded in the receipt"
    );
}

/// A failed re-enable must not discard config facts from the last successful
/// enable because those keys remain present on the host.
#[test]
fn reenable_install_failure_preserves_applied_config_facts() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "preserved.key"
value = true
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("initial enable");
    world.apply_env(&guard, Some("install"));
    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("re-enable install must fail");

    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("cleanup receipt");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
    assert!(
        claim.resources.iter().any(|resource| matches!(
            &resource.kind,
            ClaimResourceKind::FrameworkConfig {
                key,
                state: ConfigApplyState::Applied,
                ..
            } if key == "preserved.key"
        )),
        "the successful enable's config fact must survive failed re-enable"
    );
    let DriverPayload::OpenClaw(payload) = &claim.driver_payload else {
        panic!("expected OpenClaw payload");
    };
    assert_eq!(payload.config_resources.len(), 1);
}

/// Successful re-enable reuses the matching applied fact without duplicating
/// either the resource or its payload reference.
#[test]
fn successful_reenable_keeps_config_receipt_idempotent() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "idempotent.key"
value = true
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("initial enable");
    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("re-enable");

    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("enabled receipt");
    let config_resources: Vec<_> = claim
        .resources
        .iter()
        .filter(|resource| matches!(resource.kind, ClaimResourceKind::FrameworkConfig { .. }))
        .collect();
    assert_eq!(config_resources.len(), 1);
    let DriverPayload::OpenClaw(payload) = &claim.driver_payload else {
        panic!("expected OpenClaw payload");
    };
    assert_eq!(payload.config_resources.len(), 1);
    assert_eq!(payload.config_resources[0], config_resources[0].id);
}

/// A command that mutates and then exits non-zero must leave a typed pending
/// fact rather than falsely claiming success or omitting uncertain host state.
#[test]
fn first_config_failure_after_mutation_records_pending_intent() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "first.key"
value = true

[[adapters.openclaw.config]]
key = "second.key"
value = true
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_CONFIG_FAIL_AFTER_KEY", "first.key");
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());

    world
        .manager()
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("the first config write must fail enable");

    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("cleanup receipt");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
    assert!(
        world.config_marker_exists("first.key"),
        "fake host must reproduce mutation before failure"
    );
    let config_resources: Vec<_> = claim
        .resources
        .iter()
        .filter(|resource| matches!(resource.kind, ClaimResourceKind::FrameworkConfig { .. }))
        .collect();
    assert_eq!(config_resources.len(), 1);
    assert!(matches!(
        &config_resources[0].kind,
        ClaimResourceKind::FrameworkConfig {
            key,
            state: ConfigApplyState::Pending,
            ..
        } if key == "first.key"
    ));
    let DriverPayload::OpenClaw(payload) = &claim.driver_payload else {
        panic!("expected OpenClaw payload");
    };
    assert!(payload.config_resources.is_empty());

    let config_sets: Vec<String> = argv_lines(&world.argv_log())
        .into_iter()
        .filter(|line| line.starts_with("config set "))
        .collect();
    assert_eq!(config_sets.len(), 1);
    assert!(config_sets[0].contains("first.key"));
    assert!(!config_sets[0].contains("second.key"));

    world.apply_env(&guard, None);
    guard.set("FAKE_OC_CONFIG_FAIL_AFTER_KEY", "");
    world
        .manager()
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("re-enable must replay and confirm the pending entry");
    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("recovered receipt");
    assert_eq!(claim.status, ClaimStatus::Enabled);
    let config_resources: Vec<_> = claim
        .resources
        .iter()
        .filter(|resource| matches!(resource.kind, ClaimResourceKind::FrameworkConfig { .. }))
        .collect();
    assert_eq!(config_resources.len(), 2);
    assert!(config_resources.iter().all(|resource| matches!(
        &resource.kind,
        ClaimResourceKind::FrameworkConfig {
            state: ConfigApplyState::Applied,
            ..
        }
    )));
    assert_eq!(
        config_resources
            .iter()
            .filter(|resource| matches!(
                &resource.kind,
                ClaimResourceKind::FrameworkConfig { key, .. } if key == "first.key"
            ))
            .count(),
        1,
        "the recovered pending key must not be duplicated"
    );
    let DriverPayload::OpenClaw(payload) = &claim.driver_payload else {
        panic!("expected OpenClaw payload");
    };
    assert_eq!(payload.config_resources.len(), 2);
}

/// A pending config that the replacement manifest no longer selects cannot be
/// reconciled, so re-enable must fail before another framework mutation.
#[test]
fn reenable_rejects_pending_config_removed_from_manifest() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "removed.key"
value = true
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_CONFIG_FAIL_AFTER_KEY", "removed.key");
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());

    world
        .manager()
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("the first config write must fail enable");
    write_openclaw_manifest(&world.layout, &plugin_adapter_block(None));
    guard.set("FAKE_OC_CONFIG_FAIL_AFTER_KEY", "");

    let err = world
        .manager()
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("an unselected pending config must block re-enable");
    let message = err.to_string();
    assert!(message.contains("removed.key"), "got {message}");
    assert!(message.contains("disable"), "got {message}");

    let lines = argv_lines(&world.argv_log());
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("plugins install ") && !line.contains("--help"))
            .count(),
        1,
        "the blocked re-enable must fail before another plugin install"
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("config set removed.key "))
            .count(),
        1,
        "the removed pending key cannot be replayed"
    );
    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("pending receipt must remain visible");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
    assert!(claim.resources.iter().any(|resource| matches!(
        &resource.kind,
        ClaimResourceKind::FrameworkConfig {
            key,
            state: ConfigApplyState::Pending,
            ..
        } if key == "removed.key"
    )));
}

/// Explicit disable reports uncertain config that may remain on the host
/// before removing the receipt.
#[test]
fn disable_reports_pending_config_left_in_place() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "uncertain.key"
value = true
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_CONFIG_FAIL_AFTER_KEY", "uncertain.key");

    world
        .manager()
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("the config write must leave pending state");
    guard.set("FAKE_OC_CONFIG_FAIL_AFTER_KEY", "");
    let outcome = world
        .manager()
        .disable(COMPONENT, Some(FRAMEWORK), false)
        .expect("explicit disable");

    assert!(outcome.claim_removed);
    assert!(outcome.report.cleanup_complete);
    assert!(
        outcome.report.messages.iter().any(|message| {
            message.contains("1 openclaw config entry")
                && message.contains("uncertain")
                && message.contains("left in place")
        }),
        "disable must disclose uncertain config before discarding the receipt: {:?}",
        outcome.report.messages
    );
    assert!(!world.has_claim());
}

/// A mid-sequence failure keeps the successful prefix confirmed, the failed
/// entry pending, and later unattempted entries absent.
#[test]
fn mid_sequence_config_failure_records_applied_prefix() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "first.key"
value = true

[[adapters.openclaw.config]]
key = "second.key"
value = true

[[adapters.openclaw.config]]
key = "third.key"
value = true
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_CONFIG_FAIL_KEY", "second.key");
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());

    world
        .manager()
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("the second config write must fail enable");

    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("cleanup receipt");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
    let config_resources: Vec<_> = claim
        .resources
        .iter()
        .filter(|resource| matches!(resource.kind, ClaimResourceKind::FrameworkConfig { .. }))
        .collect();
    assert_eq!(config_resources.len(), 2);
    assert_eq!(config_resources[0].id, "openclaw_config_0");
    assert!(matches!(
        &config_resources[0].kind,
        ClaimResourceKind::FrameworkConfig {
            key,
            state: ConfigApplyState::Applied,
            ..
        } if key == "first.key"
    ));
    assert_eq!(config_resources[1].id, "openclaw_config_1");
    assert!(matches!(
        &config_resources[1].kind,
        ClaimResourceKind::FrameworkConfig {
            key,
            state: ConfigApplyState::Pending,
            ..
        } if key == "second.key"
    ));
    let DriverPayload::OpenClaw(payload) = &claim.driver_payload else {
        panic!("expected OpenClaw payload");
    };
    assert_eq!(payload.config_resources, ["openclaw_config_0"]);

    let config_sets: Vec<String> = argv_lines(&world.argv_log())
        .into_iter()
        .filter(|line| line.starts_with("config set "))
        .collect();
    assert_eq!(config_sets.len(), 2);
    assert!(config_sets[0].contains("first.key"));
    assert!(config_sets[1].contains("second.key"));
    assert!(!config_sets.iter().any(|line| line.contains("third.key")));
}

/// 9. Inspect help exposes `--runtime`: runtime verification uses it.
#[test]
fn runtime_verification_uses_runtime_flag_when_supported() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_INSPECT_RUNTIME", "1");
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let lines = argv_lines(&world.argv_log());
    let inspect = inspect_argv(&lines).expect("inspect argv recorded");
    assert!(
        inspect.contains("--runtime") && inspect.contains("--json"),
        "runtime-capable host must inspect with --runtime --json: {inspect}"
    );
}

/// 10. Inspect help lacks `--runtime`: verification falls back to `--json`.
#[test]
fn runtime_verification_falls_back_to_json_only() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None); // FAKE_OC_INSPECT_RUNTIME defaults to 0
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let lines = argv_lines(&world.argv_log());
    let inspect = inspect_argv(&lines).expect("inspect argv recorded");
    assert!(
        inspect.contains("--json"),
        "must inspect with --json: {inspect}"
    );
    assert!(
        !inspect.contains("--runtime"),
        "must not pass --runtime when unsupported: {inspect}"
    );
}

/// 11. Legacy diagnostics before the JSON must still parse.
#[test]
fn runtime_verification_tolerates_leading_diagnostics() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_INSPECT_DIAG", "1");
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable succeeds despite legacy diagnostics before the JSON");
    assert!(world.has_claim());
}

/// 12. Runtime status is not `loaded`: enable fails with diagnostics and the
///     receipt is kept for cleanup retry.
#[test]
fn runtime_status_error_fails_and_keeps_cleanup_receipt() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_RUNTIME_STATUS", "error");
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("non-loaded runtime status must fail enable");
    match err {
        AdapterError::FrameworkCli { reason, .. } => {
            assert!(
                reason.contains("error") && reason.contains("loaded"),
                "diagnostics must surface the observed and expected status: {reason}"
            );
        }
        other => panic!("expected FrameworkCli, got {other:?}"),
    }
    assert!(
        world.registry_marker_exists(),
        "install ran before the failed runtime verification"
    );
    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("receipt kept for cleanup retry");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

/// 13. Dry-run: probes are allowed but nothing is mutated, and the plan shows
///     the single install command.
#[test]
fn dry_run_probes_but_does_not_mutate() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    let outcome = manager
        .enable(COMPONENT, Some(FRAMEWORK), true)
        .expect("dry-run enable");
    match outcome {
        EnableOutcome::Planned(plan) => {
            let cmd = plan
                .register_command
                .expect("plan shows the install command");
            assert!(cmd.contains("--force"), "plan must show --force: {cmd}");
            assert!(
                !cmd.contains("--dangerously-force-unsafe-install"),
                "unauthorized dry-run plan must not show the unsafe flag: {cmd}"
            );
        }
        EnableOutcome::Enabled(_) => panic!("dry-run must not enable"),
    }
    // Read-only probes may have run, but nothing was installed or persisted.
    let lines = argv_lines(&world.argv_log());
    assert!(
        install_argv(&lines).is_none(),
        "dry-run must not run a real install: {lines:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// 15. An unsafe authorization for a skill-only adapter is rejected before
///     any work.
#[test]
fn unsafe_authorization_rejected_for_skill_bundle() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
adapter_type = "skill_bundle"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    let manager = world.manager();

    let err = manager
        .enable_with_options(
            COMPONENT,
            Some(FRAMEWORK),
            false,
            EnableOptions {
                allow_unsafe_plugin_install: true,
            },
        )
        .expect_err("unsafe authorization must be rejected for skill_bundle");
    assert!(
        matches!(err, AdapterError::UnsafeInstallNotApplicable { .. }),
        "got {err:?}"
    );
    assert!(!world.has_claim());
}

/// 1 (extended). A skill_bundle also honors the adapter-level version gate:
/// an incompatible host blocks enable with no receipt.
#[test]
fn skill_bundle_honors_adapter_version_gate() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
adapter_type = "skill_bundle"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[adapters.compat]
framework_version = ">=2026.5.0"

[adapters.openclaw]
skills = ["sec-audit"]
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None); // host 2026.4.14 < 2026.5.0
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("skill_bundle below the adapter minimum must be blocked");
    assert!(
        matches!(err, AdapterError::FrameworkVersionMismatch { .. }),
        "got {err:?}"
    );
    assert!(!world.has_claim());
}

/// P1 fail-closed: an unparseable `--version` blocks a plugin enable even
/// when the manifest declares no version condition.
#[test]
fn unparseable_version_blocks_even_without_condition() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage(); // default manifest declares no compat requirement
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_VERSION", "nightly-build");
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("an unreadable version must fail closed before install");
    assert!(
        matches!(err, AdapterError::FrameworkCli { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// P1 fail-closed: a host whose inspect help exposes no `--json` is rejected
/// before the first mutation (the full profile, including inspect help, is
/// probed during prepare), so no plugin is installed and no receipt is left.
#[test]
fn missing_inspect_json_blocks_before_mutation() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_INSPECT_JSON", "0");
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("no --json inspect support must fail closed before install");
    match err {
        AdapterError::FrameworkCli { reason, .. } => {
            assert!(
                reason.contains("--json"),
                "must explain the missing --json capability: {reason}"
            );
        }
        other => panic!("expected FrameworkCli, got {other:?}"),
    }
    assert!(
        !world.registry_marker_exists(),
        "install must not run when runtime verification cannot be performed"
    );
    assert!(!world.has_claim());
}

/// P1 fail-closed: every read-only probe — `--version`, install `--help`, and
/// inspect `--help` — is performed before the first mutation, so a non-zero
/// exit from any of them blocks enable with no install and no receipt, even
/// when the output would otherwise look like a capability answer.
#[test]
fn nonzero_probe_exit_blocks_enable_before_mutation() {
    for (stage_label, note) in [
        ("version", "a non-zero `--version` with parseable output"),
        (
            "install_help",
            "a non-zero install --help still mentioning --force",
        ),
        (
            "inspect_help",
            "a non-zero inspect --help still mentioning --json",
        ),
    ] {
        let guard = OpenClawEnvGuard::acquire();
        let world = stage();
        world.apply_env(&guard, None);
        guard.set("FAKE_OC_PROBE_FAIL", stage_label);
        let manager = world.manager();

        let err = manager
            .enable(COMPONENT, Some(FRAMEWORK), false)
            .expect_err(note);
        assert!(
            matches!(err, AdapterError::FrameworkCli { .. }),
            "{note}: got {err:?}"
        );
        assert!(!world.registry_marker_exists(), "{note}: no install");
        assert!(!world.has_claim(), "{note}: no receipt");
    }
}

/// Each probe runs exactly once in a real enable: all three (`--version`,
/// install `--help`, inspect `--help`) happen in prepare, and apply re-probes
/// nothing (it reuses the prepared capabilities).
#[test]
fn each_probe_runs_exactly_once_per_enable() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let lines = argv_lines(&world.argv_log());
    let count = |pred: &dyn Fn(&&String) -> bool| lines.iter().filter(|l| pred(l)).count();
    assert_eq!(
        count(&|l| l.as_str() == "--version"),
        1,
        "one --version probe: {lines:?}"
    );
    assert_eq!(
        count(&|l| l.as_str() == "plugins install --help"),
        1,
        "one install --help probe: {lines:?}"
    );
    assert_eq!(
        count(&|l| l.as_str() == "plugins inspect --help"),
        1,
        "one inspect --help probe: {lines:?}"
    );
}

/// P2 (negative): when the host does NOT expose the unsafe flag, a plain
/// safety-rejected install must not dangle the `--allow-unsafe-plugin-install`
/// hint (retrying would just fail in prepare), and there is no auto-retry.
#[test]
fn safety_rejection_without_unsafe_support_omits_hint() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, Some("install_unsafe_policy"));
    // FAKE_OC_INSTALL_UNSAFE defaults to 0 → host does not expose the flag.
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("safety-rejected install must fail");
    match err {
        AdapterError::FrameworkCli { reason, .. } => {
            assert!(
                !reason.contains("--allow-unsafe-plugin-install"),
                "must not suggest an unsafe retry the host cannot honor: {reason}"
            );
        }
        other => panic!("expected FrameworkCli, got {other:?}"),
    }
    let lines = argv_lines(&world.argv_log());
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.starts_with("plugins install ") && !l.contains("--help"))
            .count(),
        1,
        "must not auto-retry the install: {lines:?}"
    );
}

/// An advertised deprecated no-op is equivalent to no effective unsafe
/// capability for retry guidance.
#[test]
fn safety_rejection_with_deprecated_noop_omits_hint() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, Some("install_unsafe_policy"));
    guard.set("FAKE_OC_INSTALL_UNSAFE", "1");
    guard.set("FAKE_OC_INSTALL_UNSAFE_NOOP", "1");
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("safety-rejected install must fail");
    match err {
        AdapterError::FrameworkCli { reason, .. } => {
            assert!(
                !reason.contains("--allow-unsafe-plugin-install"),
                "must not suggest retrying with a no-op option: {reason}"
            );
        }
        other => panic!("expected FrameworkCli, got {other:?}"),
    }
}

/// P2: plugin-safety findings printed to stdout are surfaced in the failure,
/// alongside the explicit-retry hint.
#[test]
fn safety_rejection_on_stdout_is_surfaced() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, Some("install_unsafe_policy_stdout"));
    guard.set("FAKE_OC_INSTALL_UNSAFE", "1"); // host supports the unsafe flag
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("stdout safety rejection must fail");
    match err {
        AdapterError::FrameworkCli { reason, .. } => {
            assert!(
                reason.contains("SECURITY FINDING"),
                "stdout findings must be surfaced to the operator: {reason}"
            );
            assert!(
                reason.contains("--allow-unsafe-plugin-install"),
                "must hint the explicit retry: {reason}"
            );
        }
        other => panic!("expected FrameworkCli, got {other:?}"),
    }
}

/// P1: two config entries sharing a key but gated on different versions —
/// only the entry whose condition the host satisfies is applied and recorded.
#[test]
fn same_key_config_applies_only_selected_version() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "shared.key"
value = "future"
framework_version = ">=2026.5.0"

[[adapters.openclaw.config]]
key = "shared.key"
value = "current"
framework_version = ">=2026.4.0"
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None); // host 2026.4.14: only the ">=2026.4.0" entry matches
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("enable");
    let lines = argv_lines(&world.argv_log());
    let config_sets: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("config set "))
        .collect();
    assert_eq!(
        config_sets.len(),
        1,
        "only the version-selected same-key entry must be applied: {config_sets:?}"
    );
    assert!(
        config_sets[0].contains("current") && !config_sets[0].contains("future"),
        "the applied value must be the selected version's, not the skipped one: {config_sets:?}"
    );
    // The receipt records exactly one config resource.
    let state = world.load_state();
    let claim = state
        .find_adapter_claim(COMPONENT, FRAMEWORK)
        .expect("claim");
    assert_eq!(
        claim
            .resources
            .iter()
            .filter(|r| matches!(r.kind, ClaimResourceKind::FrameworkConfig { .. }))
            .count(),
        1,
        "only the selected config entry must appear in the receipt"
    );
}

/// P2: when a normal install is rejected by OpenClaw's plugin-safety policy
/// and the host exposes the unsafe flag, the error points the operator at the
/// explicit `--allow-unsafe-plugin-install` retry — without auto-retrying.
#[test]
fn safety_rejection_surfaces_explicit_retry_hint() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, Some("install_unsafe_policy"));
    guard.set("FAKE_OC_INSTALL_UNSAFE", "1"); // host supports the unsafe flag
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("safety-rejected install must fail");
    match err {
        AdapterError::FrameworkCli { reason, .. } => {
            assert!(
                reason.contains("--allow-unsafe-plugin-install"),
                "must hint the explicit retry authorization: {reason}"
            );
        }
        other => panic!("expected FrameworkCli, got {other:?}"),
    }
    // No automatic unsafe retry: only the one failed (safe) install ran.
    let lines = argv_lines(&world.argv_log());
    let installs: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("plugins install ") && !l.contains("--help"))
        .collect();
    assert_eq!(
        installs.len(),
        1,
        "must not auto-retry the install: {installs:?}"
    );
    assert!(
        !installs[0].contains("--dangerously-force-unsafe-install"),
        "the failed attempt must have been the safe one: {installs:?}"
    );
}

/// P1: an explicitly empty adapter-level version requirement is a manifest
/// error, not a silent "no requirement" — it must not fall through to enable.
#[test]
fn empty_compat_framework_version_is_invalid() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[adapters.compat]
framework_version = ""
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("an empty compat.framework_version must be rejected");
    assert!(
        matches!(err, AdapterError::InvalidAdapterInput { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// P1: an explicitly empty per-config version condition is a manifest error;
/// no config is applied and no receipt is written.
#[test]
fn empty_config_framework_version_is_invalid() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "some.key"
value = true
framework_version = ""
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("an empty config framework_version must be rejected");
    assert!(
        matches!(err, AdapterError::InvalidAdapterInput { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// P1: a malformed version constraint is a typed input error (not a generic
/// framework-CLI error), and enable stops before any mutation.
#[test]
fn malformed_config_constraint_is_invalid_input() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "some.key"
value = true
framework_version = ">=not.a.version"
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("a malformed constraint must be rejected");
    assert!(
        matches!(err, AdapterError::InvalidAdapterInput { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// Every config condition clause is validated before the condition is
/// evaluated. A malformed later clause cannot hide behind an earlier
/// non-match and silently skip the config.
#[test]
fn malformed_later_config_clause_is_invalid_input() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "openclaw"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"

[[adapters.openclaw.config]]
key = "some.key"
value = true
framework_version = ">=2027.0.0, >=not.a.version"
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None); // host fails the first clause
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("a malformed later clause must still be rejected");
    assert!(
        matches!(err, AdapterError::InvalidAdapterInput { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// P1: a malformed adapter-level constraint (an empty `-` suffix here) is a
/// typed input error and stops enable before any mutation — it must not be
/// silently treated as the well-formed `>=2026.4.14`.
#[test]
fn malformed_compat_constraint_is_invalid_input() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    write_openclaw_manifest(&world.layout, &plugin_adapter_block(Some(">=2026.4.14-")));
    world.apply_env(&guard, None); // host 2026.4.14 would satisfy >=2026.4.14
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("a malformed compat constraint must be rejected");
    assert!(
        matches!(err, AdapterError::InvalidAdapterInput { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// P1: a `--version` output carrying only an unrelated (non-calendar) number
/// is treated as unknown; with a declared requirement, enable fails closed.
#[test]
fn unrelated_numeric_version_is_not_accepted() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    write_openclaw_manifest(&world.layout, &plugin_adapter_block(Some(">=2026.4.0")));
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_VERSION", "22.14.0"); // not calendar-shaped
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("a non-calendar version must not satisfy the gate");
    assert!(
        matches!(err, AdapterError::FrameworkCli { .. }),
        "got {err:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// A calendar-shaped token in a warning must not be selected ahead of the
/// explicit OpenClaw version line.
#[test]
fn version_warning_date_does_not_override_openclaw_version() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    write_openclaw_manifest(&world.layout, &plugin_adapter_block(Some("<2027.0.0")));
    world.apply_env(&guard, None);
    guard.set(
        "FAKE_OC_VERSION_PREAMBLE",
        "warning: certificate expires on 2099.1.1",
    );
    let manager = world.manager();

    manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect("the explicit 2026.4.14 version satisfies the gate");
    assert!(world.registry_marker_exists());
    assert!(world.has_claim());
}

/// P2: when `--version` output is multi-line and unparseable, the failure
/// preserves the full trimmed output (not just the first line), so the real
/// version text is actionable even behind a leading warning line.
#[test]
fn unparseable_version_error_keeps_full_output() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    write_openclaw_manifest(&world.layout, &plugin_adapter_block(Some(">=2026.4.0")));
    world.apply_env(&guard, None);
    guard.set(
        "FAKE_OC_VERSION_PREAMBLE",
        "warning: config migration pending",
    );
    guard.set("FAKE_OC_VERSION", "nightly-build"); // unparseable, on the 2nd line
    let manager = world.manager();

    let err = manager
        .enable(COMPONENT, Some(FRAMEWORK), false)
        .expect_err("unparseable version must block");
    match err {
        AdapterError::FrameworkCli { reason, .. } => {
            assert!(
                reason.contains("nightly-build"),
                "the real (unparseable) version line must survive, not just the warning: {reason}"
            );
        }
        other => panic!("expected FrameworkCli, got {other:?}"),
    }
    assert!(!world.has_claim());
}

/// P2 acceptance: an authorized-unsafe dry-run shows the unsafe flag in the
/// planned install command and mutates nothing.
#[test]
fn authorized_unsafe_dry_run_shows_flag_without_mutation() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_INSTALL_UNSAFE", "1");
    guard.set("FAKE_OC_ARGV_LOG", world.argv_log());
    let manager = world.manager();

    let outcome = manager
        .enable_with_options(
            COMPONENT,
            Some(FRAMEWORK),
            true,
            EnableOptions {
                allow_unsafe_plugin_install: true,
            },
        )
        .expect("dry-run enable");
    match outcome {
        EnableOutcome::Planned(plan) => {
            let cmd = plan
                .register_command
                .expect("plan shows the install command");
            assert!(
                cmd.contains("--dangerously-force-unsafe-install"),
                "authorized dry-run plan must show the unsafe flag: {cmd}"
            );
        }
        EnableOutcome::Enabled(_) => panic!("dry-run must not enable"),
    }
    let lines = argv_lines(&world.argv_log());
    assert!(
        install_argv(&lines).is_none(),
        "dry-run must not run a real install: {lines:?}"
    );
    assert!(!world.registry_marker_exists());
    assert!(!world.has_claim());
}

/// P2 acceptance: an authorized real enable records the exact install command,
/// including the unsafe flag, in the live central operation log.
#[test]
fn central_log_records_authorized_unsafe_install_argv() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    world.apply_env(&guard, None);
    guard.set("FAKE_OC_INSTALL_UNSAFE", "1");
    let manager = world.manager();

    manager
        .enable_with_options(
            COMPONENT,
            Some(FRAMEWORK),
            false,
            EnableOptions {
                allow_unsafe_plugin_install: true,
            },
        )
        .expect("authorized unsafe enable");
    let log = std::fs::read_to_string(&world.layout.central_log).expect("central log");
    assert!(
        log.contains("plugins install")
            && log.contains("--force")
            && log.contains("--dangerously-force-unsafe-install"),
        "central log must record the exact install argv incl. the unsafe flag: {log}"
    );
}

/// 15. An unsafe authorization for a non-OpenClaw framework is rejected.
#[test]
fn unsafe_authorization_rejected_for_non_openclaw_framework() {
    let guard = OpenClawEnvGuard::acquire();
    let world = stage();
    let block = format!(
        r#"[[adapters]]
framework = "hermes"
source = "adapters/{COMPONENT}/hermes"
dest = "{{datadir}}/adapters/{{component}}/hermes/"
"#
    );
    write_openclaw_manifest(&world.layout, &block);
    world.apply_env(&guard, None);
    let manager = world.manager();

    let err = manager
        .enable_with_options(
            COMPONENT,
            Some("hermes"),
            false,
            EnableOptions {
                allow_unsafe_plugin_install: true,
            },
        )
        .expect_err("unsafe authorization must be rejected for a non-OpenClaw framework");
    assert!(
        matches!(err, AdapterError::UnsafeInstallNotApplicable { .. }),
        "got {err:?}"
    );
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, "hermes")
            .is_none(),
        "no receipt for a rejected unsafe authorization"
    );
}
