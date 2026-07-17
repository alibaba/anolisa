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

// `Activity` and `ProgressReporter` reach the tests through the module glob
// (`use super::*`); `NoopReporter` is the only progress item not re-imported by
// the upgrade module, so it is brought in explicitly here.
use crate::progress::NoopReporter;

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
    /// packages whose installed query reports multiple/unparseable rows.
    ambiguous_after: HashSet<String>,
    /// packages whose installed query fails.
    fail_query: HashSet<String>,
    /// packages whose next installed query fails once, then recovers.
    fail_query_once: RefCell<HashSet<String>>,
    /// package -> source repo returned by `installed_origin`.
    origins: HashMap<String, String>,
    /// packages whose `installed_origin` returns an error.
    fail_origin: HashSet<String>,
    /// ordered transaction log (`"update:<pkg>"` / `"install:<pkg>"`).
    calls: RefCell<Vec<String>>,
    /// package names inspected through `query_installed`.
    query_calls: RefCell<Vec<String>>,
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

    fn query_calls(&self) -> Vec<String> {
        self.query_calls.borrow().clone()
    }
}

impl PackageQuery for FakeHost {
    fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
        self.query_calls.borrow_mut().push(package.to_string());
        if self.ambiguous_after.contains(package) {
            return Err(PackageQueryError::UnexpectedOutput {
                command: "rpm".to_string(),
                detail: "2 installed versions".to_string(),
            });
        }
        if self.fail_query.contains(package) || self.fail_query_once.borrow_mut().remove(package) {
            return Err(PackageQueryError::QueryFailed {
                command: "rpm".to_string(),
                code: Some(1),
                stderr: "rpmdb read failed".to_string(),
            });
        }
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

fn info_with_arch(name: &str, version: &str, release: Option<&str>, arch: &str) -> PackageInfo {
    let mut package = info(name, version, release);
    package.arch = arch.to_string();
    package
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
        backfill_rpm_metadata: false,
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

    let err = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        false,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        false,
        true,
        COMMAND,
        &NoopReporter,
    )
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

#[test]
fn upgrade_rejects_pending_rpm_claim_before_any_transaction() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
        .expect("begin pending install");
    let host = FakeHost::default();
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "cosh",
            Some("copilot-shell"),
            None,
            None,
            None,
            ACTION_INSTALL,
            None,
        )],
    );

    for dry_run in [true, false] {
        let err = run_upgrade_with_deps(
            &ctx,
            &layout,
            &plan,
            &host,
            &host,
            true,
            dry_run,
            COMMAND,
            &NoopReporter,
        )
        .expect_err("pending install must block the whole upgrade");
        assert!(err.reason().contains("repair cosh"));
    }
    assert!(host.txn_calls().is_empty());
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
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

// ── batch RPM state reconciliation (issue #1472) ───────────────────────────

#[test]
fn empty_plan_reconciles_all_drifted_rpm_components() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let components = [
        ("sec-core", "agent-sec-core", "0.7.1", "0.8.0"),
        ("agentsight", "agentsight", "0.7.1", "0.8.0"),
        ("tokenless", "tokenless", "0.6.1", "0.7.0"),
        ("agent-memory", "agent-memory", "0.2.1", "0.2.3"),
        ("cosh", "copilot-shell", "2.6.1", "2.7.0"),
        ("skillfs", "skillfs", "0.3.2", "0.3.3"),
    ];
    let objects = components
        .iter()
        .map(|(name, package, old, _)| {
            rpm_component(
                name,
                package,
                &format!("{old}-1.alnx4"),
                Ownership::RpmManaged,
            )
        })
        .collect();
    seed_state(&layout, objects);

    let mut host = FakeHost::default();
    for (_, package, _, new) in components {
        host.installed
            .insert(package.to_string(), info(package, new, Some("1.alnx4")));
    }
    let plan = build_plan(None, &cli_noop(), &[]);

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("reconcile empty plan");

    assert_eq!(result.status, STATUS_OK);
    assert!(result.updated.is_empty());
    assert_eq!(result.reconciled.len(), 6);
    assert!(host.txn_calls().is_empty(), "reconcile must not call dnf");

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    assert_eq!(state.operations.len(), 1, "one upgrade operation");
    let operation_id = &state.operations[0].id;
    for (name, _package, _old, new) in components {
        let expected = format!("{new}-1.alnx4");
        let object = state
            .find_object(ObjectKind::Component, name)
            .expect("reconciled component");
        let metadata = object.rpm_metadata.as_ref().expect("rpm metadata");
        assert_eq!(object.version, expected);
        assert_eq!(metadata.evr.as_deref(), Some(expected.as_str()));
        assert_eq!(metadata.arch.as_deref(), Some("x86_64"));
        assert_eq!(
            object.last_operation_id.as_deref(),
            Some(operation_id.as_str())
        );
    }
}

#[test]
fn empty_plan_reconciles_stale_component_manifest() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let component = "manifest-sync-test";
    let package = "manifest-sync-test-rpm";
    let evr = "2.0.0-1.alnx4";
    seed_state(
        &layout,
        vec![rpm_component(
            component,
            package,
            evr,
            Ownership::RpmManaged,
        )],
    );

    let source = FsLayout::component_contract_path(&layout.datadir, component);
    std::fs::create_dir_all(source.parent().expect("source parent")).expect("mkdir source");
    std::fs::write(&source, "framework = \"new\"\n").expect("write source contract");
    let snapshot = FsLayout::component_manifest_snapshot_path(&layout.state_dir, component);
    std::fs::create_dir_all(snapshot.parent().expect("snapshot parent")).expect("mkdir snapshot");
    std::fs::write(&snapshot, "framework = \"old\"\n").expect("write stale snapshot");

    let host = FakeHost::default().with_installed(package, info(package, "2.0.0", Some("1.alnx4")));
    let plan = build_plan(None, &cli_noop(), &[]);

    let preview = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        false,
        true,
        COMMAND,
        &NoopReporter,
    )
    .expect("preview manifest drift");

    assert_eq!(preview.reconciled.len(), 1);
    assert_eq!(preview.reconciled[0].reason, "component manifest drift");
    let json = serde_json::to_value(&preview).expect("serialize preview");
    assert_eq!(json["reconciled"][0]["reason"], "component manifest drift");
    assert_eq!(
        std::fs::read_to_string(&snapshot).expect("read unchanged snapshot"),
        "framework = \"old\"\n",
        "dry-run must not refresh the snapshot"
    );

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("reconcile manifest drift");

    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.reconciled.len(), 1);
    assert_eq!(result.reconciled[0].name, component);
    assert!(
        host.txn_calls().is_empty(),
        "manifest sync must not call dnf"
    );
    assert_eq!(
        std::fs::read_to_string(&snapshot).expect("read refreshed snapshot"),
        "framework = \"new\"\n"
    );
}

#[test]
fn empty_plan_reconciles_legacy_rpm_component_and_backfills_metadata() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let mut legacy = rpm_component(
        "cosh",
        "copilot-shell",
        "2.6.1-1.alnx4",
        Ownership::RpmManaged,
    );
    legacy.rpm_metadata = None;
    seed_state(&layout, vec![legacy]);

    let host = FakeHost::default()
        .with_installed(
            "copilot-shell",
            info("copilot-shell", "2.7.0", Some("1.alnx4")),
        )
        .with_origin("copilot-shell", "anolisa");
    let check = component_check(
        "cosh",
        Some("copilot-shell"),
        Some("rpm-managed"),
        Some("2.7.0-1.alnx4"),
        None,
        ACTION_RECONCILE,
        None,
    );
    let plan = build_plan(None, &cli_noop(), &[check]);

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("reconcile legacy component");

    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.reconciled.len(), 1);
    assert_eq!(result.reconciled[0].name, "cosh");
    assert!(host.txn_calls().is_empty());
    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let object = state
        .find_object(ObjectKind::Component, "cosh")
        .expect("cosh");
    let metadata = object.rpm_metadata.as_ref().expect("backfilled metadata");
    assert_eq!(object.version, "2.7.0-1.alnx4");
    assert_eq!(metadata.package_name, "copilot-shell");
    assert_eq!(metadata.evr.as_deref(), Some("2.7.0-1.alnx4"));
    assert_eq!(metadata.arch.as_deref(), Some("x86_64"));
    assert_eq!(metadata.source_repo.as_deref(), Some("anolisa"));
}

#[test]
fn planned_update_backfills_legacy_rpm_metadata() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let mut legacy = rpm_component(
        "cosh",
        "copilot-shell",
        "2.6.1-1.alnx4",
        Ownership::RpmManaged,
    );
    legacy.rpm_metadata = None;
    seed_state(&layout, vec![legacy]);

    let host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "2.7.0", Some("1.alnx4")),
    );
    let mut check = component_check(
        "cosh",
        Some("copilot-shell"),
        Some("rpm-managed"),
        Some("2.6.1-1.alnx4"),
        Some("2.7.0-1.alnx4"),
        ACTION_UPDATE,
        None,
    );
    check.backfill_rpm_metadata = true;
    let plan = build_plan(None, &cli_noop(), &[check]);

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("update legacy component");

    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.updated.len(), 1);
    assert!(result.reconciled.is_empty());
    assert_eq!(host.txn_calls(), vec!["update:copilot-shell"]);
    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let object = state
        .find_object(ObjectKind::Component, "cosh")
        .expect("cosh");
    let metadata = object.rpm_metadata.as_ref().expect("backfilled metadata");
    assert_eq!(object.version, "2.7.0-1.alnx4");
    assert_eq!(metadata.package_name, "copilot-shell");
    assert_eq!(metadata.evr.as_deref(), Some("2.7.0-1.alnx4"));
}

#[test]
fn empty_plan_without_drift_is_true_noop() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let mut object = rpm_component(
        "cosh",
        "copilot-shell",
        "2.7.0-1.alnx4",
        Ownership::RpmManaged,
    );
    object.last_operation_id = Some("op-before".to_string());
    seed_state(&layout, vec![object]);
    let before = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("before");
    let host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "2.7.0", Some("1.alnx4")),
    );
    let plan = build_plan(None, &cli_noop(), &[]);

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("true noop");

    assert_eq!(result.status, STATUS_OK);
    assert!(result.reconciled.is_empty());
    assert!(result.errors.is_empty());
    assert!(host.txn_calls().is_empty());
    let after = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("after");
    assert_eq!(after, before, "a drift-free run must not save state");
}

#[test]
fn planned_update_is_not_reported_as_reconciled() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    seed_state(
        &layout,
        vec![
            rpm_component(
                "cosh",
                "copilot-shell",
                "2.6.1-1.alnx4",
                Ownership::RpmManaged,
            ),
            rpm_component(
                "sec-core",
                "agent-sec-core",
                "0.7.1-1.alnx4",
                Ownership::RpmManaged,
            ),
        ],
    );
    let host = FakeHost::default()
        .with_installed(
            "copilot-shell",
            info("copilot-shell", "2.7.0", Some("1.alnx4")),
        )
        .with_installed(
            "agent-sec-core",
            info("agent-sec-core", "0.8.0", Some("1.alnx4")),
        );
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "cosh",
            Some("copilot-shell"),
            Some("rpm-managed"),
            Some("2.6.1-1.alnx4"),
            Some("2.7.0-1.alnx4"),
            ACTION_UPDATE,
            None,
        )],
    );

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("update and reconcile");

    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.updated.len(), 1);
    assert_eq!(result.updated[0].name, "cosh");
    assert_eq!(result.reconciled.len(), 1);
    assert_eq!(result.reconciled[0].name, "sec-core");
    assert_eq!(host.txn_calls(), vec!["update:copilot-shell"]);

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    assert_eq!(state.operations.len(), 1, "one operation and one save path");
    let operation_id = &state.operations[0].id;
    for name in ["cosh", "sec-core"] {
        assert_eq!(
            state
                .find_object(ObjectKind::Component, name)
                .and_then(|object| object.last_operation_id.as_deref()),
            Some(operation_id.as_str()),
            "all state changes share the upgrade operation",
        );
    }
}

#[test]
fn post_transaction_query_failure_is_not_double_counted() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    seed_state(
        &layout,
        vec![rpm_component(
            "cosh",
            "copilot-shell",
            "2.6.1-1.alnx4",
            Ownership::RpmManaged,
        )],
    );
    let mut host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "2.7.0", Some("1.alnx4")),
    );
    host.fail_query.insert("copilot-shell".to_string());
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "cosh",
            Some("copilot-shell"),
            Some("rpm-managed"),
            Some("2.6.1-1.alnx4"),
            Some("2.7.0-1.alnx4"),
            ACTION_UPDATE,
            None,
        )],
    );

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("post-transaction query failure");

    assert_eq!(host.txn_calls(), vec!["update:copilot-shell"]);
    assert_eq!(host.query_calls(), vec!["copilot-shell", "copilot-shell"]);
    assert_eq!(result.status, STATUS_FAILED);
    assert!(result.updated.is_empty());
    assert!(result.reconciled.is_empty());
    assert_eq!(result.errors.len(), 1, "one component yields one error");
    assert_eq!(result.errors[0].name, "cosh");

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    assert_eq!(
        state
            .find_object(ObjectKind::Component, "cosh")
            .expect("cosh")
            .version,
        "2.6.1-1.alnx4",
    );
    assert_eq!(state.operations.len(), 1);
    assert_eq!(state.operations[0].status, STATUS_FAILED);
}

#[test]
fn post_transaction_query_retry_can_reconcile() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    seed_state(
        &layout,
        vec![rpm_component(
            "cosh",
            "copilot-shell",
            "2.6.1-1.alnx4",
            Ownership::RpmManaged,
        )],
    );
    let host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "2.7.0", Some("1.alnx4")),
    );
    host.fail_query_once
        .borrow_mut()
        .insert("copilot-shell".to_string());
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "cosh",
            Some("copilot-shell"),
            Some("rpm-managed"),
            Some("2.6.1-1.alnx4"),
            Some("2.7.0-1.alnx4"),
            ACTION_UPDATE,
            None,
        )],
    );

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("reconcile after query recovery");

    assert_eq!(host.txn_calls(), vec!["update:copilot-shell"]);
    assert_eq!(host.query_calls(), vec!["copilot-shell", "copilot-shell"]);
    assert_eq!(result.status, STATUS_PARTIAL);
    assert!(result.updated.is_empty());
    assert_eq!(result.reconciled.len(), 1);
    assert_eq!(result.reconciled[0].name, "cosh");
    assert_eq!(result.errors.len(), 1, "the original apply error remains");

    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let object = state
        .find_object(ObjectKind::Component, "cosh")
        .expect("cosh");
    assert_eq!(object.version, "2.7.0-1.alnx4");
    assert_eq!(state.operations.len(), 1);
    assert_eq!(state.operations[0].status, STATUS_PARTIAL);
    assert_eq!(
        object.last_operation_id.as_deref(),
        Some(state.operations[0].id.as_str())
    );
}

#[test]
fn reconcile_preserves_rpm_ownership_and_metadata() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let mut managed = rpm_component(
        "cosh",
        "copilot-shell",
        "2.6.1-1.alnx4",
        Ownership::RpmManaged,
    );
    managed.manifest_digest = Some("sha256:managed".to_string());
    managed.enabled_features = vec!["shell-hooks".to_string()];
    let mut observed = rpm_component(
        "os-skills",
        "os-skills",
        "0.6.1-1.alnx4",
        Ownership::RpmObserved,
    );
    observed.status = ObjectStatus::Adopted;
    observed.installed_at = "2026-05-01T00:00:00Z".to_string();
    observed.manifest_digest = Some("sha256:observed".to_string());
    if let Some(metadata) = observed.rpm_metadata.as_mut() {
        metadata.evr = None;
        metadata.arch = None;
    }
    let mut raw = rpm_component("local-tool", "local-tool", "1.0.0", Ownership::RawManaged);
    raw.enabled_features = vec!["local".to_string()];
    let managed_before = managed.clone();
    let observed_before = observed.clone();
    let raw_before = raw.clone();
    seed_state(&layout, vec![managed, observed, raw]);
    let host = FakeHost::default()
        .with_installed(
            "copilot-shell",
            info_with_arch("copilot-shell", "2.7.0", Some("1.alnx4"), "aarch64"),
        )
        .with_origin("copilot-shell", "anolisa-updates")
        .with_installed(
            "os-skills",
            info_with_arch("os-skills", "0.6.1", Some("1.alnx4"), "noarch"),
        )
        .with_origin("os-skills", "anolisa-updates");
    let plan = build_plan(None, &cli_noop(), &[]);

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("preserving reconcile");

    assert_eq!(result.reconciled.len(), 2);
    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let mut expected_managed = managed_before;
    let actual_managed = state
        .find_object(ObjectKind::Component, "cosh")
        .expect("managed");
    expected_managed.version = "2.7.0-1.alnx4".to_string();
    expected_managed.last_operation_id = actual_managed.last_operation_id.clone();
    let managed_meta = expected_managed.rpm_metadata.as_mut().expect("metadata");
    managed_meta.evr = Some("2.7.0-1.alnx4".to_string());
    managed_meta.arch = Some("aarch64".to_string());
    managed_meta.source_repo = Some("anolisa-updates".to_string());
    assert_eq!(actual_managed, &expected_managed);

    let mut expected_observed = observed_before;
    let actual_observed = state
        .find_object(ObjectKind::Component, "os-skills")
        .expect("observed");
    expected_observed.version = "0.6.1-1.alnx4".to_string();
    expected_observed.last_operation_id = actual_observed.last_operation_id.clone();
    let observed_meta = expected_observed.rpm_metadata.as_mut().expect("metadata");
    observed_meta.evr = Some("0.6.1-1.alnx4".to_string());
    observed_meta.arch = Some("noarch".to_string());
    observed_meta.source_repo = Some("anolisa-updates".to_string());
    assert_eq!(actual_observed, &expected_observed);
    assert_eq!(
        state
            .find_object(ObjectKind::Component, "local-tool")
            .expect("raw"),
        &raw_before,
        "raw-managed state is untouched",
    );
    assert_eq!(
        host.query_calls().into_iter().collect::<HashSet<_>>(),
        HashSet::from(["copilot-shell".to_string(), "os-skills".to_string()]),
        "raw-managed components are not queried",
    );
}

#[test]
fn reconcile_missing_or_ambiguous_rpm_is_not_written() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let objects = vec![
        rpm_component(
            "missing",
            "missing-pkg",
            "1.0.0-1.alnx4",
            Ownership::RpmManaged,
        ),
        rpm_component(
            "ambiguous",
            "ambiguous-pkg",
            "1.0.0-1.alnx4",
            Ownership::RpmManaged,
        ),
        rpm_component(
            "broken",
            "broken-pkg",
            "1.0.0-1.alnx4",
            Ownership::RpmManaged,
        ),
    ];
    seed_state(&layout, objects.clone());
    let mut host = FakeHost::default();
    host.missing_after.insert("missing-pkg".to_string());
    host.ambiguous_after.insert("ambiguous-pkg".to_string());
    host.fail_query.insert("broken-pkg".to_string());
    let plan = build_plan(None, &cli_noop(), &[]);

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("failed reconciliation audit");

    assert_eq!(result.status, STATUS_FAILED);
    assert!(result.reconciled.is_empty());
    assert_eq!(result.errors.len(), 3);
    for (name, package) in [
        ("missing", "missing-pkg"),
        ("ambiguous", "ambiguous-pkg"),
        ("broken", "broken-pkg"),
    ] {
        let error = result
            .errors
            .iter()
            .find(|error| error.name == name)
            .expect("item error");
        assert!(
            error.reason.contains(package),
            "package identity is visible"
        );
    }
    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    assert_eq!(
        state.objects, objects,
        "failed items remain byte-for-byte facts"
    );
    assert_eq!(state.operations.len(), 1);
    assert_eq!(state.operations[0].status, STATUS_FAILED);

    let partial_tmp = tempfile::tempdir().expect("partial tmpdir");
    let partial_ctx = system_ctx(partial_tmp.path().to_path_buf());
    let partial_layout = FsLayout::system(Some(partial_tmp.path().to_path_buf()));
    seed_state(
        &partial_layout,
        vec![
            rpm_component("good", "good-pkg", "1.0.0-1.alnx4", Ownership::RpmManaged),
            rpm_component(
                "missing",
                "missing-pkg",
                "1.0.0-1.alnx4",
                Ownership::RpmManaged,
            ),
        ],
    );
    let mut partial_host =
        FakeHost::default().with_installed("good-pkg", info("good-pkg", "2.0.0", Some("1.alnx4")));
    partial_host.missing_after.insert("missing-pkg".to_string());
    let partial_result = run_upgrade_with_deps(
        &partial_ctx,
        &partial_layout,
        &plan,
        &partial_host,
        &partial_host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("partial reconciliation");
    assert_eq!(partial_result.status, STATUS_PARTIAL);
    assert_eq!(partial_result.reconciled.len(), 1);
    assert_eq!(partial_result.errors.len(), 1);
}

#[test]
fn reconcile_origin_failure_preserves_prior_source() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    seed_state(
        &layout,
        vec![rpm_component(
            "cosh",
            "copilot-shell",
            "2.6.1-1.alnx4",
            Ownership::RpmManaged,
        )],
    );
    let mut host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "2.7.0", Some("1.alnx4")),
    );
    host.fail_origin.insert("copilot-shell".to_string());
    let plan = build_plan(None, &cli_noop(), &[]);

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        true,
        false,
        COMMAND,
        &NoopReporter,
    )
    .expect("origin warning");

    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.reconciled.len(), 1);
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("copilot-shell"));
    let state = InstalledState::load(&layout.state_dir.join("installed.toml")).expect("state");
    let object = state
        .find_object(ObjectKind::Component, "cosh")
        .expect("cosh");
    assert_eq!(object.version, "2.7.0-1.alnx4");
    assert_eq!(
        object
            .rpm_metadata
            .as_ref()
            .and_then(|metadata| metadata.source_repo.as_deref()),
        Some("@System"),
    );
}

#[test]
fn blocked_dry_run_does_not_inspect_rpm_reconciliation() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    seed_state(
        &layout,
        vec![rpm_component(
            "cosh",
            "copilot-shell",
            "2.6.1-1.alnx4",
            Ownership::RpmManaged,
        )],
    );
    let host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "2.7.0", Some("1.alnx4")),
    );
    let plan = build_plan(
        None,
        &cli_noop(),
        &[component_check(
            "broken",
            None,
            Some("rpm-managed"),
            None,
            None,
            ACTION_ERROR,
            Some("planning failed"),
        )],
    );

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        false,
        true,
        COMMAND,
        &NoopReporter,
    )
    .expect("blocked dry-run renders");

    assert_eq!(result.status, STATUS_BLOCKED);
    assert!(result.reconciled.is_empty());
    assert_eq!(result.errors.len(), 1);
    assert!(host.query_calls().is_empty());
    assert!(host.txn_calls().is_empty());
}

#[test]
fn upgrade_dry_run_reports_reconcile_without_writes() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    seed_state(
        &layout,
        vec![rpm_component(
            "cosh",
            "copilot-shell",
            "2.6.1-1.alnx4",
            Ownership::RpmManaged,
        )],
    );
    let state_path = layout.state_dir.join("installed.toml");
    let before = std::fs::read(&state_path).expect("state bytes");
    let host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "2.7.0", Some("1.alnx4")),
    );
    let plan = build_plan(None, &cli_noop(), &[]);

    let result = run_upgrade_with_deps(
        &ctx,
        &layout,
        &plan,
        &host,
        &host,
        false,
        true,
        COMMAND,
        &NoopReporter,
    )
    .expect("dry-run reconcile preview");

    assert!(result.dry_run);
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(result.reconciled.len(), 1);
    assert_eq!(result.reconciled[0].name, "cosh");
    assert_eq!(result.reconciled[0].from, "2.6.1-1.alnx4");
    assert_eq!(result.reconciled[0].to, "2.7.0-1.alnx4");
    let json = serde_json::to_value(&result).expect("serialize result");
    assert_eq!(json["reconciled"][0]["package"], "copilot-shell");
    assert_eq!(json["reconciled"][0]["reason"], "RPM state drift");
    assert!(host.txn_calls().is_empty());
    assert_eq!(std::fs::read(&state_path).expect("state bytes"), before);
    assert!(
        !layout.lock_file.exists(),
        "dry-run must not acquire the install lock"
    );
    let state = InstalledState::load(&state_path).expect("state");
    assert!(state.operations.is_empty());
    assert!(
        state
            .find_object(ObjectKind::Component, "cosh")
            .expect("cosh")
            .last_operation_id
            .is_none()
    );
}

// ── progress reporting (issue #1452) ─────────────────────────────────────────

/// Records the phase messages emitted during the apply loop so tests can assert
/// the exact sequence without a TTY or animated-frame timing.
#[derive(Default)]
struct RecordingReporter {
    messages: RefCell<Vec<String>>,
}

impl RecordingReporter {
    fn messages(&self) -> Vec<String> {
        self.messages.borrow().clone()
    }
}

impl ProgressReporter for RecordingReporter {
    fn report(&self, message: &str) {
        self.messages.borrow_mut().push(message.to_string());
    }
}

/// A real apply reports each transaction item with a reliable `i/total`, then a
/// finalize phase, in order: CLI → component update → install → finalizing.
#[test]
fn apply_reports_each_phase_with_reliable_counts() {
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
    let reporter = RecordingReporter::default();

    let result = run_upgrade_with_deps(
        &ctx, &layout, &plan, &host, &host, true, false, COMMAND, &reporter,
    )
    .expect("apply");
    assert_eq!(result.status, STATUS_OK);
    assert_eq!(
        reporter.messages(),
        vec![
            "Upgrading anolisa (1/3)...".to_string(),
            "Upgrading cosh (2/3)...".to_string(),
            "Installing agent-memory (3/3)...".to_string(),
            "Finalizing ANOLISA state...".to_string(),
        ],
        "each transaction reports a reliable i/total, then finalizing"
    );
}

/// A dry-run plans only; it must not report any apply-phase progress.
#[test]
fn dry_run_reports_no_apply_phase() {
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
    let reporter = RecordingReporter::default();

    let result = run_upgrade_with_deps(
        &ctx, &layout, &plan, &host, &host, false, true, COMMAND, &reporter,
    )
    .expect("dry-run");
    assert!(result.dry_run);
    assert!(
        reporter.messages().is_empty(),
        "dry-run must not report an apply phase that never happens"
    );
}

/// A per-item transaction failure still reports the item (its message is emitted
/// before the failing `dnf` call) and still reaches the finalize phase, so the
/// reporter contract holds on the error path.
#[test]
fn apply_reports_phases_even_when_an_item_fails() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let mut host = FakeHost::default().with_installed(
        "copilot-shell",
        info("copilot-shell", "1.1.0", Some("1.al4")),
    );
    host.fail_update.insert("agent-sec-core".to_string());
    // Both components are seeded so they are authorized against locked state and
    // both reach the `dnf` layer; only the second one's transaction fails there.
    seed_state(
        &layout,
        vec![
            rpm_component(
                "cosh",
                "copilot-shell",
                "1.0.0-1.al4",
                Ownership::RpmManaged,
            ),
            rpm_component(
                "sec-core",
                "agent-sec-core",
                "1.0.0-1.al4",
                Ownership::RpmManaged,
            ),
        ],
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
    let reporter = RecordingReporter::default();

    let result = run_upgrade_with_deps(
        &ctx, &layout, &plan, &host, &host, true, false, COMMAND, &reporter,
    )
    .expect("apply");
    assert_eq!(result.status, STATUS_PARTIAL);
    assert_eq!(
        reporter.messages(),
        vec![
            "Upgrading cosh (1/2)...".to_string(),
            "Upgrading sec-core (2/2)...".to_string(),
            "Finalizing ANOLISA state...".to_string(),
        ],
        "a failing item is still announced and the finalize phase still runs"
    );
}

/// A blocked plan (planning error) never reaches the apply loop, so no phase is
/// reported — real execution aborts before any `dnf` transaction.
#[test]
fn blocked_plan_reports_no_apply_phase() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path().to_path_buf());
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let host = FakeHost::default();
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
    let reporter = RecordingReporter::default();

    let result = run_upgrade_with_deps(
        &ctx, &layout, &plan, &host, &host, true, false, COMMAND, &reporter,
    )
    .expect("blocked plan renders");
    assert_eq!(result.status, STATUS_BLOCKED);
    assert!(
        reporter.messages().is_empty(),
        "a blocked plan must not report an apply phase"
    );
}
