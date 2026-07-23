//! RPM-family tests for the `install` command: candidate resolution and the
//! delegated install pipeline (decision table I2/I3/I6–I9/I11).

use super::super::tests::*;

use anolisa_core::domain::{ManagementRelation, ProviderBinding};
use anolisa_core::state::{
    InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
    Ownership, RpmMetadata,
};
use anolisa_core::transaction::{Transaction, TransactionOutcomeStatus};
use anolisa_platform::fs_layout::FsLayout;

use crate::commands::common;
use crate::commands::tier1::rpm_install;

#[test]
fn candidates_cli_override_matching_package_map_is_accepted() {
    let repo = repo_with_rpm_map(&[("cosh", "site-copilot")]);
    let backend = repo.backends.get("rpm");
    let q = FakeQuery::default();
    let got = rpm_package_candidates(Some("site-copilot"), backend, &q, "cosh").unwrap();
    assert_eq!(got, vec![target("cosh", "site-copilot")]);
}

#[test]
fn candidates_cli_override_uses_override_package_provides() {
    let q = FakeQuery {
        available_provides: vec![available_component_provider("cosh", "explicit-pkg")],
        ..Default::default()
    };
    let got = rpm_package_candidates(Some("explicit-pkg"), None, &q, "cosh").unwrap();
    assert_eq!(got, vec![target("cosh", "explicit-pkg")]);
}

#[test]
fn candidates_cli_override_without_component_identity_returns_empty() {
    let q = FakeQuery::default();
    let got = rpm_package_candidates(Some("explicit-pkg"), None, &q, "cosh").unwrap();
    assert!(got.is_empty());
}

#[test]
fn candidates_package_map_wins() {
    let repo = repo_with_rpm_map(&[("cosh", "site-copilot")]);
    let backend = repo.backends.get("rpm");
    let q = FakeQuery::default();
    let got = rpm_package_candidates(None, backend, &q, "cosh").unwrap();
    assert_eq!(got, vec![target("cosh", "site-copilot")]);
}

#[test]
fn candidates_provides_single_match() {
    let q = FakeQuery {
        provides: vec![(
            "anolisa-component(cosh)".to_string(),
            vec!["copilot-shell".to_string()],
        )],
        ..Default::default()
    };
    let got = rpm_package_candidates(None, None, &q, "cosh").unwrap();
    assert_eq!(got, vec![target("cosh", "copilot-shell")]);
}

#[test]
fn candidates_provides_multiple_is_ambiguous() {
    let q = FakeQuery {
        provides: vec![(
            "anolisa-component(cosh)".to_string(),
            vec!["pkg-a".to_string(), "pkg-b".to_string()],
        )],
        ..Default::default()
    };
    let got = rpm_package_candidates(None, None, &q, "cosh").unwrap();
    assert_eq!(got, vec![target("cosh", "pkg-a"), target("cosh", "pkg-b")]);
}

#[test]
fn candidates_package_name_uses_package_own_provides() {
    let q = FakeQuery {
        available_package_provides: vec![package_component_provide("copilot-shell", "cosh")],
        ..Default::default()
    };
    let got = rpm_package_candidates(None, None, &q, "copilot-shell").unwrap();
    assert_eq!(got, vec![target("cosh", "copilot-shell")]);
}

#[test]
fn candidates_plain_package_without_metadata_returns_empty() {
    let q = FakeQuery::default();
    let got = rpm_package_candidates(None, None, &q, "copilot-shell").unwrap();
    assert!(got.is_empty());
}

// ── I3: an unmanaged system RPM is never silently adopted ───────────

#[test]
fn install_over_unmanaged_system_rpm_points_at_adopt() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        ..Default::default()
    };
    let err = handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
        .expect_err("present unmanaged system RPM must refuse, not auto-adopt");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(
        err.reason().contains("adopt copilot-shell"),
        "must point at adopt: {}",
        err.reason()
    );
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "the refusal must not write any record"
    );
}

#[test]
fn install_dry_run_over_unmanaged_system_rpm_also_refuses() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(true);
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        ..Default::default()
    };
    let err = handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
        .expect_err("the plan refusal does not depend on dry-run");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("adopt"), "got: {}", err.reason());
}

// ── I8: install over a tracked (observed) record is idempotent ──────

#[test]
fn install_over_tracked_observed_record_is_a_noop() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    seed_tracked_rpm(&ctx, "copilot-shell", Ownership::RpmObserved);
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        ..Default::default()
    };
    let outcome =
        handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
            .expect("tracked + present is idempotent");
    assert_eq!(outcome, InstallOutcome::AlreadyInstalled);
}

// ── I6: install over a managed + present record refuses ─────────────

#[test]
fn install_of_present_rpm_managed_component_is_already_managed() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    seed_tracked_rpm(&ctx, "copilot-shell", Ownership::RpmManaged);
    // rpmdb still has the package, so the managed-record probe yields Present.
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"),
        )],
        ..Default::default()
    };
    let err = handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
        .expect_err("re-install of a managed component must refuse");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("update"), "got: {}", err.reason());

    let store = load_store(&ctx);
    let record = store
        .find(ObjectKind::Component, "copilot-shell")
        .expect("managed record preserved");
    assert!(matches!(
        record.binding,
        ProviderBinding::Delegated {
            relation: ManagementRelation::Managed { .. },
            ..
        }
    ));
}

// ── I2: fresh delegated install through the planner pipeline ────────

#[test]
fn delegated_install_writes_a_managed_record() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    )
    .with_origin("anolisa")
    .expect_lock_held(layout.lock_file.clone());
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let outcome = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect("delegated install ok");
    assert_eq!(outcome, InstallOutcome::Installed);
    assert_eq!(fake.install_calls.get(), 1, "dnf install must run once");
    assert!(
        fake.lock_was_held.get(),
        "install lock must remain held while dnf runs"
    );

    let store = load_store(&ctx);
    let record = store
        .find(ObjectKind::Component, "copilot-shell")
        .expect("component recorded");
    match &record.binding {
        ProviderBinding::Delegated {
            package,
            relation,
            last_observed,
            ..
        } => {
            assert_eq!(package.resolved_name(), Some("copilot-shell"));
            assert!(matches!(relation, ManagementRelation::Managed { .. }));
            let observed = last_observed.as_ref().expect("fresh observation");
            assert_eq!(observed.evr.as_deref(), Some("2.3.0-1.al8"));
            assert_eq!(observed.arch.as_deref(), Some("x86_64"));
        }
        other => panic!("expected a delegated binding, got {other:?}"),
    }
    assert_eq!(store.operations.len(), 1);
    assert!(store.operations[0].command.starts_with("install"));
    assert_eq!(
        record.last_operation_id,
        Some(store.operations[0].id.clone())
    );

    let journals = load_journals(&layout);
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0].status, TransactionOutcomeStatus::Ok);
}

#[test]
fn delegated_install_lock_failure_precedes_dnf() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    let _held = anolisa_core::lock::InstallLock::acquire(&layout.lock_file).expect("hold lock");
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect_err("held lock must fail before dnf");
    assert!(err.reason().contains("install lock"));
    assert_eq!(fake.install_calls.get(), 0, "dnf must not run before lock");
}

#[test]
fn delegated_install_rechecks_native_absence_under_lock() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    )
    .package_appears_under_lock(layout.lock_file.clone());
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect_err("an external RPM appearing before locked execution must block dnf");

    assert_eq!(fake.install_calls.get(), 0, "dnf must not run");
    assert!(err.reason().contains("appeared"), "got: {}", err.reason());
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "a refused race must not claim a managed record"
    );
}

#[test]
fn delegated_install_corrupt_state_fails_before_dnf() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    std::fs::write(layout.state_dir.join("installed.toml"), "not = [valid toml")
        .expect("write corrupt state");
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect_err("corrupt state must fail before dnf");
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(err.reason().contains("failed to load installed state"));
    assert_eq!(fake.install_calls.get(), 0, "dnf must not run");
}

// ── I7: a managed record whose package vanished points at repair ────

#[test]
fn managed_rpm_removed_externally_points_at_repair() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "copilot-shell", Ownership::RpmManaged);
    // rpmdb no longer has the package.
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );

    let err = install_component_with_deps(
        "copilot-shell",
        &args("copilot-shell"),
        &ctx,
        &fake,
        &fake,
        true,
    )
    .expect_err("externally removed managed package must not reinstall implicitly");
    assert!(err.reason().contains("repair"), "got: {}", err.reason());
    assert_eq!(fake.install_calls.get(), 0);
}

// ── I9: a tracked (observed) record whose package vanished ──────────

#[test]
fn observed_rpm_removed_externally_points_at_forget() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "copilot-shell", Ownership::RpmObserved);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );

    let err = install_component_with_deps(
        "copilot-shell",
        &args("copilot-shell"),
        &ctx,
        &fake,
        &fake,
        true,
    )
    .expect_err("observed package must not become managed implicitly");
    assert!(err.reason().contains("forget"), "got: {}", err.reason());
    assert_eq!(fake.install_calls.get(), 0);
}

#[test]
fn observed_rpm_alias_resolves_through_the_recorded_package() {
    // The record for 'cosh' tracks package 'copilot-shell'; addressing it by
    // component name must probe the recorded package, not re-derive one.
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "cosh", Ownership::RpmObserved);
    let query = FakeQuery::default();

    let err = handle_one_with_query("cosh".to_string(), args("cosh"), &ctx, &query)
        .expect_err("observed record with a missing package points at forget");
    assert!(err.reason().contains("forget"), "got: {}", err.reason());
    assert!(!err.reason().contains("not an ANOLISA RPM component"));
}

// ── I6/I11 with aliases and overrides ────────────────────────────────

#[test]
fn managed_component_alias_is_already_managed() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "cosh", Ownership::RpmManaged);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"),
    );
    // rpmdb still holds the recorded package.
    *fake.installed.borrow_mut() =
        Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"));

    let err = install_component_with_deps("cosh", &args("cosh"), &ctx, &fake, &fake, true)
        .expect_err("managed + present is not reinstalled through install");
    assert!(err.reason().contains("update"), "got: {}", err.reason());
    assert_eq!(fake.install_calls.get(), 0);
}

#[test]
fn package_override_conflicting_with_managed_record_is_rejected() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "cosh", Ownership::RpmManaged);
    let fake = FakeInstaller::new(
        "replacement-shell",
        pkg_info("replacement-shell", "9.9.9", Some("1.al8"), "x86_64"),
    );
    let mut install_args = args("cosh");
    install_args.package = Some("replacement-shell".to_string());

    let err = install_component_with_deps("cosh", &install_args, &ctx, &fake, &fake, true)
        .expect_err("managed package identity must not be repointed");
    assert!(err.reason().contains("conflicts"), "got: {}", err.reason());
    assert_eq!(fake.install_calls.get(), 0);
}

#[test]
fn managed_rpm_query_parse_error_is_not_reported_as_multi_version() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "copilot-shell", Ownership::RpmManaged);
    let query = FakeQuery {
        unexpected_installed: vec![(
            "copilot-shell".to_string(),
            "expected 5 fields, got 4".to_string(),
        )],
        ..Default::default()
    };

    let err = handle_one_with_query(
        "copilot-shell".to_string(),
        args("copilot-shell"),
        &ctx,
        &query,
    )
    .expect_err("malformed rpm output must remain a query error");
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(err.reason().contains("expected 5 fields, got 4"));
    assert!(!err.reason().contains("multiple installed versions"));
}

#[test]
fn delegated_install_non_root_is_refused() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, false)
        .expect_err("must refuse without root");
    assert!(
        err.reason().contains("root") || err.to_string().contains("sudo"),
        "reason must point at privileges: {err}"
    );
    assert_eq!(fake.install_calls.get(), 0, "dnf must not run without root");
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "refused install must not write state"
    );
}

#[test]
fn delegated_install_dry_run_previews_without_txn_or_state() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(true);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let outcome = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, false)
        .expect("dry-run ok");
    assert_eq!(outcome, InstallOutcome::Installed);
    assert_eq!(fake.install_calls.get(), 0, "dry-run must not run dnf");
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "dry-run must not persist state"
    );
}

#[test]
fn delegated_install_dnf_failure_is_forward_only_and_suggests_repair() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    )
    .failing_install();
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect_err("dnf failure must propagate");
    assert!(err.reason().contains("repair"), "got: {}", err.reason());
    assert_eq!(fake.install_calls.get(), 1);
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "failed install must not write state"
    );
}

#[test]
fn delegated_install_state_save_failure_surfaces_repair_guidance() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    )
    .failing_state_save(layout.state_dir.join("installed.toml"));
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect_err("state save failure after dnf must require repair");
    assert!(err.reason().contains("repair"), "got: {}", err.reason());
}

#[test]
fn pending_journal_blocks_install_before_dnf() {
    for dry_run in [false, true] {
        let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(dry_run);
        let layout = common::resolve_layout(&ctx);
        rpm_install::begin_fresh_install(&layout, "cosh", "copilot-shell", "install cosh")
            .expect("begin pending install");
        let fake = FakeInstaller::new(
            "copilot-shell",
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        );
        let mut install_args = args("cosh");
        install_args.backend = Some("rpm".to_string());
        install_args.package = Some("copilot-shell".to_string());

        let err = install_component_with_deps("cosh", &install_args, &ctx, &fake, &fake, true)
            .expect_err("a pending operation journal must block a new install");
        assert!(err.reason().contains("repair"), "got: {}", err.reason());
        assert_eq!(fake.install_calls.get(), 0);
    }
}

#[test]
fn pending_journal_injected_after_install_preflight_blocks_locked_execution() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    let state_path = layout.state_dir.join("installed.toml");
    anolisa_core::state_store::StateStore::empty()
        .save(&state_path)
        .expect("seed state");
    let state_before = std::fs::read(&state_path).expect("read state");
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    )
    .injecting_pending_journal(&layout, "copilot-shell");
    let mut install_args = args("copilot-shell");
    install_args.backend = Some("rpm".to_string());
    install_args.package = Some("copilot-shell".to_string());

    let err = install_component_with_deps("copilot-shell", &install_args, &ctx, &fake, &fake, true)
        .expect_err("the locked recovery gate must catch the injected journal");

    assert!(
        err.reason().contains("anolisa repair copilot-shell"),
        "got: {}",
        err.reason()
    );
    assert_eq!(fake.install_calls.get(), 0, "dnf must not run");
    assert_eq!(
        std::fs::read(&state_path).expect("read state"),
        state_before
    );
    let journals = load_journals(&layout);
    assert_eq!(journals.len(), 1, "no second journal may be created");
    assert!(journals[0].is_pending());
    assert_eq!(journals[0].subject.as_deref(), Some("copilot-shell"));
}

fn load_journals(layout: &FsLayout) -> Vec<Transaction> {
    let dir = layout.state_dir.join("journal");
    let mut paths = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| entry.expect("journal entry").path())
            .collect::<Vec<_>>(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => panic!("read journals: {err}"),
    };
    paths.sort();
    paths
        .into_iter()
        .map(|path| Transaction::load_journal(&path).expect("valid journal"))
        .collect()
}

#[test]
fn delegated_install_requires_configured_rpm_backend() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect_err("missing rpm backend config must block dnf install");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(
        err.reason().contains("backend 'rpm' is not configured"),
        "got: {}",
        err.reason()
    );
    assert_eq!(
        fake.install_calls.get(),
        0,
        "dnf must not run without a configured RPM source"
    );
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "refused install must not write state"
    );
}

#[test]
fn system_install_without_rpm_tooling_warns_and_exits() {
    // System scope, fresh state: with rpm/dnf absent the probe cannot prove
    // the component is not an unobserved system RPM (I3), so install refuses
    // rather than silently placing raw files over one.
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let q = FakeQuery {
        command_missing: true,
        ..Default::default()
    };
    let err = handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
        .expect_err("missing rpm/dnf must abort, not fall back to raw");
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(
        err.reason().contains("not found on PATH"),
        "got: {}",
        err.reason()
    );
    // No fallback raw install happened: state stays empty.
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "warn-and-exit must not write any state"
    );
}

#[test]
fn explicit_rpm_without_tooling_warns_and_exits() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let q = FakeQuery {
        command_missing: true,
        ..Default::default()
    };
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    let err = handle_one_with_query("copilot-shell".to_string(), a, &ctx, &q)
        .expect_err("missing rpm/dnf must abort");
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(
        err.reason().contains("not found on PATH"),
        "got: {}",
        err.reason()
    );
}

#[test]
fn explicit_rpm_with_ambiguous_candidates_is_invalid_argument() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let q = FakeQuery {
        provides: vec![(
            "anolisa-component(copilot-shell)".to_string(),
            vec!["pkg-a".to_string(), "pkg-b".to_string()],
        )],
        ..Default::default()
    };
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    let err = handle_one_with_query("copilot-shell".to_string(), a, &ctx, &q)
        .expect_err("ambiguous → refuse");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("pkg-a") && err.reason().contains("pkg-b"));
}

#[test]
fn explicit_rpm_not_an_anolisa_component_is_invalid_argument() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let q = FakeQuery::default();
    let mut a = args("random-package");
    a.backend = Some("rpm".to_string());
    let err = handle_one_with_query("random-package".to_string(), a, &ctx, &q)
        .expect_err("no component identity → refuse");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(
        err.reason().contains("not an ANOLISA RPM component"),
        "got: {}",
        err.reason()
    );
}

#[test]
fn explicit_rpm_on_raw_installed_component_is_rejected() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    // Component already installed via raw.
    let mut state = InstalledState {
        install_mode: StateInstallMode::System,
        prefix: common::resolve_layout(&ctx).prefix.clone(),
        ..Default::default()
    };
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: "copilot-shell".to_string(),
        version: "1.0.0".to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: Some("https://example.com/raw".to_string()),
        raw_package: None,
        install_backend: Some("raw".to_string()),
        ownership: Some(Ownership::RawManaged),
        rpm_metadata: None,
        installed_at: "2026-06-01T10:00:00Z".to_string(),
        last_operation_id: Some("op-prior".to_string()),
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
        .save(
            &common::resolve_layout(&ctx)
                .state_dir
                .join("installed.toml"),
        )
        .expect("seed state");

    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        ..Default::default()
    };
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    let err = handle_one_with_query("copilot-shell".to_string(), a, &ctx, &q)
        .expect_err("backend switch must be rejected");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("conflicts"), "got: {}", err.reason());
}

fn tracked_rpm_component(component: &str, ownership: Ownership) -> InstalledObject {
    let observed = ownership == Ownership::RpmObserved;
    let rpm = ownership != Ownership::RawManaged;
    InstalledObject {
        kind: ObjectKind::Component,
        name: component.to_string(),
        version: "2.2.0-1.al8".to_string(),
        status: if observed {
            ObjectStatus::Adopted
        } else {
            ObjectStatus::Installed
        },
        manifest_digest: Some("preserve-manifest-digest".to_string()),
        distribution_source: None,
        raw_package: None,
        install_backend: Some(if rpm { "rpm" } else { "raw" }.to_string()),
        ownership: Some(ownership),
        rpm_metadata: rpm.then(|| RpmMetadata {
            package_name: "copilot-shell".to_string(),
            evr: Some("2.2.0-1.al8".to_string()),
            arch: Some("x86_64".to_string()),
            source_repo: Some("old-repo".to_string()),
        }),
        installed_at: "2026-06-01T10:00:00Z".to_string(),
        last_operation_id: Some("op-prior".to_string()),
        managed: !observed,
        adopted: observed,
        subscription_scope: Default::default(),
        enabled_features: vec!["feature-a".to_string()],
        component_refs: vec!["legacy-ref".to_string()],
        files: Vec::new(),
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
        provisioned_packages: vec!["dependency-a".to_string()],
    }
}

fn seed_tracked_rpm(ctx: &CliContext, component: &str, ownership: Ownership) -> InstalledObject {
    let layout = common::resolve_layout(ctx);
    let object = tracked_rpm_component(component, ownership);
    let mut state = InstalledState {
        install_mode: StateInstallMode::System,
        prefix: layout.prefix.clone(),
        ..Default::default()
    };
    state.upsert_object(object.clone());
    state
        .save(&layout.state_dir.join("installed.toml"))
        .expect("seed tracked RPM state");
    object
}

// ---- Version-pinned RPM install (issue #1682) ----------------------------

/// Repo config with an rpm backend and a `package_map`, written to disk so the
/// full install pipeline (which loads repo.toml) sees the mapping. Used by the
/// `--package` + `--version` composition test.
fn system_ctx_with_rpm_package_map(pairs: &[(&str, &str)]) -> (tempfile::TempDir, CliContext) {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let prefix = tmp.path().to_path_buf();
    let layout = FsLayout::system(Some(prefix.clone()));
    std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
    std::fs::create_dir_all(&layout.state_dir).expect("state dir");
    let map = pairs
        .iter()
        .map(|(component, package)| format!("{component} = \"{package}\""))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        layout.etc_dir.join("repo.toml"),
        format!(
            r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "https://example.com/anolisa"

[backends.rpm]
base_url = "https://repo.example/anolisa"
gpgcheck = false

[backends.rpm.package_map]
{map}
"#
        ),
    )
    .expect("write repo.toml");
    let ctx = ctx_with_prefix(false, Some(prefix));
    (tmp, ctx)
}

#[test]
fn pinned_delegated_install_passes_exact_nevra_and_records_bare_package() {
    // The repo publishes 0.6.2 and a newer 0.7.0; pinning 0.6.2 must hand the
    // exact 0.6.2 NEVRA to dnf, while observation and the persisted record
    // keep the bare package identity.
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let arch = host_arch();
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "0.6.2", Some("1.al8"), &arch),
    )
    .with_origin("anolisa-configured")
    .with_available(vec![
        available_candidate(
            "copilot-shell",
            None,
            "0.6.2",
            "1.al8",
            &arch,
            "anolisa-configured",
        ),
        available_candidate(
            "copilot-shell",
            None,
            "0.7.0",
            "1.al8",
            &arch,
            "anolisa-configured",
        ),
    ])
    .expecting_install(&format!("copilot-shell-0.6.2-1.al8.{arch}"));
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    a.version = Some("0.6.2".to_string());

    let outcome = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect("pinned install ok");
    assert_eq!(outcome, InstallOutcome::Installed);
    assert_eq!(fake.install_calls.get(), 1, "dnf install must run once");
    assert_eq!(
        fake.install_specs.borrow().as_slice(),
        &[format!("copilot-shell-0.6.2-1.al8.{arch}")],
        "the exact pinned NEVRA must reach dnf, not the bare package"
    );

    let store = load_store(&ctx);
    let record = store
        .find(ObjectKind::Component, "copilot-shell")
        .expect("component recorded");
    match &record.binding {
        ProviderBinding::Delegated {
            package,
            last_observed,
            ..
        } => {
            // Identity and observation stay on the bare package — the NEVRA
            // never leaks into persisted state.
            assert_eq!(package.resolved_name(), Some("copilot-shell"));
            let observed = last_observed.as_ref().expect("fresh observation");
            assert_eq!(observed.evr.as_deref(), Some("0.6.2-1.al8"));
        }
        other => panic!("expected a delegated binding, got {other:?}"),
    }
}

#[test]
fn pinned_install_selects_highest_release_for_requested_version() {
    // One version, several releases: the highest EVR release is the one that
    // reaches dnf.
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let arch = host_arch();
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "0.6.2", Some("10.al8"), &arch),
    )
    .with_available(vec![
        available_candidate(
            "copilot-shell",
            None,
            "0.6.2",
            "1.al8",
            &arch,
            "anolisa-configured",
        ),
        available_candidate(
            "copilot-shell",
            None,
            "0.6.2",
            "10.al8",
            &arch,
            "anolisa-configured",
        ),
        available_candidate(
            "copilot-shell",
            None,
            "0.6.2",
            "2.al8",
            &arch,
            "anolisa-configured",
        ),
    ])
    .expecting_install(&format!("copilot-shell-0.6.2-10.al8.{arch}"));
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    a.version = Some("0.6.2".to_string());

    install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect("pinned install ok");
    assert_eq!(
        fake.install_specs.borrow().as_slice(),
        &[format!("copilot-shell-0.6.2-10.al8.{arch}")]
    );
}

#[test]
fn pinned_install_missing_version_fails_before_txn_journal_and_state() {
    // The requested version is absent (repo only has 0.7.0): the install must
    // fail before dnf runs, before a journal is written, and before any state
    // record — and never retry without the version constraint.
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    let arch = host_arch();
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "0.7.0", Some("1.al8"), &arch),
    )
    .with_available(vec![available_candidate(
        "copilot-shell",
        None,
        "0.7.0",
        "1.al8",
        &arch,
        "anolisa-configured",
    )]);
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    a.version = Some("0.6.2".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect_err("a missing pinned version must fail");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("0.6.2"), "got: {}", err.reason());
    assert_eq!(fake.install_calls.get(), 0, "dnf must not run");
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "a missing version must not claim a record"
    );
    assert!(
        load_journals(&layout).is_empty(),
        "a missing version must not open a journal"
    );
}

#[test]
fn pinned_install_wrong_arch_fails_before_mutation() {
    // The version exists, but only for an architecture this host cannot run.
    // The refusal names the version and the host arch and changes nothing.
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let arch = host_arch();
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "0.6.2", Some("1.al8"), &arch),
    )
    .with_available(vec![available_candidate(
        "copilot-shell",
        None,
        "0.6.2",
        "1.al8",
        "s390x",
        "anolisa-configured",
    )]);
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    a.version = Some("0.6.2".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect_err("a host-incompatible version must fail");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(
        err.reason().contains("architecture") && err.reason().contains(&arch),
        "message must name the host architecture: {}",
        err.reason()
    );
    assert_eq!(fake.install_calls.get(), 0, "dnf must not run");
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none()
    );
}

#[test]
fn pinned_delegated_dry_run_shows_resolved_candidate_without_txn_or_state() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(true);
    let arch = host_arch();
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "0.6.2", Some("1.al8"), &arch),
    )
    .with_available(vec![
        available_candidate(
            "copilot-shell",
            None,
            "0.6.2",
            "1.al8",
            &arch,
            "anolisa-configured",
        ),
        available_candidate(
            "copilot-shell",
            None,
            "0.7.0",
            "1.al8",
            &arch,
            "anolisa-configured",
        ),
    ]);
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    a.version = Some("0.6.2".to_string());

    let outcome = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, false)
        .expect("pinned dry-run ok");
    assert_eq!(outcome, InstallOutcome::Installed);
    assert_eq!(fake.install_calls.get(), 0, "dry-run must not run dnf");
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "dry-run must not persist state"
    );
}

#[test]
fn pinned_dry_run_still_validates_against_the_repository() {
    // Dry-run resolves against real repository candidates, so an absent
    // version fails in dry-run just as it would for a real install — it does
    // not blindly echo the request.
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(true);
    let arch = host_arch();
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "0.7.0", Some("1.al8"), &arch),
    )
    .with_available(vec![available_candidate(
        "copilot-shell",
        None,
        "0.7.0",
        "1.al8",
        &arch,
        "anolisa-configured",
    )]);
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    a.version = Some("0.6.2".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, false)
        .expect_err("dry-run must validate the pinned version");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("0.6.2"), "got: {}", err.reason());
}

#[test]
fn pinned_install_handles_a_non_zero_epoch() {
    // A non-zero epoch on the winning candidate renders into the NEVRA in the
    // `name-epoch:version-release.arch` form dnf accepts.
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let arch = host_arch();
    let mut installs_to = pkg_info("copilot-shell", "0.6.2", Some("1.al8"), &arch);
    installs_to.version.epoch = Some("2".to_string());
    let fake = FakeInstaller::new("copilot-shell", installs_to)
        .with_available(vec![available_candidate(
            "copilot-shell",
            Some("2"),
            "0.6.2",
            "1.al8",
            &arch,
            "anolisa-configured",
        )])
        .expecting_install(&format!("copilot-shell-2:0.6.2-1.al8.{arch}"));
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    a.version = Some("0.6.2".to_string());

    install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect("pinned install ok");
    assert_eq!(
        fake.install_specs.borrow().as_slice(),
        &[format!("copilot-shell-2:0.6.2-1.al8.{arch}")]
    );
}

#[test]
fn pinned_install_composes_with_package_override() {
    // `--package` selects the backend package; `--version` then pins against
    // that package's candidates. The override NEVRA reaches dnf while the
    // component name stays the addressed one.
    let (_tmp, ctx) = system_ctx_with_rpm_package_map(&[("cosh", "site-copilot")]);
    let arch = host_arch();
    let fake = FakeInstaller::new(
        "site-copilot",
        pkg_info("site-copilot", "0.6.2", Some("1.al8"), &arch),
    )
    .with_available(vec![available_candidate(
        "site-copilot",
        None,
        "0.6.2",
        "1.al8",
        &arch,
        "anolisa-configured",
    )])
    .expecting_install(&format!("site-copilot-0.6.2-1.al8.{arch}"));
    let mut a = args("cosh");
    a.backend = Some("rpm".to_string());
    a.package = Some("site-copilot".to_string());
    a.version = Some("0.6.2".to_string());

    install_component_with_deps("cosh", &a, &ctx, &fake, &fake, true).expect("pinned install ok");
    assert_eq!(
        fake.install_specs.borrow().as_slice(),
        &[format!("site-copilot-0.6.2-1.al8.{arch}")]
    );
    let store = load_store(&ctx);
    let record = store
        .find(ObjectKind::Component, "cosh")
        .expect("component recorded under the addressed name");
    match &record.binding {
        ProviderBinding::Delegated { package, .. } => {
            assert_eq!(package.resolved_name(), Some("site-copilot"));
        }
        other => panic!("expected a delegated binding, got {other:?}"),
    }
}

#[test]
fn unpinned_delegated_install_passes_bare_package() {
    // Regression guard: without `--version` the native transaction still
    // receives the bare package name (repository default), unchanged by the
    // version-pinning work.
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let arch = host_arch();
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), &arch),
    );
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect("unpinned install ok");
    assert_eq!(
        fake.install_specs.borrow().as_slice(),
        &["copilot-shell".to_string()],
        "an unpinned install must send the bare package name"
    );
}

#[test]
fn pinned_install_refuses_state_when_dnf_installs_a_different_evr() {
    // End-to-end guard: the pin resolves 0.6.2, but dnf actually lands 0.7.0
    // (module stream / Obsoletes). The command must fail toward repair and
    // must not persist a record for the wrong version.
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let arch = host_arch();
    // Candidate resolves to 0.6.2, but the rpmdb reports 0.7.0 after install.
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "0.7.0", Some("1.al8"), &arch),
    )
    .with_available(vec![available_candidate(
        "copilot-shell",
        None,
        "0.6.2",
        "1.al8",
        &arch,
        "anolisa-configured",
    )])
    .expecting_install(&format!("copilot-shell-0.6.2-1.al8.{arch}"));
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    a.version = Some("0.6.2".to_string());

    let err = install_component_with_deps("copilot-shell", &a, &ctx, &fake, &fake, true)
        .expect_err("a wrong installed EVR must fail");
    assert!(err.reason().contains("repair"), "got: {}", err.reason());
    assert!(
        load_store(&ctx)
            .find(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "the wrong version must not be recorded"
    );
}
