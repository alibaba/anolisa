//! raw_e2e tests for the  command.

use super::super::tests::*;

use anolisa_core::ComponentManifest;
use anolisa_core::state::{
    InstallMode as StateInstallMode, InstalledObject, ObjectKind, ObjectStatus,
};
use anolisa_platform::fs_layout::FsLayout;

use crate::commands::common;
use tempfile::tempdir;

#[test]
fn install_dry_run_resolves_without_writing_files() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo(&tmp.path().join("repo"));

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    let mut ctx = ctx_with_prefix(false, Some(prefix.clone()));
    ctx.dry_run = true;
    handle_with_fake_rpm(a, &ctx).expect("dry-run must succeed");

    let layout = FsLayout::system(Some(prefix));
    assert!(
        !layout.bin_dir.join("agentsight").exists(),
        "dry-run must not install the binary"
    );
    assert!(
        !layout.state_dir.join("installed.toml").exists(),
        "dry-run must not write state"
    );
    let cached_names: Vec<String> = std::fs::read_dir(layout.cache_dir.join("downloads"))
        .expect("downloads cache exists")
        .map(|entry| {
            entry
                .expect("cache entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        cached_names
            .iter()
            .all(|name| !name.ends_with("agentsight.tar.gz")),
        "dry-run must not download the install artifact; cache entries: {cached_names:?}"
    );
}

#[test]
fn install_dry_run_reads_version_meta_without_downloading_artifact() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_published_layout_repo_with_meta(
        &tmp.path().join("repo"),
        "remote-only",
        "1.0.0",
        &["system"],
    );
    let mut ctx = ctx_with_prefix(false, Some(prefix.clone()));
    ctx.dry_run = true;
    let layout = FsLayout::system(Some(prefix));
    let env = anolisa_env::EnvService::detect();

    let resolution = resolve_raw(
        &ctx,
        &layout,
        &env,
        ResolveInputs {
            component: "remote-only".to_string(),
            package: "remote-only".to_string(),
            backend: "raw".to_string(),
            base_url: repo_url,
            version: None,
            warnings: Vec::new(),
        },
    )
    .expect("resolve");
    let preview =
        build_install_preview(&ctx, &layout, &Default::default(), resolution).expect("preview");

    assert_eq!(preview.files.len(), 1);
    assert_eq!(preview.files[0].dest, layout.bin_dir.join("remote-only"));
    assert!(
        preview
            .resolution
            .warnings
            .iter()
            .all(|warning| !warning.contains("file and service details are unavailable")),
        "version-level meta.toml should provide file details: {:?}",
        preview.resolution.warnings
    );

    let cached_names: Vec<String> = std::fs::read_dir(layout.cache_dir.join("downloads"))
        .expect("downloads cache exists")
        .map(|entry| {
            entry
                .expect("cache entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        cached_names
            .iter()
            .all(|name| !name.ends_with("remote-only-1.0.0-linux-x86_64.tar.gz")),
        "dry-run must not download the install artifact; cache entries: {cached_names:?}"
    );
}

#[test]
fn install_binary_artifact_uses_local_catalog_contract() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let layout = FsLayout::system(Some(prefix.clone()));
    write_overlay_manifest(&layout, "legacy-bin", "1.0.0", &["system"]);

    let mut a = args("legacy-bin");
    a.repo = Some(write_binary_repo_component(
        &tmp.path().join("repo"),
        "legacy-bin",
        "1.0.0",
        &["system"],
    ));

    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install must succeed");

    let bin = FsLayout::system(Some(prefix)).bin_dir.join("legacy-bin");
    assert!(bin.exists(), "binary artifact must be installed");
    assert_eq!(
        std::fs::read_to_string(&bin).expect("read installed binary"),
        "#!/bin/sh\necho legacy-bin\n"
    );
}

#[test]
fn install_raw_end_to_end_from_local_repo() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo(&tmp.path().join("repo"));

    let mut a = args("agentsight");
    a.repo = Some(repo_url.clone());
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install must succeed");

    let layout = FsLayout::system(Some(prefix));
    let bin = layout.bin_dir.join("agentsight");
    assert!(bin.exists(), "binary must be installed at {{bindir}}");
    let manifest_path = common::installed_component_manifest_path(&layout, "agentsight", COMMAND)
        .expect("manifest path");
    assert!(
        manifest_path.exists(),
        "installed component manifest must be persisted"
    );
    let saved_manifest =
        ComponentManifest::from_file(&manifest_path).expect("saved manifest parses");
    assert_eq!(saved_manifest.component.name, "agentsight");
    assert_eq!(saved_manifest.component.version, "0.2.0");

    let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
        .expect("state must load");
    let obj = state
        .find_object(ObjectKind::Component, "agentsight")
        .expect("component object must be recorded");
    assert_eq!(obj.version, "0.2.0");
    assert_eq!(obj.status, ObjectStatus::Installed);
    assert_eq!(obj.files.len(), 2);
    assert!(
        obj.files.iter().any(|file| file.path == manifest_path),
        "installed manifest must be tracked as an owned file"
    );
    assert!(
        obj.distribution_source
            .as_deref()
            .is_some_and(|u| u.starts_with(&repo_url)),
        "distribution_source must record the resolved artifact URL"
    );
    assert_eq!(
        obj.raw_package.as_deref(),
        Some("agentsight"),
        "raw_package must record the resolved package so update can reuse it"
    );
    assert_eq!(
        obj.install_backend.as_deref(),
        Some("raw"),
        "install_backend must record the selected backend"
    );
    assert!(
        obj.services.iter().all(|s| !s.enabled),
        "install must not mark services enabled"
    );
    assert_eq!(state.operations.len(), 1);
    assert!(state.operations[0].id.starts_with("op-install-"));
}

#[test]
fn prepare_raw_execution_resolves_declared_capabilities() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_with_capability(
        &tmp.path().join("repo"),
        "agentsight",
        "0.2.0",
        &["system"],
        "{bindir}/agentsight",
        &["CAP_BPF", "CAP_PERFMON"],
        true,
    );
    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix.clone()));
    let env = anolisa_env::EnvService::detect();
    let resolution = resolve_raw(
        &ctx,
        &layout,
        &env,
        ResolveInputs {
            component: "agentsight".to_string(),
            package: "agentsight".to_string(),
            backend: "raw".to_string(),
            base_url: repo_url,
            version: None,
            warnings: Vec::new(),
        },
    )
    .expect("resolve");
    let prepared = prepare_raw_execution(&ctx, &layout, resolution).expect("prepare");

    assert_eq!(prepared.capabilities.len(), 1);
    assert_eq!(
        prepared.capabilities[0].path,
        layout.bin_dir.join("agentsight")
    );
    assert_eq!(
        prepared.capabilities[0].caps,
        vec!["CAP_BPF".to_string(), "CAP_PERFMON".to_string()]
    );
    assert!(prepared.capabilities[0].optional);
    // Resolve-only: no setcap, no file laid, no state.
    assert!(!layout.bin_dir.join("agentsight").exists());
    assert!(!layout.state_dir.join("installed.toml").exists());
}

#[test]
fn install_raw_end_to_end_applies_optional_capability() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_with_capability(
        &tmp.path().join("repo"),
        "agentsight",
        "0.2.0",
        &["system"],
        "{bindir}/agentsight",
        &["CAP_BPF"],
        true,
    );

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install with optional capability must succeed even without root");

    let layout = FsLayout::system(Some(prefix));
    assert!(
        layout.bin_dir.join("agentsight").exists(),
        "binary must be installed even when the optional setcap is skipped"
    );
    let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
        .expect("state must load");
    assert!(
        state
            .find_object(ObjectKind::Component, "agentsight")
            .is_some(),
        "component must be recorded despite optional capability outcome"
    );
}

#[test]
fn prepare_raw_execution_resolves_declared_services() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_with_service(
        &tmp.path().join("repo"),
        "agentsight",
        "0.2.0",
        &["system"],
        "agentsight.service",
        true,
        true,
    );
    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix.clone()));
    let env = anolisa_env::EnvService::detect();
    let resolution = resolve_raw(
        &ctx,
        &layout,
        &env,
        ResolveInputs {
            component: "agentsight".to_string(),
            package: "agentsight".to_string(),
            backend: "raw".to_string(),
            base_url: repo_url,
            version: None,
            warnings: Vec::new(),
        },
    )
    .expect("resolve");
    let prepared = prepare_raw_execution(&ctx, &layout, resolution).expect("prepare");

    assert_eq!(prepared.services.len(), 1);
    assert_eq!(prepared.services[0].unit, "agentsight.service");
    assert!(prepared.services[0].enable && prepared.services[0].start);
    // Resolve-only: nothing activated or laid.
    assert!(!layout.bin_dir.join("agentsight").exists());
    assert!(!layout.state_dir.join("installed.toml").exists());
}

#[test]
fn install_raw_end_to_end_records_declared_service() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_with_service(
        &tmp.path().join("repo"),
        "agentsight",
        "0.2.0",
        &["system"],
        "agentsight.service",
        true,
        true,
    );

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install with a declared service must succeed (activation is best-effort)");

    let layout = FsLayout::system(Some(prefix));
    assert!(
        layout.bin_dir.join("agentsight").exists(),
        "binary installed"
    );
    let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
        .expect("state must load");
    let obj = state
        .find_object(ObjectKind::Component, "agentsight")
        .expect("component recorded");
    assert_eq!(obj.services.len(), 1);
    assert_eq!(obj.services[0].name, "agentsight.service");
}

#[test]
#[cfg(unix)]
fn install_raw_runs_post_install_hook() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let sentinel = tmp.path().join("post-install.ran");
    let body = format!("#!/bin/sh\ntouch {}\n", sentinel.display());
    let repo_url = write_local_repo_component_with_hook(
        &tmp.path().join("repo"),
        "agentsight",
        "0.2.0",
        "post_install",
        false,
        &body,
    );

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install with a post_install hook must succeed");

    let layout = FsLayout::system(Some(prefix));
    assert!(
        layout.bin_dir.join("agentsight").exists(),
        "binary installed"
    );
    assert!(
        sentinel.exists(),
        "post_install hook must run after files are laid down"
    );
}

#[test]
#[cfg(unix)]
fn install_raw_strict_post_install_failure_rolls_back() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_with_hook(
        &tmp.path().join("repo"),
        "agentsight",
        "0.2.0",
        "post_install",
        true,
        "#!/bin/sh\nexit 1\n",
    );

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    let err = handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect_err("strict post_install failure must abort the install");
    assert!(matches!(err, CliError::Runtime { .. }));

    let layout = FsLayout::system(Some(prefix));
    assert!(
        !layout.bin_dir.join("agentsight").exists(),
        "installed files must be rolled back after a strict hook failure"
    );
    let snapshot = common::installed_component_manifest_path(&layout, "agentsight", COMMAND)
        .expect("manifest path");
    assert!(
        !snapshot.exists(),
        "installed manifest snapshot must be rolled back"
    );
    let state_path = layout.state_dir.join("installed.toml");
    if state_path.exists() {
        let state = anolisa_core::InstalledState::load(&state_path).expect("state load");
        assert!(
            state
                .find_object(ObjectKind::Component, "agentsight")
                .is_none(),
            "component must not be recorded after rollback"
        );
    }
}

#[test]
#[cfg(unix)]
fn install_raw_pre_install_hook_skipped_as_missing_on_fresh_install() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let sentinel = tmp.path().join("pre-install.ran");
    let body = format!("#!/bin/sh\ntouch {}\n", sentinel.display());
    let repo_url = write_local_repo_component_with_hook(
        &tmp.path().join("repo"),
        "agentsight",
        "0.2.0",
        "pre_install",
        false,
        &body,
    );

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install must succeed; pre_install script is not yet laid");

    let layout = FsLayout::system(Some(prefix));
    assert!(
        layout.bin_dir.join("agentsight").exists(),
        "binary installed"
    );
    assert!(
        !sentinel.exists(),
        "pre_install must skip when its script is not yet on disk"
    );
}

#[test]
fn install_raw_uses_embedded_manifest_without_local_catalog() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component(
        &tmp.path().join("repo"),
        "remote-only",
        "1.0.0",
        &["system"],
    );

    let mut a = args("remote-only");
    a.repo = Some(repo_url);
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install must succeed");

    let layout = FsLayout::system(Some(prefix));
    assert!(
        layout.bin_dir.join("remote-only").exists(),
        "component absent from local manifests must install from embedded artifact contract"
    );
    let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
        .expect("state must load");
    assert!(
        state
            .find_object(ObjectKind::Component, "remote-only")
            .is_some(),
        "remote-only component must be recorded"
    );
}

#[test]
fn install_existing_component_with_different_backend_is_invalid_argument() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let layout = FsLayout::system(Some(prefix.clone()));
    std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
    std::fs::create_dir_all(&layout.state_dir).expect("state dir");
    std::fs::write(
        layout.etc_dir.join("repo.toml"),
        r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "https://example.com/anolisa"

[backends.npm]
base_url = "https://registry.npmjs.org"
scope = "@anolisa"
"#,
    )
    .expect("write repo.toml");

    let mut state = anolisa_core::InstalledState {
        install_mode: StateInstallMode::System,
        prefix: layout.prefix.clone(),
        ..Default::default()
    };
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: "agentsight".to_string(),
        version: "0.2.0".to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: Some("file:///repo/v1/agentsight-bin".to_string()),
        raw_package: None,
        install_backend: Some("raw".to_string()),
        ownership: None,
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
        .save(&layout.state_dir.join("installed.toml"))
        .expect("save state");

    let mut a = args("agentsight");
    a.backend = Some("npm".to_string());
    let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");

    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(
        err.reason().contains("already installed via backend 'raw'")
            && err.reason().contains("backend 'npm'"),
        "reason must explain backend conflict: {}",
        err.reason()
    );
}

#[test]
fn install_derives_artifact_url_from_convention_when_index_omits_url() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_conventional_repo(&tmp.path().join("repo"));

    let mut a = args("agentsight");
    a.repo = Some(repo_url.clone());
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install must succeed");

    let layout = FsLayout::system(Some(prefix));
    assert!(layout.bin_dir.join("agentsight").exists());

    let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
        .expect("state must load");
    let obj = state
        .find_object(ObjectKind::Component, "agentsight")
        .expect("component object must be recorded");
    let env = anolisa_env::EnvService::detect();
    assert_eq!(
        obj.distribution_source.as_deref(),
        Some(
            format!(
                "{repo_url}/agentsight/0.2.0/{os}/{arch}/agentsight-0.2.0-{os}-{arch}.tar.gz",
                os = env.os,
                arch = env.arch
            )
            .as_str()
        ),
        "distribution_source must record the convention-derived URL"
    );
}

#[test]
fn install_resolves_legacy_template_form_repo_url() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_root = tmp.path().join("repo");
    // write_conventional_repo puts the tree under <root>/v1/; point the
    // template's static prefix at that same directory.
    let _ = write_conventional_repo(&repo_root);
    let template_url = format!(
        "file://{}/v1/{{component}}/{{version}}/{{os}}/{{arch}}/",
        repo_root.display()
    );

    let mut a = args("agentsight");
    a.repo = Some(template_url);
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install must succeed");

    let layout = FsLayout::system(Some(prefix));
    assert!(layout.bin_dir.join("agentsight").exists());

    let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
        .expect("state must load");
    let obj = state
        .find_object(ObjectKind::Component, "agentsight")
        .expect("component object must be recorded");
    let env = anolisa_env::EnvService::detect();
    assert_eq!(
        obj.distribution_source.as_deref(),
        Some(
            format!(
                "file://{}/v1/agentsight/0.2.0/{os}/{arch}/agentsight-0.2.0-{os}-{arch}.tar.gz",
                repo_root.display(),
                os = env.os,
                arch = env.arch
            )
            .as_str()
        ),
        "distribution_source must record the convention-derived URL"
    );
}

#[test]
fn install_unpublished_version_is_invalid_argument() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo(&tmp.path().join("repo"));

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    a.version = Some("9.9.9".to_string());
    let err = handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix)))
        .expect_err("must fail to resolve");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("9.9.9"), "got: {}", err.reason());
}

// ---------------------------------------------------------------------------
// Component-level mutual-exclusion (conflicts) tests
// ---------------------------------------------------------------------------

/// Build a local file:// repo with a single component whose manifest declares
/// `conflicts = [...]`. Returns the repo URL.
fn write_local_repo_with_conflicts(
    root: &std::path::Path,
    component: &str,
    version: &str,
    modes: &[&str],
    conflicts: &[&str],
) -> String {
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dirs");

    let manifest = component_manifest_toml_with_conflicts(component, version, modes, conflicts);
    let bin_path = format!("bin/{component}");
    let payload = format!("#!/bin/sh\necho {component}\n");
    let artifact = build_tar_gz(&[
        (".anolisa/component.toml", manifest.as_bytes()),
        (bin_path.as_str(), payload.as_bytes()),
    ]);
    let artifact_name = format!("{component}.tar.gz");
    std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
    let sha = format!("{:x}", Sha256::digest(&artifact));
    let modes_str = toml_string_array(modes);

    let env = anolisa_env::EnvService::detect();
    let index = format!(
        r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = {modes_str}
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(v1.join("index.toml"), index).expect("write index");
    format!("file://{}", v1.display())
}

#[test]
fn install_conflict_blocks_when_conflicting_component_is_installed() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let layout = FsLayout::system(Some(prefix.clone()));

    // Pre-seed state: cosh v2.6.0 is already installed.
    std::fs::create_dir_all(&layout.state_dir).expect("create state dir");
    let mut state = anolisa_core::InstalledState {
        install_mode: StateInstallMode::System,
        prefix: layout.prefix.clone(),
        ..Default::default()
    };
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: "cosh".to_string(),
        version: "2.6.0".to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: Some("file:///repo/v1/cosh.tar.gz".to_string()),
        raw_package: Some("cosh".to_string()),
        install_backend: Some("raw".to_string()),
        ownership: None,
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
        .save(&layout.state_dir.join("installed.toml"))
        .expect("save state");

    // Write a repo with cosh-ng that declares conflicts = ["cosh"].
    let repo_url = write_local_repo_with_conflicts(
        &tmp.path().join("repo"),
        "cosh-ng",
        "0.11.0",
        &["system"],
        &["cosh"],
    );

    let mut a = args("cosh-ng");
    a.repo = Some(repo_url);
    let err = handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix)))
        .expect_err("install must fail due to conflict");

    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(
        err.reason()
            .contains("conflicts with installed component 'cosh'"),
        "error must identify the conflicting component: {}",
        err.reason()
    );
    assert!(
        err.reason().contains("v2.6.0"),
        "error must show the installed version: {}",
        err.reason()
    );
    assert!(
        err.reason().contains("uninstall 'cosh' first"),
        "error must provide remediation: {}",
        err.reason()
    );
}

#[test]
fn install_no_conflict_when_conflicting_component_not_installed() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");

    // Write a repo with cosh-ng that declares conflicts = ["cosh"], but cosh
    // is NOT installed — install should succeed.
    let repo_url = write_local_repo_with_conflicts(
        &tmp.path().join("repo"),
        "cosh-ng",
        "0.11.0",
        &["system"],
        &["cosh"],
    );

    let mut a = args("cosh-ng");
    a.repo = Some(repo_url);
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("install must succeed when no conflict");

    // Verify cosh-ng is recorded in state.
    let layout = FsLayout::system(Some(prefix));
    let state = anolisa_core::InstalledState::load(&layout.state_dir.join("installed.toml"))
        .expect("state must load");
    let obj = state
        .find_object(ObjectKind::Component, "cosh-ng")
        .expect("cosh-ng must be recorded");
    assert_eq!(obj.version, "0.11.0");
    assert_eq!(obj.status, ObjectStatus::Installed);
}
