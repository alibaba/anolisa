//! Unit tests for `anolisa upgrade`. The apply path is driven entirely through
//! an injected fake that implements both [`PackageQuery`] and
//! [`PackageTransaction`], so no live rpmdb/dnf is required. The fake records
//! transaction call order and refuses to be called on the dry-run path.

use super::*;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anolisa_platform::pkg_query::{PackageInfo, PackageQueryError, PackageVersion};

use anolisa_core::state::{InstalledObject, InstalledState, RpmMetadata, SubscriptionScope};

// ── fake host ────────────────────────────────────────────────────────────────

/// In-memory host: `query_installed` returns the configured post-transaction
/// version, `update`/`install` record their call order and may be made to fail
/// for a named package.
#[derive(Default)]
struct FakeHost {
    /// package -> version returned by `query_installed` (post-transaction).
    installed: HashMap<String, PackageInfo>,
    /// packages whose `update` transaction fails.
    fail_update: HashSet<String>,
    /// packages whose `install` transaction fails.
    fail_install: HashSet<String>,
    /// packages whose `query_installed` returns `Ok(None)` (rpmdb miss).
    missing_after: HashSet<String>,
    /// package -> source repo returned by `installed_origin`.
    origins: HashMap<String, String>,
    /// packages whose `installed_origin` returns an error.
    fail_origin: HashSet<String>,
    /// ordered transaction log (`"update:<pkg>"` / `"install:<pkg>"`).
    calls: RefCell<Vec<String>>,
}

impl FakeHost {
    fn with_installed(mut self, package: &str, info: PackageInfo) -> Self {
        self.installed.insert(package.to_string(), info);
        self
    }

    fn with_origin(mut self, package: &str, origin: &str) -> Self {
        self.origins.insert(package.to_string(), origin.to_string());
        self
    }

    fn txn_calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl PackageQuery for FakeHost {
    fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
        if self.missing_after.contains(package) {
            return Ok(None);
        }
        Ok(self.installed.get(package).cloned())
    }

    fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
        Ok(Vec::new())
    }

    fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
        if self.fail_origin.contains(package) {
            return Err(PackageQueryError::QueryFailed {
                command: "dnf".to_string(),
                code: Some(1),
                stderr: "repo lookup failed".to_string(),
            });
        }
        Ok(self.origins.get(package).cloned())
    }
}

impl PackageTransaction for FakeHost {
    fn install(&self, package: &str) -> Result<(), PackageTransactionError> {
        self.calls.borrow_mut().push(format!("install:{package}"));
        if self.fail_install.contains(package) {
            return Err(PackageTransactionError::TransactionFailed {
                command: "dnf".to_string(),
                operation: "install".to_string(),
                code: Some(1),
                stderr: "boom".to_string(),
            });
        }
        Ok(())
    }

    fn update(&self, package: &str) -> Result<(), PackageTransactionError> {
        self.calls.borrow_mut().push(format!("update:{package}"));
        if self.fail_update.contains(package) {
            return Err(PackageTransactionError::TransactionFailed {
                command: "dnf".to_string(),
                operation: "update".to_string(),
                code: Some(1),
                stderr: "boom".to_string(),
            });
        }
        Ok(())
    }

    fn remove(&self, package: &str) -> Result<(), PackageTransactionError> {
        // `upgrade` never removes packages; a call is a routing bug.
        self.calls.borrow_mut().push(format!("remove:{package}"));
        Ok(())
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn info(name: &str, version: &str, release: Option<&str>) -> PackageInfo {
    PackageInfo {
        name: name.to_string(),
        version: PackageVersion {
            epoch: None,
            version: version.to_string(),
            release: release.map(str::to_string),
        },
        arch: "x86_64".to_string(),
        origin: None,
    }
}

fn cli_check(
    action: &str,
    package: Option<&str>,
    installed: Option<&str>,
    available: Option<&str>,
    error: Option<&str>,
) -> CliCheck {
    CliCheck {
        package: package.map(str::to_string),
        installed: installed.map(str::to_string),
        available: available.map(str::to_string),
        action: action.to_string(),
        error: error.map(str::to_string),
    }
}

fn cli_noop() -> CliCheck {
    cli_check(
        ACTION_NOOP,
        Some("anolisa"),
        Some("1.0.0-1.al4"),
        None,
        None,
    )
}

fn component_check(
    component: &str,
    package: Option<&str>,
    ownership: Option<&str>,
    installed: Option<&str>,
    available: Option<&str>,
    action: &str,
    error: Option<&str>,
) -> ComponentCheck {
    ComponentCheck {
        component: component.to_string(),
        package: package.map(str::to_string),
        ownership: ownership.map(str::to_string),
        installed: installed.map(str::to_string),
        available: available.map(str::to_string),
        action: action.to_string(),
        error: error.map(str::to_string),
        absent_from_state: false,
    }
}

/// Build an installed component object as it would exist in state before an
/// upgrade, so a planned update has a row to refresh.
fn rpm_component(name: &str, package: &str, evr: &str, ownership: Ownership) -> InstalledObject {
    InstalledObject {
        kind: ObjectKind::Component,
        name: name.to_string(),
        version: evr.to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some(
            if matches!(ownership, Ownership::RawManaged) {
                "raw"
            } else {
                "rpm"
            }
            .to_string(),
        ),
        ownership: Some(ownership),
        rpm_metadata: if matches!(ownership, Ownership::RawManaged) {
            None
        } else {
            Some(RpmMetadata {
                package_name: package.to_string(),
                evr: Some(evr.to_string()),
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            })
        },
        installed_at: "2026-06-01T10:00:00Z".to_string(),
        last_operation_id: None,
        managed: !matches!(ownership, Ownership::RpmObserved),
        adopted: matches!(ownership, Ownership::RpmObserved),
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

/// Write a state file under `layout` seeded with `objects` so an apply run can
/// find (and refresh) already-installed components.
fn seed_state(layout: &FsLayout, objects: Vec<InstalledObject>) {
    let mut state = InstalledState::default();
    for obj in objects {
        state.upsert_object(obj);
    }
    state
        .save(&layout.state_dir.join("installed.toml"))
        .expect("seed state");
}

fn system_ctx(prefix: PathBuf) -> CliContext {
    CliContext {
        install_mode: InstallMode::System,
        prefix: Some(prefix),
        json: false,
        dry_run: false,
        verbose: false,
        quiet: true,
        no_color: true,
    }
}

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

// ── argument parsing ─────────────────────────────────────────────────────────

#[test]
fn upgrade_parses_bare() {
    let args = UpgradeArgs::try_parse_from(["upgrade"]).expect("parse");
    assert!(args.target.is_none());
    assert!(!args.assume_yes);
}

#[test]
fn upgrade_parses_target() {
    let args =
        UpgradeArgs::try_parse_from(["upgrade", "--target", "agentic_os-latest"]).expect("parse");
    assert_eq!(args.target.as_deref(), Some("agentic_os-latest"));
}

#[test]
fn upgrade_parses_assume_yes_long_and_short() {
    assert!(
        UpgradeArgs::try_parse_from(["upgrade", "--assume-yes"])
            .expect("parse")
            .assume_yes
    );
    assert!(
        UpgradeArgs::try_parse_from(["upgrade", "-y"])
            .expect("parse")
            .assume_yes
    );
}

// ── mode / privilege gating ──────────────────────────────────────────────────

#[test]
fn upgrade_rejects_user_mode() {
    let args = UpgradeArgs {
        target: None,
        assume_yes: false,
    };
    let err = super::handle(args, &user_ctx()).expect_err("user mode must be rejected");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("system"));
}

#[test]
fn upgrade_real_execution_without_root_is_rejected_with_sudo_hint() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default();
    let plan = build_plan(None, &cli_noop(), &[]);

    let err = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, false, false, COMMAND)
        .expect_err("non-root real execution must be rejected");
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(err.reason().contains("sudo anolisa upgrade"));
    assert!(host.txn_calls().is_empty(), "no dnf on a rejected run");
}

/// The apply layer allows a dry-run without root (no dnf, no state write). Note
/// this is the *system-mode* apply path: at the dispatcher layer a non-root
/// `anolisa upgrade --dry-run` still resolves to user mode unless `--install-mode
/// system` is given (or run under sudo), and is rejected there — the
/// `system_only("upgrade", true)` policy only waives the root requirement for a
/// dry-run, not the system-mode requirement (same as `repair`).
#[test]
fn upgrade_dry_run_apply_runs_without_root_and_touches_nothing() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default();
    let plan = build_plan(
        Some("agentic_os-latest".to_string()),
        &cli_noop(),
        &[component_check(
            "cosh",
            Some("copilot-shell"),
            Some("rpm-managed"),
            Some("1.0.0-1.al4"),
            Some("1.1.0-1.al4"),
            ACTION_UPDATE,
            None,
        )],
    );

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, false, true, COMMAND)
        .expect("dry-run is allowed without root");
    assert!(result.dry_run);
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.updated.len(), 1);
    assert!(
        host.txn_calls().is_empty(),
        "dry-run must not run a transaction"
    );
    assert!(
        !layout.state_dir.join("installed.toml").exists(),
        "dry-run must not write state"
    );
}

// ── plan conversion ──────────────────────────────────────────────────────────

#[test]
fn plan_classifies_update_install_skip_and_error() {
    let cli = cli_check(
        ACTION_UPDATE,
        Some("anolisa"),
        Some("0.5.0-1.al4"),
        Some("1.0.0-1.al4"),
        None,
    );
    let components = vec![
        // rpm update
        component_check(
            "cosh",
            Some("copilot-shell"),
            Some("rpm-observed"),
            Some("0.5.0-1.al4"),
            Some("1.0.0-1.al4"),
            ACTION_UPDATE,
            None,
        ),
        // installable default (resolved package)
        component_check(
            "agent-memory",
            Some("agent-memory"),
            None,
            None,
            None,
            ACTION_INSTALL,
            None,
        ),
        // raw-managed → skipped
        component_check(
            "local-tool",
            None,
            Some("raw-managed"),
            Some("0.5.0"),
            None,
            ACTION_UNSUPPORTED_RPM,
            None,
        ),
        // installable default without a resolved package → error
        component_check("mystery", None, None, None, None, ACTION_INSTALL, None),
        // item error surfaced by the check
        component_check(
            "broken",
            Some("broken-pkg"),
            Some("rpm-managed"),
            Some("1.0.0-1"),
            None,
            ACTION_ERROR,
            Some("rpmdb miss"),
        ),
        // noop → nothing
        component_check(
            "sec-core",
            Some("agent-sec-core"),
            Some("rpm-managed"),
            Some("1.0.0-1"),
            None,
            ACTION_NOOP,
            None,
        ),
    ];

    let plan = build_plan(Some("agentic_os-latest".to_string()), &cli, &components);

    assert_eq!(
        plan.cli.as_ref().map(|c| c.package.as_str()),
        Some("anolisa")
    );
    assert_eq!(plan.updates.len(), 1);
    assert_eq!(plan.updates[0].package, "copilot-shell");
    assert_eq!(plan.installs.len(), 1);
    assert_eq!(plan.installs[0].package, "agent-memory");
    assert_eq!(plan.skipped.len(), 1);
    assert_eq!(plan.skipped[0].name, "local-tool");
    assert_eq!(plan.skipped[0].reason, RAW_SKIP_REASON);
    // "mystery" (install without package) + "broken" (check error) → 2 errors.
    assert_eq!(plan.errors.len(), 2);
    assert!(plan.has_errors());
}

#[test]
fn plan_treats_non_rpm_cli_as_skipped_not_error() {
    let cli = cli_check(
        ACTION_UNSUPPORTED,
        None,
        None,
        None,
        Some("anolisa CLI is not RPM-owned"),
    );
    let plan = build_plan(None, &cli, &[]);
    assert!(plan.cli.is_none());
    assert_eq!(plan.skipped.len(), 1);
    assert!(!plan.has_errors());
}

// ── apply: error blocking ────────────────────────────────────────────────────

#[test]
fn plan_errors_block_real_execution_before_dnf() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default();
    // An install with no resolvable package is a plan error.
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "mystery",
            None,
            None,
            None,
            None,
            ACTION_INSTALL,
            None,
        )],
    );
    assert!(plan.has_errors());

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("blocked plan renders rather than erroring");
    assert_eq!(result.status, STATUS_BLOCKED);
    assert!(!result.dry_run);
    assert!(
        host.txn_calls().is_empty(),
        "a blocked plan must not run any dnf transaction"
    );
    assert!(
        !layout.state_dir.join("installed.toml").exists(),
        "a blocked plan must not write state"
    );
}

// ── apply: raw boundary ──────────────────────────────────────────────────────

#[test]
fn raw_managed_component_is_skipped_and_never_transacted() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default();
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "local-tool",
            None,
            Some("raw-managed"),
            Some("0.5.0"),
            None,
            ACTION_UNSUPPORTED_RPM,
            None,
        )],
    );

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].reason, RAW_SKIP_REASON);
    assert!(
        host.txn_calls().is_empty(),
        "a raw-managed component must never be transacted"
    );
}

// ── apply: transaction ordering ──────────────────────────────────────────────

#[test]
fn transaction_order_is_cli_then_updates_then_installs() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default()
        .with_installed("anolisa", info("anolisa", "1.0.0", Some("1.al4")))
        .with_installed(
            "copilot-shell",
            info("copilot-shell", "1.1.0", Some("1.al4")),
        )
        .with_installed("agent-memory", info("agent-memory", "1.0.0", Some("1.al4")));
    // Seed the component being updated so its state row is refreshed cleanly.
    seed_state(
        &layout,
        vec![rpm_component(
            "cosh",
            "copilot-shell",
            "1.0.0-1.al4",
            Ownership::RpmManaged,
        )],
    );

    let cli = cli_check(
        ACTION_UPDATE,
        Some("anolisa"),
        Some("0.5.0-1.al4"),
        Some("1.0.0-1.al4"),
        None,
    );
    let components = vec![
        component_check(
            "cosh",
            Some("copilot-shell"),
            Some("rpm-managed"),
            Some("1.0.0-1.al4"),
            Some("1.1.0-1.al4"),
            ACTION_UPDATE,
            None,
        ),
        component_check(
            "agent-memory",
            Some("agent-memory"),
            None,
            None,
            None,
            ACTION_INSTALL,
            None,
        ),
    ];
    let plan = build_plan(None, &cli, &components);

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(
        host.txn_calls(),
        vec![
            "update:anolisa".to_string(),
            "update:copilot-shell".to_string(),
            "install:agent-memory".to_string(),
        ],
        "must apply CLI update, then component updates, then installs"
    );
}

// ── apply: state refresh for a new default ───────────────────────────────────

#[test]
fn installed_default_is_recorded_as_rpm_managed_after_refresh() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default()
        .with_installed("agent-memory", info("agent-memory", "1.0.0", Some("1.al4")))
        .with_origin("agent-memory", "alinux4-agentic-os");
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "agent-memory",
            Some("agent-memory"),
            None,
            None,
            None,
            ACTION_INSTALL,
            None,
        )],
    );

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.installed.len(), 1);
    assert_eq!(result.installed[0].version.as_deref(), Some("1.0.0-1.al4"));

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let obj = state
        .find_object(ObjectKind::Component, "agent-memory")
        .expect("recorded component");
    assert_eq!(obj.effective_ownership(), Ownership::RpmManaged);
    assert_eq!(obj.install_backend.as_deref(), Some("rpm"));
    assert!(obj.managed);
    assert!(!obj.adopted);
    assert_eq!(obj.version, "1.0.0-1.al4");
    assert_eq!(
        obj.rpm_metadata.as_ref().map(|m| m.package_name.as_str()),
        Some("agent-memory")
    );
    assert_eq!(
        obj.rpm_metadata
            .as_ref()
            .and_then(|m| m.source_repo.as_deref()),
        Some("alinux4-agentic-os")
    );
}

#[test]
fn origin_lookup_failure_is_warning_not_apply_failure() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let mut host = FakeHost::default()
        .with_installed("agent-memory", info("agent-memory", "1.0.0", Some("1.al4")));
    host.fail_origin.insert("agent-memory".to_string());
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "agent-memory",
            Some("agent-memory"),
            None,
            None,
            None,
            ACTION_INSTALL,
            None,
        )],
    );

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("could not determine source repo"));

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let obj = state
        .find_object(ObjectKind::Component, "agent-memory")
        .expect("recorded component");
    assert_eq!(
        obj.rpm_metadata
            .as_ref()
            .and_then(|m| m.source_repo.as_deref()),
        None
    );
}

#[test]
fn origin_lookup_failure_preserves_existing_source_repo() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let mut host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "1.1.0", Some("1.al4")),
    );
    host.fail_origin.insert("copilot-shell".to_string());
    seed_state(
        &layout,
        vec![rpm_component(
            "cosh",
            "copilot-shell",
            "1.0.0-1.al4",
            Ownership::RpmManaged,
        )],
    );
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "cosh",
            Some("copilot-shell"),
            Some("rpm-managed"),
            Some("1.0.0-1.al4"),
            Some("1.1.0-1.al4"),
            ACTION_UPDATE,
            None,
        )],
    );

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.warnings.len(), 1);

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let obj = state
        .find_object(ObjectKind::Component, "cosh")
        .expect("component");
    assert_eq!(obj.version, "1.1.0-1.al4");
    assert_eq!(
        obj.rpm_metadata
            .as_ref()
            .and_then(|m| m.source_repo.as_deref()),
        Some("@System")
    );
}

// ── apply: partial failure ───────────────────────────────────────────────────

#[test]
fn partial_transaction_failure_is_reported_and_not_claimed_as_success() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let mut host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "1.1.0", Some("1.al4")),
    );
    // The second component update fails at the dnf layer.
    host.fail_update.insert("agent-sec-core".to_string());
    // Seed the component that upgrades cleanly so its update is recorded.
    seed_state(
        &layout,
        vec![rpm_component(
            "cosh",
            "copilot-shell",
            "1.0.0-1.al4",
            Ownership::RpmManaged,
        )],
    );

    let components = vec![
        component_check(
            "cosh",
            Some("copilot-shell"),
            Some("rpm-managed"),
            Some("1.0.0-1.al4"),
            Some("1.1.0-1.al4"),
            ACTION_UPDATE,
            None,
        ),
        component_check(
            "sec-core",
            Some("agent-sec-core"),
            Some("rpm-managed"),
            Some("1.0.0-1.al4"),
            Some("1.1.0-1.al4"),
            ACTION_UPDATE,
            None,
        ),
    ];
    let plan = build_plan(None, &cli_noop(), &components);

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(
        result.status, STATUS_PARTIAL,
        "must not claim clean success"
    );
    assert_eq!(result.updated.len(), 1);
    assert_eq!(result.updated[0].name, "cosh");
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].name, "sec-core");

    // The durable operation record must reflect the partial outcome, not `ok`,
    // even though the cosh update landed in state.
    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    assert_eq!(
        state.operations.last().map(|op| op.status.as_str()),
        Some("partial"),
        "a partial upgrade must not record an `ok` operation"
    );
}

// ── apply: state drift under the lock ────────────────────────────────────────

/// A default install whose name already belongs to a raw-managed component in
/// state (a concurrent install after planning) must be refused before dnf runs.
#[test]
fn install_does_not_overwrite_existing_raw_managed_component() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default()
        .with_installed("agent-memory", info("agent-memory", "1.0.0", Some("1.al4")));
    // A raw-managed component of the same name appeared in state after planning.
    seed_state(
        &layout,
        vec![rpm_component(
            "agent-memory",
            "agent-memory",
            "0.9.0",
            Ownership::RawManaged,
        )],
    );

    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "agent-memory",
            Some("agent-memory"),
            None,
            None,
            None,
            ACTION_INSTALL,
            None,
        )],
    );

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_FAILED, "nothing landed cleanly");
    assert!(result.installed.is_empty());
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].name, "agent-memory");
    assert!(
        host.txn_calls().is_empty(),
        "stale state must block dnf before RPM mutation"
    );

    // State must still describe the original raw-managed component.
    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let obj = state
        .find_object(ObjectKind::Component, "agent-memory")
        .expect("component");
    assert_eq!(obj.effective_ownership(), Ownership::RawManaged);
    assert_eq!(obj.version, "0.9.0");
}

/// A component update whose state row vanished after planning must be rejected
/// under the install lock before dnf mutates the host.
#[test]
fn update_state_drift_missing_object_is_reported_as_error() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default()
        .with_installed(
            "copilot-shell",
            info("copilot-shell", "1.1.0", Some("1.al4")),
        )
        .with_origin("copilot-shell", "alinux4-agentic-os");
    // No state is seeded, so the planned update has no row to refresh.

    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "cosh",
            Some("copilot-shell"),
            Some("rpm-managed"),
            Some("1.0.0-1.al4"),
            Some("1.1.0-1.al4"),
            ACTION_UPDATE,
            None,
        )],
    );

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(
        result.status, STATUS_FAILED,
        "a dnf success with no state refresh must not be reported as ok"
    );
    assert!(
        result.updated.is_empty(),
        "drifted update is not reported updated"
    );
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].name, "cosh");
    assert!(
        host.txn_calls().is_empty(),
        "stale state must block dnf before RPM mutation"
    );
}

#[test]
fn observed_default_update_missing_from_state_is_recorded_as_rpm_observed() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default()
        .with_installed(
            "copilot-shell",
            info("copilot-shell", "1.1.0", Some("1.al4")),
        )
        .with_origin("copilot-shell", "alinux4-agentic-os");
    let mut observed_default = component_check(
        "cosh",
        Some("copilot-shell"),
        Some("rpm-observed"),
        Some("1.0.0-1.al4"),
        Some("1.1.0-1.al4"),
        ACTION_UPDATE,
        None,
    );
    observed_default.absent_from_state = true;
    let plan = build_plan(None, &cli_noop(), &[observed_default]);

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.updated.len(), 1);
    assert!(result.errors.is_empty());

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let obj = state
        .find_object(ObjectKind::Component, "cosh")
        .expect("observed default recorded");
    assert_eq!(obj.effective_ownership(), Ownership::RpmObserved);
    assert_eq!(obj.status, ObjectStatus::Adopted);
    assert!(obj.adopted);
    assert!(!obj.managed);
    assert_eq!(obj.version, "1.1.0-1.al4");
    assert_eq!(
        obj.rpm_metadata.as_ref().map(|m| m.package_name.as_str()),
        Some("copilot-shell")
    );
    assert_eq!(
        obj.rpm_metadata
            .as_ref()
            .and_then(|m| m.source_repo.as_deref()),
        Some("alinux4-agentic-os")
    );
}

#[test]
fn observed_default_noop_missing_from_state_is_recorded_without_dnf() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "1.1.0", Some("1.al4")),
    );
    let mut observed_default = component_check(
        "cosh",
        Some("copilot-shell"),
        Some("rpm-observed"),
        Some("1.1.0-1.al4"),
        None,
        ACTION_NOOP,
        None,
    );
    observed_default.absent_from_state = true;
    let plan = build_plan(None, &cli_noop(), &[observed_default]);

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_OK);
    assert!(result.updated.is_empty(), "no package update happened");
    assert_eq!(result.recorded.len(), 1);
    assert_eq!(result.recorded[0].name, "cosh");
    assert_eq!(result.recorded[0].version.as_deref(), Some("1.1.0-1.al4"));
    assert!(
        host.txn_calls().is_empty(),
        "recording an observed default must not run dnf"
    );

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let obj = state
        .find_object(ObjectKind::Component, "cosh")
        .expect("observed default recorded");
    assert_eq!(obj.effective_ownership(), Ownership::RpmObserved);
    assert_eq!(obj.status, ObjectStatus::Adopted);
    assert_eq!(obj.version, "1.1.0-1.al4");
    assert_eq!(
        obj.rpm_metadata.as_ref().map(|m| m.package_name.as_str()),
        Some("copilot-shell")
    );
}

// ── audit for CLI-only / component-less runs ─────────────────────────────────

/// A CLI-only upgrade really mutates the system (dnf updates the CLI RPM) but
/// records no ANOLISA component object; it must still leave a durable operation
/// record so the change is auditable via `anolisa logs`, not a silent `ok`.
#[test]
fn cli_only_upgrade_records_durable_audit() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host =
        FakeHost::default().with_installed("anolisa", info("anolisa", "1.0.0", Some("1.al4")));
    let cli = cli_check(
        ACTION_UPDATE,
        Some("anolisa"),
        Some("0.5.0-1.al4"),
        Some("1.0.0-1.al4"),
        None,
    );
    let plan = build_plan(None, &cli, &[]);

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.updated.len(), 1);
    assert_eq!(result.updated[0].name, "anolisa");
    assert!(result.installed.is_empty());

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    assert_eq!(
        state.operations.last().map(|op| op.status.as_str()),
        Some("ok"),
        "a CLI-only upgrade must still leave an operation record"
    );
    assert!(
        state
            .find_object(ObjectKind::Component, "anolisa")
            .is_none(),
        "the CLI is not recorded as a component object"
    );
}

/// A CLI update that succeeds while a component transaction fails records a
/// `partial` operation, even though no component state object changed.
#[test]
fn cli_success_with_component_failure_records_partial_audit() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let mut host =
        FakeHost::default().with_installed("anolisa", info("anolisa", "1.0.0", Some("1.al4")));
    host.fail_update.insert("copilot-shell".to_string());

    let cli = cli_check(
        ACTION_UPDATE,
        Some("anolisa"),
        Some("0.5.0-1.al4"),
        Some("1.0.0-1.al4"),
        None,
    );
    let components = vec![component_check(
        "cosh",
        Some("copilot-shell"),
        Some("rpm-managed"),
        Some("1.0.0-1.al4"),
        Some("1.1.0-1.al4"),
        ACTION_UPDATE,
        None,
    )];
    let plan = build_plan(None, &cli, &components);

    let result = run_upgrade_with_deps(&ctx, &layout, &plan, &host, &host, true, false, COMMAND)
        .expect("apply");
    assert_eq!(result.status, STATUS_PARTIAL);
    assert_eq!(result.updated.len(), 1);
    assert_eq!(result.updated[0].name, "anolisa");
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].name, "cosh");

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    assert_eq!(
        state.operations.last().map(|op| op.status.as_str()),
        Some("partial"),
        "CLI success + component failure must record a partial operation"
    );
}
