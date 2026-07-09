//! Unit tests for `update --check`. Driven entirely through the injected
//! [`PackageQuery`] fake plus in-memory state, so no live rpmdb/dnf is required.

use super::*;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use anolisa_platform::pkg_query::{PackageInfo, PackageVersion};
use anolisa_platform::pkg_transaction::{PackageTransaction, PackageTransactionError};

use anolisa_core::state::{ObjectStatus, RpmMetadata, SubscriptionScope};

use super::super::UpdateArgs;
use super::render::build_motd;

/// In-memory host implementing both [`PackageQuery`] and [`PackageTransaction`]:
/// the check must only ever call the query side, so any transaction call is a
/// routing bug the counter surfaces.
#[derive(Default)]
struct FakeHost {
    /// exe/capability path → owning package names.
    provides: HashMap<String, Vec<String>>,
    /// package → installed EVR info.
    installed: HashMap<String, PackageInfo>,
    /// package → repo candidate infos.
    available: HashMap<String, Vec<PackageInfo>>,
    /// packages whose `query_available` returns an error.
    available_errors: HashSet<String>,
    /// packages whose `query_installed` reports a multi-version drift.
    installed_multi: HashSet<String>,
    txn_calls: Cell<usize>,
}

impl FakeHost {
    fn with_cli_noop() -> Self {
        // A CLI that is RPM-owned and already current, so CLI status never
        // perturbs component-focused assertions.
        let mut host = FakeHost::default();
        host.provides
            .insert("/usr/bin/anolisa".to_string(), vec!["anolisa".to_string()]);
        host.installed.insert(
            "anolisa".to_string(),
            info("anolisa", "1.0.0", Some("1.al4")),
        );
        host
    }
}

impl PackageQuery for FakeHost {
    fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
        if self.installed_multi.contains(package) {
            return Err(PackageQueryError::UnexpectedOutput {
                command: "rpm".to_string(),
                detail: "2 installed versions".to_string(),
            });
        }
        Ok(self.installed.get(package).cloned())
    }

    fn query_available(&self, package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
        if self.available_errors.contains(package) {
            return Err(PackageQueryError::QueryFailed {
                command: "dnf".to_string(),
                code: Some(1),
                stderr: "repo unreachable".to_string(),
            });
        }
        Ok(self.available.get(package).cloned().unwrap_or_default())
    }

    fn what_provides_installed(&self, capability: &str) -> Result<Vec<String>, PackageQueryError> {
        Ok(self.provides.get(capability).cloned().unwrap_or_default())
    }
}

impl PackageTransaction for FakeHost {
    fn install(&self, _package: &str) -> Result<(), PackageTransactionError> {
        self.txn_calls.set(self.txn_calls.get() + 1);
        Ok(())
    }
    fn update(&self, _package: &str) -> Result<(), PackageTransactionError> {
        self.txn_calls.set(self.txn_calls.get() + 1);
        Ok(())
    }
    fn remove(&self, _package: &str) -> Result<(), PackageTransactionError> {
        self.txn_calls.set(self.txn_calls.get() + 1);
        Ok(())
    }
}

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

fn rpm_component(
    component: &str,
    package: &str,
    evr: &str,
    ownership: Ownership,
) -> InstalledObject {
    InstalledObject {
        kind: ObjectKind::Component,
        name: component.to_string(),
        version: evr.to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("rpm".to_string()),
        ownership: Some(ownership),
        rpm_metadata: Some(RpmMetadata {
            package_name: package.to_string(),
            evr: Some(evr.to_string()),
            arch: Some("x86_64".to_string()),
            source_repo: Some("@System".to_string()),
        }),
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

fn raw_component(component: &str, version: &str) -> InstalledObject {
    InstalledObject {
        kind: ObjectKind::Component,
        name: component.to_string(),
        version: version.to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: Some("https://example.com/x".to_string()),
        raw_package: None,
        install_backend: Some("raw".to_string()),
        ownership: Some(Ownership::RawManaged),
        rpm_metadata: None,
        installed_at: "2026-06-01T10:00:00Z".to_string(),
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

fn state_with(objects: Vec<InstalledObject>) -> InstalledState {
    let mut state = InstalledState::default();
    for obj in objects {
        state.upsert_object(obj);
    }
    state
}

fn run(
    host: &FakeHost,
    installed: &InstalledState,
    target: Option<TargetProfile>,
    target_name: Option<String>,
) -> UpdateCheckReport {
    run_update_check(CheckInputs {
        installed,
        query: host,
        cli_exe_path: "/usr/bin/anolisa",
        arch: "x86_64",
        target_name,
        target,
    })
}

fn system_ctx() -> CliContext {
    CliContext {
        install_mode: crate::context::InstallMode::System,
        prefix: None,
        json: false,
        dry_run: false,
        verbose: false,
        quiet: true,
        no_color: true,
    }
}

// ── CLI parse surface ───────────────────────────────────────────────

use clap::Parser;

#[test]
fn update_check_parse_accepts_check_flag() {
    let args = UpdateArgs::try_parse_from(["update", "--check"]).expect("parse");
    assert!(args.check);
    assert!(args.component.is_none());
    assert!(args.command.is_none());
}

#[test]
fn update_check_rejects_self_target() {
    // With `--check` present, clap binds `self` to the positional rather than
    // dispatching the subcommand, so the rejection lands in `handle`.
    let args = UpdateArgs::try_parse_from(["update", "--check", "self"]).expect("parse");
    assert!(args.check);
    let err =
        super::super::handle(args, &system_ctx()).expect_err("`--check self` must be rejected");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
}

#[test]
fn update_check_parse_motd_requires_check() {
    UpdateArgs::try_parse_from(["update", "--motd"])
        .expect_err("--motd is only valid together with --check");
}

#[test]
fn update_check_parse_target_requires_check() {
    UpdateArgs::try_parse_from(["update", "--target", "image-v1.0"])
        .expect_err("--target is only valid together with --check");
}

#[test]
fn update_check_rejects_component_argument() {
    let args = UpdateArgs {
        component: Some("cosh".to_string()),
        command: None,
        check: true,
        motd: false,
        refresh: false,
        target: None,
    };
    let err = super::super::handle(args, &system_ctx())
        .expect_err("component + --check must be rejected");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("no component argument"));
}

// ── report shape and detection ──────────────────────────────────────

#[test]
fn update_check_json_shape_has_cli_components_summary() {
    let host = FakeHost::with_cli_noop();
    let state = state_with(vec![]);
    let report = run(&host, &state, None, None);
    let value = serde_json::to_value(&report).expect("serialize");
    assert!(value.get("cli").is_some(), "cli field present");
    assert!(
        value.get("components").is_some(),
        "components field present"
    );
    assert!(value.get("summary").is_some(), "summary field present");
    assert_eq!(value["backend"], "rpm");
    // A current CLI and no components → nothing to do.
    assert_eq!(value["upgrade_available"], false);
    assert_eq!(value["action_required"], false);
}

#[test]
fn update_check_rpm_component_update_candidate() {
    let mut host = FakeHost::with_cli_noop();
    host.installed.insert(
        "copilot-shell".to_string(),
        info("copilot-shell", "1.0.0", Some("1.al4")),
    );
    host.available.insert(
        "copilot-shell".to_string(),
        vec![info("copilot-shell", "1.1.0", Some("1.al4"))],
    );
    let state = state_with(vec![rpm_component(
        "cosh",
        "copilot-shell",
        "1.0.0-1.al4",
        Ownership::RpmObserved,
    )]);

    let report = run(&host, &state, None, None);
    let item = report
        .components
        .iter()
        .find(|c| c.component == "cosh")
        .expect("component present");
    assert_eq!(item.action, ACTION_UPDATE);
    assert_eq!(item.installed.as_deref(), Some("1.0.0-1.al4"));
    assert_eq!(item.available.as_deref(), Some("1.1.0-1.al4"));
    assert_eq!(item.ownership.as_deref(), Some("rpm-observed"));
    assert_eq!(report.summary.updates, 1);
    assert!(report.upgrade_available);
    assert!(report.action_required);
    assert_eq!(
        host.txn_calls.get(),
        0,
        "check must never run a transaction"
    );
}

/// Regression for the P1 semver bug: real RPM EVRs that semver cannot parse
/// (two-segment version) must still be detected as upgradable.
#[test]
fn update_check_detects_non_semver_rpm_upgrade() {
    let mut host = FakeHost::with_cli_noop();
    host.installed.insert(
        "copilot-shell".to_string(),
        info("copilot-shell", "0.5", Some("1.al4")),
    );
    host.available.insert(
        "copilot-shell".to_string(),
        vec![info("copilot-shell", "1.0.0", Some("1.al4"))],
    );
    let state = state_with(vec![rpm_component(
        "cosh",
        "copilot-shell",
        "0.5-1.al4",
        Ownership::RpmObserved,
    )]);

    let report = run(&host, &state, None, None);
    let item = &report.components[0];
    assert_eq!(item.action, ACTION_UPDATE);
    assert_eq!(item.available.as_deref(), Some("1.0.0-1.al4"));
    assert_eq!(report.summary.updates, 1);
}

#[test]
fn update_check_rpm_component_up_to_date_is_noop() {
    let mut host = FakeHost::with_cli_noop();
    host.installed.insert(
        "copilot-shell".to_string(),
        info("copilot-shell", "1.1.0", Some("1.al4")),
    );
    // Repo only offers the same version.
    host.available.insert(
        "copilot-shell".to_string(),
        vec![info("copilot-shell", "1.1.0", Some("1.al4"))],
    );
    let state = state_with(vec![rpm_component(
        "cosh",
        "copilot-shell",
        "1.1.0-1.al4",
        Ownership::RpmManaged,
    )]);

    let report = run(&host, &state, None, None);
    let item = &report.components[0];
    assert_eq!(item.action, ACTION_NOOP);
    assert_eq!(report.summary.updates, 0);
}

#[test]
fn update_check_raw_component_is_unsupported() {
    let host = FakeHost::with_cli_noop();
    let state = state_with(vec![raw_component("tokenless", "0.5.0")]);

    let report = run(&host, &state, None, None);
    let item = &report.components[0];
    assert_eq!(item.action, ACTION_UNSUPPORTED_RPM);
    assert_eq!(item.ownership.as_deref(), Some("raw-managed"));
    assert_eq!(report.summary.unsupported, 1);
    assert_eq!(report.summary.updates, 0);
    assert_eq!(
        host.txn_calls.get(),
        0,
        "raw component must not run a transaction"
    );
}

#[test]
fn update_check_missing_default_reports_install() {
    let host = FakeHost::with_cli_noop();
    let state = state_with(vec![]);
    let profile = TargetProfile {
        default_components: vec!["cosh".to_string(), "sec-core".to_string()],
    };

    let report = run(&host, &state, Some(profile), Some("image-v1.0".to_string()));
    assert_eq!(report.target.as_deref(), Some("image-v1.0"));
    assert_eq!(report.summary.missing_defaults, 2);
    assert!(
        report.components.iter().all(|c| c.action == ACTION_INSTALL),
        "absent defaults must be reported as installable"
    );
    // Installs are not "upgrades", but they still require action.
    assert!(!report.upgrade_available);
    assert!(report.action_required);
}

#[test]
fn update_check_present_default_is_not_reported() {
    let mut host = FakeHost::with_cli_noop();
    host.installed.insert(
        "copilot-shell".to_string(),
        info("copilot-shell", "1.0.0", Some("1.al4")),
    );
    let state = state_with(vec![rpm_component(
        "cosh",
        "copilot-shell",
        "1.0.0-1.al4",
        Ownership::RpmManaged,
    )]);
    let profile = TargetProfile {
        default_components: vec!["cosh".to_string()],
    };

    let report = run(&host, &state, Some(profile), Some("image-v1.0".to_string()));
    assert_eq!(report.summary.missing_defaults, 0);
}

#[test]
fn update_check_repo_query_failure_is_item_error() {
    let mut host = FakeHost::with_cli_noop();
    host.installed.insert(
        "copilot-shell".to_string(),
        info("copilot-shell", "1.0.0", Some("1.al4")),
    );
    host.available_errors.insert("copilot-shell".to_string());
    let state = state_with(vec![rpm_component(
        "cosh",
        "copilot-shell",
        "1.0.0-1.al4",
        Ownership::RpmObserved,
    )]);

    let report = run(&host, &state, None, None);
    let item = &report.components[0];
    assert_eq!(item.action, ACTION_ERROR);
    assert!(item.error.is_some());
    assert_eq!(report.summary.errors, 1);
}

#[test]
fn update_check_component_missing_from_rpmdb_is_item_error() {
    let host = FakeHost::with_cli_noop();
    // No installed entry for the package → rpmdb miss.
    let state = state_with(vec![rpm_component(
        "cosh",
        "copilot-shell",
        "1.0.0-1.al4",
        Ownership::RpmObserved,
    )]);

    let report = run(&host, &state, None, None);
    let item = &report.components[0];
    assert_eq!(item.action, ACTION_ERROR);
    assert!(item.error.as_deref().unwrap().contains("forget"));
    assert_eq!(report.summary.errors, 1);
}

#[test]
fn update_check_cli_update_available() {
    let mut host = FakeHost::default();
    host.provides
        .insert("/usr/bin/anolisa".to_string(), vec!["anolisa".to_string()]);
    host.installed.insert(
        "anolisa".to_string(),
        info("anolisa", "0.5.0", Some("1.al4")),
    );
    host.available.insert(
        "anolisa".to_string(),
        vec![info("anolisa", "1.0.0", Some("1.al4"))],
    );
    let state = state_with(vec![]);

    let report = run(&host, &state, None, None);
    assert_eq!(report.cli.action, ACTION_UPDATE);
    assert_eq!(report.cli.package.as_deref(), Some("anolisa"));
    assert_eq!(report.cli.installed.as_deref(), Some("0.5.0-1.al4"));
    assert_eq!(report.cli.available.as_deref(), Some("1.0.0-1.al4"));
    assert_eq!(report.summary.updates, 1);
    assert!(report.upgrade_available);
}

#[test]
fn update_check_cli_not_rpm_owned_is_unsupported() {
    // No provider for the exe path → not RPM-owned.
    let host = FakeHost::default();
    let state = state_with(vec![]);
    let report = run(&host, &state, None, None);
    assert_eq!(report.cli.action, ACTION_UNSUPPORTED);
    assert!(report.cli.package.is_none());
    assert_eq!(report.summary.unsupported, 1);
}

// ── MOTD rendering ──────────────────────────────────────────────────

#[test]
fn update_check_motd_text_lists_upgrades_and_installs() {
    let report = UpdateCheckReport {
        target: Some("image-v1.0".to_string()),
        backend: "rpm".to_string(),
        upgrade_available: true,
        action_required: true,
        cli: cli_unsupported("test"),
        components: Vec::new(),
        summary: CheckSummary {
            updates: 1,
            missing_defaults: 1,
            unsupported: 0,
            errors: 0,
        },
    };
    let text = build_motd(&report).expect("motd text present");
    assert!(text.contains("ANOLISA toolchain update is available."));
    assert!(text.contains("1 component can be upgraded"));
    assert!(text.contains("1 new default component can be installed"));
    assert!(text.contains("Run: sudo anolisa upgrade"));
}

#[test]
fn update_check_motd_is_silent_when_nothing_to_do() {
    let report = UpdateCheckReport {
        target: None,
        backend: "rpm".to_string(),
        upgrade_available: false,
        action_required: false,
        cli: CliCheck {
            package: Some("anolisa".to_string()),
            installed: Some("1.0.0-1.al4".to_string()),
            available: None,
            action: ACTION_NOOP.to_string(),
            error: None,
        },
        components: Vec::new(),
        summary: CheckSummary::default(),
    };
    assert!(build_motd(&report).is_none());
}

// ── cache ───────────────────────────────────────────────────────────

fn report_with_target(target: Option<&str>) -> UpdateCheckReport {
    UpdateCheckReport {
        target: target.map(str::to_string),
        backend: "rpm".to_string(),
        upgrade_available: true,
        action_required: true,
        cli: cli_unsupported("test"),
        components: Vec::new(),
        summary: CheckSummary {
            updates: 1,
            ..Default::default()
        },
    }
}

#[test]
fn update_check_cache_round_trips_and_respects_ttl() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let path = cache_path(&layout);

    write_cache(&path, &report_with_target(None)).expect("write cache");

    let cache = read_cache(&path).expect("cache readable");
    assert_eq!(cache.report.summary.updates, 1);
    assert!(is_fresh(&cache.generated_at), "just-written cache is fresh");

    // A far-past timestamp is stale.
    assert!(!is_fresh("2000-01-01T00:00:00Z"));
}

/// A cached report must not be reused for a different (or absent) target — the
/// MOTD fast path is keyed by target as well as freshness.
#[test]
fn update_check_cache_usable_requires_matching_target() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let path = cache_path(&layout);
    write_cache(&path, &report_with_target(Some("image-v1.0"))).expect("write cache");
    let cache = read_cache(&path).expect("cache readable");

    assert!(
        cache_is_usable(&cache, Some("image-v1.0")),
        "same target reuses the cache"
    );
    assert!(
        !cache_is_usable(&cache, None),
        "a plain MOTD must not reuse a targeted report"
    );
    assert!(
        !cache_is_usable(&cache, Some("image-v2.0")),
        "a different target must not reuse the report"
    );
}

// ── repo config read-only + target profile ──────────────────────────

/// `--check` must load repo config without writing it. Here the config already
/// exists locally, so the read-only load returns it and touches nothing else;
/// the missing-config no-write guarantee is covered by
/// `repo_config::tests::load_dry_run_fetches_without_writing`.
#[test]
fn update_check_read_only_load_uses_existing_config_without_writing() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    std::fs::create_dir_all(&layout.etc_dir).expect("mkdir etc");
    let repo_toml = layout.etc_dir.join("repo.toml");
    std::fs::write(
        &repo_toml,
        "schema_version = 1\ndefault_backend = \"rpm\"\n\n[backends.rpm]\nbase_url = \"https://repo.example/$os/$arch/os\"\n",
    )
    .expect("write repo.toml");

    let cfg = load_repo_config_read_only(&layout).expect("read-only load");
    assert_eq!(cfg.default_backend, "rpm");
    // No scratch file was left behind by the read-only path.
    assert!(!repo_toml.with_extension("toml.tmp").exists());
}

#[test]
fn update_check_target_profile_parses_default_components() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let dir = layout.etc_dir.join("profiles");
    std::fs::create_dir_all(&dir).expect("mkdir profiles");
    std::fs::write(
        dir.join("image-v1.0.toml"),
        "schema_version = 1\nname = \"image-v1.0\"\ndefault_components = [\"cosh\", \"sec-core\"]\n",
    )
    .expect("write profile");

    let profile = load_target_profile(&layout, "image-v1.0").expect("profile loads");
    assert_eq!(profile.default_components, vec!["cosh", "sec-core"]);
}

#[test]
fn update_check_target_profile_rejects_traversal() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let err = load_target_profile(&layout, "../escape").expect_err("must reject traversal");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
}
