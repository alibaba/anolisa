//! rpm tests for the  command.

use super::super::tests::*;

use anolisa_core::state::{
    InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
    Ownership, RpmMetadata,
};
use anolisa_platform::fs_layout::FsLayout;

use crate::commands::common;
use crate::context::InstallMode;
use crate::repo_config::RepoConfig;
use crate::resolution::ResolutionUse;
use tempfile::tempdir;

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

#[test]
fn rpm_component_capability_accepts_versioned_provides() {
    assert!(rpm_capability_matches_component(
        "anolisa-component(cosh) = 1.0.0",
        "anolisa-component(cosh)"
    ));
    assert!(!rpm_capability_matches_component(
        "anolisa-component(cosh-extra) = 1.0.0",
        "anolisa-component(cosh)"
    ));
}

#[test]
fn probe_reports_adoptable_for_installed_default_name() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let repo = RepoConfig::load(&common::resolve_layout(&ctx), false)
        .expect("repo")
        .config;
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        package_provides: vec![package_component_provide("copilot-shell", "copilot-shell")],
        ..Default::default()
    };
    let situation = probe_rpm_situation(
        "copilot-shell",
        None,
        repo.backends.get("rpm"),
        None,
        ResolutionUse::Install,
        &q,
        "install",
    )
    .expect("probe");
    match situation {
        RpmSituation::Adoptable { target, info } => {
            assert_eq!(target.package, "copilot-shell");
            assert_eq!(info.version.to_string(), "2.3.0-1.al8");
        }
        other => panic!(
            "expected Adoptable, got {other:?}",
            other = situation_label(&other)
        ),
    }
}

#[test]
fn probe_reports_absent_when_not_installed() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let repo = RepoConfig::load(&common::resolve_layout(&ctx), false)
        .expect("repo")
        .config;
    let q = FakeQuery {
        available_provides: vec![available_component_provider(
            "copilot-shell",
            "copilot-shell",
        )],
        ..Default::default()
    };
    let situation = probe_rpm_situation(
        "copilot-shell",
        None,
        repo.backends.get("rpm"),
        None,
        ResolutionUse::Install,
        &q,
        "install",
    )
    .expect("probe");
    assert!(matches!(situation, RpmSituation::Absent { .. }));
}

#[test]
fn probe_reports_ambiguous_for_multiple_providers() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let repo = RepoConfig::load(&common::resolve_layout(&ctx), false)
        .expect("repo")
        .config;
    let q = FakeQuery {
        provides: vec![(
            "anolisa-component(copilot-shell)".to_string(),
            vec!["pkg-a".to_string(), "pkg-b".to_string()],
        )],
        ..Default::default()
    };
    let situation = probe_rpm_situation(
        "copilot-shell",
        None,
        repo.backends.get("rpm"),
        None,
        ResolutionUse::Install,
        &q,
        "install",
    )
    .expect("probe");
    assert!(matches!(situation, RpmSituation::Ambiguous(_)));
}

#[test]
fn probe_reports_multi_version_drift() {
    let (_tmp, _ctx) = system_ctx_with_raw_repo(false);
    let repo = repo_with_rpm_map(&[("copilot-shell", "copilot-shell")]);
    let q = FakeQuery {
        multi_version: vec!["copilot-shell".to_string()],
        ..Default::default()
    };
    let situation = probe_rpm_situation(
        "copilot-shell",
        None,
        repo.backends.get("rpm"),
        None,
        ResolutionUse::Install,
        &q,
        "install",
    )
    .expect("probe");
    assert!(matches!(situation, RpmSituation::MultiVersion(_)));
}

#[test]
fn adopt_writes_rpm_observed_state() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        origins: vec![("copilot-shell".to_string(), "@System".to_string())],
        ..Default::default()
    };
    let outcome =
        handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
            .expect("adopt ok");
    assert_eq!(outcome, InstallOutcome::Adopted);

    let state = load_state(&ctx);
    let obj = state
        .find_object(ObjectKind::Component, "copilot-shell")
        .expect("component recorded");
    assert_eq!(obj.status, ObjectStatus::Adopted);
    assert_eq!(obj.ownership, Some(Ownership::RpmObserved));
    assert_eq!(obj.install_backend.as_deref(), Some("rpm"));
    assert!(!obj.managed, "rpm-observed must not be ANOLISA-managed");
    assert!(obj.adopted);
    assert!(obj.files.is_empty(), "RPM-owned files stay out of state");
    assert_eq!(obj.version, "2.3.0-1.al8");
    let meta = obj.rpm_metadata.as_ref().expect("rpm metadata");
    assert_eq!(meta.package_name, "copilot-shell");
    assert_eq!(meta.evr.as_deref(), Some("2.3.0-1.al8"));
    assert_eq!(meta.arch.as_deref(), Some("x86_64"));
    assert_eq!(meta.source_repo.as_deref(), Some("@System"));
}

#[test]
fn adopt_dry_run_does_not_write_state() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(true);
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        ..Default::default()
    };
    let outcome =
        handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
            .expect("adopt plan ok");
    assert_eq!(outcome, InstallOutcome::Adopted);
    let state = load_state(&ctx);
    assert!(
        state
            .find_object(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "dry-run must not persist adopt state"
    );
}

#[test]
fn adopt_refresh_overwrites_evr() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    // Pre-seed an older rpm-observed record.
    let mut state = InstalledState {
        install_mode: StateInstallMode::System,
        prefix: common::resolve_layout(&ctx).prefix.clone(),
        ..Default::default()
    };
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: "copilot-shell".to_string(),
        version: "2.2.0-1.al8".to_string(),
        status: ObjectStatus::Adopted,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("rpm".to_string()),
        ownership: Some(Ownership::RpmObserved),
        rpm_metadata: Some(RpmMetadata {
            package_name: "copilot-shell".to_string(),
            evr: Some("2.2.0-1.al8".to_string()),
            arch: Some("x86_64".to_string()),
            source_repo: Some("@System".to_string()),
        }),
        installed_at: "2026-06-01T10:00:00Z".to_string(),
        last_operation_id: Some("op-prior".to_string()),
        managed: false,
        adopted: true,
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

    // rpmdb now reports a newer EVR.
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        origins: vec![("copilot-shell".to_string(), "@System".to_string())],
        ..Default::default()
    };
    // No --backend: existing rpm-observed state must route to adopt-refresh,
    // not be blocked by the raw trunk.
    let outcome =
        handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
            .expect("refresh ok");
    assert_eq!(outcome, InstallOutcome::Adopted);
    let state = load_state(&ctx);
    let obj = state
        .find_object(ObjectKind::Component, "copilot-shell")
        .expect("still recorded");
    assert_eq!(obj.version, "2.3.0-1.al8");
    assert_eq!(
        obj.rpm_metadata.as_ref().and_then(|m| m.evr.as_deref()),
        Some("2.3.0-1.al8")
    );
}

#[test]
fn adopt_refuses_to_clobber_concurrent_raw_install() {
    // Post-lock TOCTOU guard: layer 1 may decide "adopt" from a pre-lock
    // read where the component is absent, but a concurrent raw install can
    // win the lock and record it first. After reloading state under the
    // lock, adopt must re-check backend compatibility and refuse rather
    // than overwrite the raw provenance with rpm-observed. Calling
    // `execute_adopt` directly reproduces the "state changed under the lock"
    // window that layer 1's routing would otherwise hide.
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let layout = common::resolve_layout(&ctx);
    let mut state = InstalledState {
        install_mode: StateInstallMode::System,
        prefix: layout.prefix.clone(),
        ..Default::default()
    };
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: "copilot-shell".to_string(),
        version: "1.0.0".to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("raw".to_string()),
        ownership: Some(Ownership::RawManaged),
        rpm_metadata: None,
        installed_at: "2026-06-01T10:00:00Z".to_string(),
        last_operation_id: Some("op-raw".to_string()),
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
        .save(&layout.state_dir.join("installed.toml"))
        .expect("seed raw record");

    let q = FakeQuery::default();
    let err = execute_adopt(
        &ctx,
        &layout,
        "install copilot-shell",
        "copilot-shell",
        "copilot-shell".to_string(),
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        &q,
    )
    .expect_err("must refuse to clobber a concurrent raw install");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("raw"), "got: {}", err.reason());

    // The raw record survives untouched: nothing was overwritten.
    let state = load_state(&ctx);
    let obj = state
        .find_object(ObjectKind::Component, "copilot-shell")
        .expect("raw record preserved");
    assert_eq!(installed_backend_label(obj), Some("raw"));
    assert!(obj.rpm_metadata.is_none(), "raw record must stay raw");
}

#[test]
fn installed_backend_label_migrates_legacy_yum_to_rpm() {
    let obj = InstalledObject {
        kind: ObjectKind::Component,
        name: "copilot-shell".to_string(),
        version: "2.3.0".to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("yum".to_string()),
        ownership: None,
        rpm_metadata: None,
        installed_at: "2026-06-01T10:00:00Z".to_string(),
        last_operation_id: Some("op-legacy-yum".to_string()),
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
    };

    assert_eq!(installed_backend_label(&obj), Some("rpm"));
}

#[test]
fn adopt_refuses_to_downgrade_concurrent_rpm_managed_install() {
    // rpm-managed and rpm-observed share the "rpm" backend label, so
    // ensure_component_backend_compatible alone cannot tell them apart. A
    // concurrent delegated `dnf install` can record the component rpm-managed
    // (owns_removal=true) after a pre-lock read saw it absent. After
    // reloading under the lock, execute_adopt must refuse rather than
    // overwrite the managed record with rpm-observed (which would silently
    // drop ANOLISA's removal authority).
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let layout = common::resolve_layout(&ctx);
    let mut state = InstalledState {
        install_mode: StateInstallMode::System,
        prefix: layout.prefix.clone(),
        ..Default::default()
    };
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: "copilot-shell".to_string(),
        version: "2.3.0-1.al8".to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("rpm".to_string()),
        ownership: Some(Ownership::RpmManaged),
        rpm_metadata: Some(RpmMetadata {
            package_name: "copilot-shell".to_string(),
            evr: Some("2.3.0-1.al8".to_string()),
            arch: Some("x86_64".to_string()),
            source_repo: Some("alinux-updates".to_string()),
        }),
        installed_at: "2026-06-01T10:00:00Z".to_string(),
        last_operation_id: Some("op-install-prior".to_string()),
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
        .save(&layout.state_dir.join("installed.toml"))
        .expect("seed rpm-managed record");

    let q = FakeQuery::default();
    let err = execute_adopt(
        &ctx,
        &layout,
        "adopt copilot-shell",
        "copilot-shell",
        "copilot-shell".to_string(),
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        &q,
    )
    .expect_err("must refuse to downgrade an rpm-managed component");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("repair"), "got: {}", err.reason());

    // The managed record survives untouched: removal authority is preserved.
    let state = load_state(&ctx);
    let obj = state
        .find_object(ObjectKind::Component, "copilot-shell")
        .expect("managed record preserved");
    assert_eq!(obj.ownership, Some(Ownership::RpmManaged));
    assert!(obj.managed, "managed flag must stay true");
}

#[test]
fn install_of_present_rpm_managed_component_refuses_without_dnf() {
    // The full entrypoint must classify this as Present and stop before the
    // NoTxn transaction double, which panics if dnf is invoked.
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let layout = common::resolve_layout(&ctx);
    let mut state = InstalledState {
        install_mode: StateInstallMode::System,
        prefix: layout.prefix.clone(),
        ..Default::default()
    };
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: "copilot-shell".to_string(),
        version: "2.3.0-1.al8".to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: None,
        raw_package: None,
        install_backend: Some("rpm".to_string()),
        ownership: Some(Ownership::RpmManaged),
        rpm_metadata: Some(RpmMetadata {
            package_name: "copilot-shell".to_string(),
            evr: Some("2.3.0-1.al8".to_string()),
            arch: Some("x86_64".to_string()),
            source_repo: Some("alinux-updates".to_string()),
        }),
        installed_at: "2026-06-01T10:00:00Z".to_string(),
        last_operation_id: Some("op-install-prior".to_string()),
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
        .save(&layout.state_dir.join("installed.toml"))
        .expect("seed rpm-managed record");

    // rpmdb still has the package, so the managed-state probe yields Present.
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        ..Default::default()
    };
    let err = handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
        .expect_err("re-install of rpm-managed must refuse");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("status"), "got: {}", err.reason());
    assert!(err.reason().contains("repair"), "got: {}", err.reason());

    let obj = load_state(&ctx)
        .find_object(ObjectKind::Component, "copilot-shell")
        .cloned()
        .expect("managed record preserved");
    assert_eq!(obj.ownership, Some(Ownership::RpmManaged));
}

#[test]
fn adopt_envelope_verb_is_the_bare_command() {
    // The success JSON envelope reports the bare verb, so an explicit adopt
    // is not mislabelled "install" (the shared execute_adopt's module COMMAND).
    assert_eq!(adopt_envelope_verb("adopt copilot-shell"), "adopt");
    assert_eq!(adopt_envelope_verb("install copilot-shell"), "install");
    assert_eq!(adopt_envelope_verb(""), COMMAND);
}

#[test]
fn adopt_origin_failure_degrades_to_none() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let q = FakeQuery {
        installed: vec![(
            "copilot-shell".to_string(),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        )],
        origin_fails: true,
        ..Default::default()
    };
    let outcome =
        handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
            .expect("adopt still succeeds");
    assert_eq!(outcome, InstallOutcome::Adopted);
    let state = load_state(&ctx);
    let obj = state
        .find_object(ObjectKind::Component, "copilot-shell")
        .expect("recorded");
    assert_eq!(
        obj.rpm_metadata
            .as_ref()
            .and_then(|m| m.source_repo.as_deref()),
        None,
        "origin lookup failure must degrade source_repo to None, not fail the adopt"
    );
}

#[test]
fn delegated_install_writes_rpm_managed_state() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    )
    .with_origin("anolisa")
    .expect_lock_held(layout.lock_file);
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let outcome = handle_one_with_exec("copilot-shell".to_string(), a, &ctx, &exec)
        .expect("delegated install ok");
    assert_eq!(outcome, InstallOutcome::Installed);
    assert_eq!(fake.install_calls.get(), 1, "dnf install must run once");
    assert!(
        fake.lock_was_held.get(),
        "install lock must remain held while dnf runs"
    );

    let state = load_state(&ctx);
    let obj = state
        .find_object(ObjectKind::Component, "copilot-shell")
        .expect("component recorded");
    assert_eq!(obj.status, ObjectStatus::Installed);
    assert_eq!(obj.ownership, Some(Ownership::RpmManaged));
    assert_eq!(obj.install_backend.as_deref(), Some("rpm"));
    assert!(obj.managed, "rpm-managed must be ANOLISA-managed");
    assert!(!obj.adopted, "delegated install is not an adoption");
    assert!(obj.files.is_empty(), "dnf-owned files stay out of state");
    assert_eq!(obj.version, "2.3.0-1.al8");
    let meta = obj.rpm_metadata.as_ref().expect("rpm metadata");
    assert_eq!(meta.package_name, "copilot-shell");
    assert_eq!(meta.evr.as_deref(), Some("2.3.0-1.al8"));
    assert_eq!(meta.arch.as_deref(), Some("x86_64"));
    assert_eq!(meta.source_repo.as_deref(), Some("anolisa"));
    assert!(state.operations[0].id.starts_with("op-install-"));
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

#[test]
fn delegated_install_lock_failure_precedes_dnf() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    let _held = anolisa_core::lock::InstallLock::acquire(&layout.lock_file).expect("hold lock");
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = handle_one_with_exec("copilot-shell".to_string(), a, &ctx, &exec)
        .expect_err("held lock must fail before dnf");
    assert!(err.reason().contains("install lock"));
    assert_eq!(fake.install_calls.get(), 0, "dnf must not run before lock");
}

#[test]
fn delegated_install_corrupt_locked_state_is_runtime_error_before_dnf() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    std::fs::write(layout.state_dir.join("installed.toml"), "not = [valid toml")
        .expect("write corrupt state");
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };
    let expectation =
        DelegatedInstallExpectation::capture(&InstalledState::default(), "copilot-shell");

    let err = execute_delegated_install(
        &exec,
        &ctx,
        &layout,
        "install copilot-shell",
        "copilot-shell",
        expectation,
    )
    .expect_err("corrupt state must fail before dnf");
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(err.reason().contains("failed to load installed state"));
    assert_eq!(fake.install_calls.get(), 0, "dnf must not run");
}

#[test]
fn delegated_install_rechecks_state_under_lock() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let layout = common::resolve_layout(&ctx);
    seed_tracked_rpm(&ctx, "copilot-shell", Ownership::RawManaged);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };
    let expectation =
        DelegatedInstallExpectation::capture(&InstalledState::default(), "copilot-shell");

    let err = execute_delegated_install(
        &exec,
        &ctx,
        &layout,
        "install copilot-shell",
        "copilot-shell",
        expectation,
    )
    .expect_err("state added after routing must block dnf");
    assert!(err.reason().contains("changed"));
    assert_eq!(fake.install_calls.get(), 0);
}

#[test]
fn delegated_install_reinstalls_managed_rpm_without_erasing_metadata() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let before = seed_tracked_rpm(&ctx, "copilot-shell", Ownership::RpmManaged);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "aarch64"),
    );
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };

    let outcome = handle_one_with_exec(
        "copilot-shell".to_string(),
        args("copilot-shell"),
        &ctx,
        &exec,
    )
    .expect("tracked managed RPM may be reinstalled");
    assert_eq!(outcome, InstallOutcome::Installed);
    assert_eq!(fake.install_calls.get(), 1);

    let state = load_state(&ctx);
    let after = state
        .find_object(ObjectKind::Component, "copilot-shell")
        .expect("tracked object remains");
    assert_eq!(after.version, "2.3.0-1.al8");
    assert_eq!(after.status, ObjectStatus::Installed);
    assert_eq!(after.ownership, Some(Ownership::RpmManaged));
    assert_eq!(after.installed_at, before.installed_at);
    assert_eq!(after.manifest_digest, before.manifest_digest);
    assert_eq!(after.enabled_features, before.enabled_features);
    assert_eq!(after.component_refs, before.component_refs);
    assert_eq!(after.provisioned_packages, before.provisioned_packages);
    let metadata = after.rpm_metadata.as_ref().expect("rpm metadata");
    assert_eq!(metadata.arch.as_deref(), Some("aarch64"));
    assert_eq!(metadata.source_repo.as_deref(), Some("old-repo"));
    assert_ne!(after.last_operation_id, before.last_operation_id);
}

#[test]
fn delegated_install_refuses_missing_observed_rpm() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "copilot-shell", Ownership::RpmObserved);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };

    let err = handle_one_with_exec(
        "copilot-shell".to_string(),
        args("copilot-shell"),
        &ctx,
        &exec,
    )
    .expect_err("observed package must not become managed implicitly");
    assert!(err.reason().contains("rpm-observed"));
    assert!(err.reason().contains("forget copilot-shell"));
    assert_eq!(fake.install_calls.get(), 0);
}

#[test]
fn delegated_install_refuses_missing_observed_rpm_without_current_resolution() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "cosh", Ownership::RpmObserved);
    let query = FakeQuery::default();

    let err = handle_one_with_query("cosh".to_string(), args("cosh"), &ctx, &query)
        .expect_err("observed state must provide a deterministic forget path");

    assert!(err.reason().contains("rpm-observed"));
    assert!(err.reason().contains("copilot-shell"));
    assert!(err.reason().contains("forget cosh"));
    assert!(!err.reason().contains("not an ANOLISA RPM component"));
}

#[test]
fn delegated_install_uses_recorded_package_for_managed_component_alias() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "cosh", Ownership::RpmManaged);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };

    let outcome = handle_one_with_exec("cosh".to_string(), args("cosh"), &ctx, &exec)
        .expect("state package identity must allow alias reinstall");
    assert_eq!(outcome, InstallOutcome::Installed);
    assert_eq!(fake.install_calls.get(), 1);

    let state = load_state(&ctx);
    let object = state
        .find_object(ObjectKind::Component, "cosh")
        .expect("managed alias remains canonical");
    assert_eq!(object.version, "2.3.0-1.al8");
    assert_eq!(
        object
            .rpm_metadata
            .as_ref()
            .map(|metadata| metadata.package_name.as_str()),
        Some("copilot-shell")
    );
}

#[test]
fn delegated_install_normalizes_recorded_managed_package() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "cosh", Ownership::RpmManaged);
    let layout = common::resolve_layout(&ctx);
    let mut state = load_state(&ctx);
    state
        .find_object_mut(ObjectKind::Component, "cosh")
        .and_then(|object| object.rpm_metadata.as_mut())
        .expect("rpm metadata")
        .package_name = " copilot-shell ".to_string();
    state
        .save(&layout.state_dir.join("installed.toml"))
        .expect("save normalized-state fixture");

    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };

    let outcome = handle_one_with_exec("cosh".to_string(), args("cosh"), &ctx, &exec)
        .expect("persisted package whitespace must be normalized consistently");
    assert_eq!(outcome, InstallOutcome::Installed);
    assert_eq!(fake.install_calls.get(), 1);
    assert_eq!(
        load_state(&ctx)
            .find_object(ObjectKind::Component, "cosh")
            .and_then(|object| object.rpm_metadata.as_ref())
            .map(|metadata| metadata.package_name.as_str()),
        Some("copilot-shell")
    );
}

#[test]
fn delegated_install_rejects_package_override_different_from_managed_state() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    seed_tracked_rpm(&ctx, "cosh", Ownership::RpmManaged);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };
    let mut install_args = args("cosh");
    install_args.package = Some("replacement-shell".to_string());

    let err = handle_one_with_exec("cosh".to_string(), install_args, &ctx, &exec)
        .expect_err("managed package identity must not be repointed");
    assert!(err.reason().contains("copilot-shell"));
    assert!(err.reason().contains("replacement-shell"));
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
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: false,
    };
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = handle_one_with_exec("copilot-shell".to_string(), a, &ctx, &exec)
        .expect_err("must refuse without root");
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(
        err.reason().contains("root") && err.reason().contains("sudo"),
        "reason must point at sudo: {}",
        err.reason()
    );
    assert_eq!(fake.install_calls.get(), 0, "dnf must not run without root");
    assert!(
        load_state(&ctx)
            .find_object(ObjectKind::Component, "copilot-shell")
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
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: false,
    };
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let outcome =
        handle_one_with_exec("copilot-shell".to_string(), a, &ctx, &exec).expect("dry-run ok");
    assert_eq!(outcome, InstallOutcome::Installed);
    assert_eq!(fake.install_calls.get(), 0, "dry-run must not run dnf");
    assert!(
        load_state(&ctx)
            .find_object(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "dry-run must not persist state"
    );
}

#[test]
fn delegated_install_dnf_failure_surfaces() {
    let (_tmp, ctx) = system_ctx_with_configured_rpm_repo(false);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    )
    .failing_install();
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = handle_one_with_exec("copilot-shell".to_string(), a, &ctx, &exec)
        .expect_err("dnf failure must propagate");
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(
        err.reason().contains("dnf install failed"),
        "got: {}",
        err.reason()
    );
    assert_eq!(fake.install_calls.get(), 1);
    assert!(
        load_state(&ctx)
            .find_object(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "failed install must not write state"
    );
}

#[test]
fn delegated_install_requires_configured_rpm_backend() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let fake = FakeInstaller::new(
        "copilot-shell",
        pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
    );
    let exec = RpmExec {
        query: &fake,
        txn: &fake,
        is_root: true,
    };
    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());

    let err = handle_one_with_exec("copilot-shell".to_string(), a, &ctx, &exec)
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
        load_state(&ctx)
            .find_object(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "refused install must not write state"
    );
}

#[test]
fn system_install_without_rpm_tooling_warns_and_exits() {
    // Auto-detect path (system mode, no --backend, fresh state): with rpm/dnf
    // absent the probe cannot prove the component is not an unobserved system
    // RPM, so install refuses rather than silently falling back to raw (§7.1).
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let q = FakeQuery {
        command_missing: true,
        ..Default::default()
    };
    let err = handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
        .expect_err("missing rpm/dnf must abort, not fall back to raw");
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(
        err.reason().contains("rpm/dnf not found"),
        "got: {}",
        err.reason()
    );
    // No fallback raw install happened: state stays empty.
    let state = load_state(&ctx);
    assert!(
        state
            .find_object(ObjectKind::Component, "copilot-shell")
            .is_none(),
        "warn-and-exit must not write any state"
    );
}

#[test]
fn explicit_rpm_without_tooling_warns_and_exits() {
    // Explicit `--backend rpm` cannot adopt without rpmdb either; missing
    // tooling is a warn-and-exit, not the #959 "dnf install" hint.
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
        err.reason().contains("rpm/dnf not found"),
        "got: {}",
        err.reason()
    );
}

#[test]
fn adopt_ambiguous_is_invalid_argument() {
    let (_tmp, ctx) = system_ctx_with_raw_repo(false);
    let q = FakeQuery {
        provides: vec![(
            "anolisa-component(copilot-shell)".to_string(),
            vec!["pkg-a".to_string(), "pkg-b".to_string()],
        )],
        ..Default::default()
    };
    let err = handle_one_with_query("copilot-shell".to_string(), args("copilot-shell"), &ctx, &q)
        .expect_err("ambiguous → refuse");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("pkg-a") && err.reason().contains("pkg-b"));
}

#[test]
fn explicit_rpm_in_user_mode_is_rejected() {
    // route_rpm_adopt rejects user scope before touching rpmdb; call it
    // directly so the test needs no $HOME isolation.
    let tmp = tempdir().expect("tmpdir");
    let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
    let repo =
        RepoConfig::from_toml_str("schema_version = 1\ndefault_backend = \"raw\"\n[backends.raw]\nbase_url = \"https://e/x\"\n")
            .expect("repo");
    let installed = InstalledState::default();
    let q = FakeQuery::default();
    let mut user_ctx = ctx_with_prefix(false, Some(tmp.path().to_path_buf()));
    user_ctx.install_mode = InstallMode::User;

    let mut a = args("copilot-shell");
    a.backend = Some("rpm".to_string());
    let txn = NoTxn;
    let exec = RpmExec {
        query: &q,
        txn: &txn,
        is_root: false,
    };
    let err = route_rpm_adopt(
        "copilot-shell",
        &a,
        &user_ctx,
        "install copilot-shell",
        &layout,
        &repo,
        &installed,
        BackendSource::Explicit,
        None,
        None,
        &exec,
    )
    .expect_err("user mode must be rejected");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(
        err.reason().contains("system"),
        "rejection must point at system scope: {}",
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
    assert!(err.reason().contains("raw") && err.reason().contains("rpm"));
}
