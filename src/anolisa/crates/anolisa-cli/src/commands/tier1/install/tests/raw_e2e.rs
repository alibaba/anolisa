//! raw_e2e tests for the  command.

use super::super::tests::*;

use anolisa_core::ComponentManifest;
use anolisa_core::domain::{Installation, LifecycleStatus, OwnedArtifact, ProviderBinding};
use anolisa_core::download::DownloadError;
use anolisa_core::state::{
    InstallMode as StateInstallMode, InstalledObject, ObjectKind, ObjectStatus,
};
use anolisa_core::state_store::StateStore;
use anolisa_platform::fs_layout::FsLayout;

use crate::commands::common;
use crate::context::InstallMode;
use crate::test_support::{TestContextOptions, TestSandbox};
use tempfile::tempdir;

const JSON_REASON_ENV: &str = "ANOLISA_TEST_MISSING_INDEX_REASON";
const JSON_BEGIN: &str = "ANOLISA_TEST_JSON_BEGIN";
const JSON_END: &str = "ANOLISA_TEST_JSON_END";

#[test]
#[ignore = "invoked as an isolated child by the missing-index regression"]
fn render_missing_index_error_json_child() {
    let reason = std::env::var(JSON_REASON_ENV)
        .expect("missing-index JSON child must be invoked by its parent regression test");
    let tmp = tempdir().expect("tmpdir");
    let err = crate::response::CliError::Runtime {
        command: "install agentsight".to_string(),
        reason,
    };

    crate::output::flush_stdout();
    crate::output::write_stdout(format_args!("{JSON_BEGIN}"), true);
    crate::output::flush_stdout();
    let exit_code =
        crate::response::render_error(&ctx_with_prefix(true, Some(tmp.path().join("sys"))), &err);
    crate::output::flush_stdout();
    crate::output::write_stdout(format_args!("{JSON_END}"), true);
    crate::output::flush_stdout();
    assert_eq!(exit_code, std::process::ExitCode::from(1));
}

/// v5 store as the pipeline persisted it for a system-prefix layout.
fn load_v5_store(layout: &FsLayout) -> StateStore {
    StateStore::load(&layout.state_dir.join("installed.toml"), 0).expect("state must load")
}

/// The owned artifact behind a component's binding; fails the test on a
/// delegated binding, which also asserts the raw family was recorded.
fn owned_artifact(installation: &Installation) -> &OwnedArtifact {
    match &installation.binding {
        ProviderBinding::Owned { artifact } => artifact,
        other => panic!("expected an owned binding, got {other:?}"),
    }
}

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
fn install_dry_run_does_not_download_the_artifact() {
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

    let mut a = args("remote-only");
    a.repo = Some(repo_url);
    handle_with_fake_rpm(a, &ctx).expect("dry-run must succeed");

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
    assert!(
        cached_names.iter().any(|name| name.ends_with("meta.toml")),
        "dry-run must fetch the lightweight contract; cache entries: {cached_names:?}"
    );
}

#[test]
fn install_reports_missing_local_repository_index() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_root = tmp.path().join("repo");
    let repo_url = write_local_repo(&repo_root);
    let index_path = repo_root.join("v1/index.toml");
    std::fs::remove_dir_all(&repo_root).expect("remove repository directory");

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    let err = handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect_err("missing repository index must fail");
    let reason = err.reason();

    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert_eq!(err.exit_code(), 1);
    assert_eq!(err.command(), "install agentsight");
    assert!(
        reason.contains(&index_path.display().to_string()),
        "missing-index diagnostic must identify {index_path:?}; got: {reason}"
    );
    assert!(
        reason.contains("repo.toml"),
        "missing-index diagnostic must name repo.toml; got: {reason}"
    );
    assert!(
        reason.contains("--repo <URL>"),
        "missing-index diagnostic must name the one-off override; got: {reason}"
    );
    assert!(
        !reason.contains("failed to fetch distribution index"),
        "missing-index diagnostic must not retain the generic wrapper; got: {reason}"
    );

    // Run this test executable as an isolated child so the production renderer's
    // stdout can be asserted without replacing process-global output in-process.
    let child_module = module_path!()
        .split_once("::")
        .map_or(module_path!(), |(_, module)| module);
    let child_test = format!("{child_module}::render_missing_index_error_json_child");
    let output = std::process::Command::new(std::env::current_exe().expect("current test binary"))
        .args([child_test.as_str(), "--exact", "--ignored", "--nocapture"])
        .env(JSON_REASON_ENV, &reason)
        .output()
        .expect("run isolated JSON renderer child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "JSON renderer child failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let (_, after_begin) = stdout
        .split_once(JSON_BEGIN)
        .expect("child stdout must contain the JSON start marker");
    let (json, _) = after_begin
        .split_once(JSON_END)
        .expect("child stdout must contain the JSON end marker");
    let parsed: serde_json::Value =
        serde_json::from_str(json.trim()).expect("rendered error must be valid JSON");
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["schema_version"], crate::response::SCHEMA_VERSION);
    assert_eq!(parsed["command"], "install agentsight");
    assert_eq!(parsed["error"]["code"], "EXECUTION_FAILED");
    assert_eq!(parsed["error"]["reason"], reason);
    let error = parsed["error"]
        .as_object()
        .expect("rendered error payload must be an object");
    assert!(!error.contains_key("hint"));
}

#[test]
fn install_preserves_non_not_found_index_fetch_error() {
    let tmp = tempdir().expect("tmpdir");
    let index_path = tmp.path().join("repo/v1/index.toml");
    let index_url = format!("file://{}", index_path.display());
    let download_error = DownloadError::Io {
        path: index_path,
        source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "index access denied"),
    };
    let rendered_download_error = download_error.to_string();
    let err = super::super::raw::index_fetch_error(&index_url, download_error);

    assert_eq!(
        err.reason(),
        format!("failed to fetch distribution index {index_url}: {rendered_download_error}"),
        "non-NotFound I/O errors must retain the generic diagnostic"
    );
    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert_eq!(err.exit_code(), 1);
    assert!(!err.reason().contains("repo.toml"));
    assert!(!err.reason().contains("--repo <URL>"));
}

#[test]
fn install_preserves_not_found_away_from_index_path() {
    let tmp = tempdir().expect("tmpdir");
    let index_path = tmp.path().join("repo/v1/index.toml");
    let other_path = tmp.path().join("cache/downloads");
    let index_url = format!("file://{}", index_path.display());
    let download_error = DownloadError::Io {
        path: other_path,
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "cache path missing"),
    };
    let rendered_download_error = download_error.to_string();
    let err = super::super::raw::index_fetch_error(&index_url, download_error);

    assert_eq!(
        err.reason(),
        format!("failed to fetch distribution index {index_url}: {rendered_download_error}"),
        "NotFound away from the index path must retain the generic diagnostic"
    );
    assert!(!err.reason().contains("repo.toml"));
    assert!(!err.reason().contains("--repo <URL>"));
}

#[test]
fn install_rejects_local_binary_before_artifact_fetch() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");

    let mut a = args("legacy-bin");
    a.repo = Some(write_binary_repo_component(
        &tmp.path().join("repo"),
        "legacy-bin",
        "1.0.0",
        &["system"],
    ));
    let err = handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect_err("local binary must be rejected");

    assert!(
        err.reason().contains("cannot resolve package 'legacy-bin'")
            && err.reason().contains("no distribution entry matches"),
        "got: {}",
        err.reason()
    );
    assert!(
        !err.reason().contains("publish it as"),
        "got: {}",
        err.reason()
    );
    let downloads = FsLayout::system(Some(prefix)).cache_dir.join("downloads");
    let cached_names: Vec<String> = std::fs::read_dir(downloads)
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
            .all(|name| !name.ends_with("legacy-bin") && !name.ends_with("meta.toml")),
        "rejected binary must not fetch artifact or sidecar: {cached_names:?}"
    );
}

#[test]
fn install_dry_run_rejects_remote_binary_before_artifact_fetch() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_root = tmp.path().join("repo");
    let repo_url = write_binary_repo_component(&repo_root, "remote-bin", "1.0.0", &["system"]);
    let index_path = repo_root.join("v1/index.toml");
    let index = std::fs::read_to_string(&index_path)
        .expect("read index")
        .replace(
            "url = \"remote-bin\"",
            "url = \"https://example.test/remote-bin\"",
        );
    std::fs::write(index_path, index).expect("write remote binary index");

    let mut a = args("remote-bin");
    a.repo = Some(repo_url);
    let mut ctx = ctx_with_prefix(false, Some(prefix.clone()));
    ctx.dry_run = true;
    let err = handle_with_fake_rpm(a, &ctx).expect_err("remote binary must be rejected");

    assert!(
        err.reason().contains("cannot resolve package 'remote-bin'")
            && err.reason().contains("no distribution entry matches"),
        "got: {}",
        err.reason()
    );
    assert!(
        !err.reason().contains("publish it as"),
        "got: {}",
        err.reason()
    );
    let downloads = FsLayout::system(Some(prefix)).cache_dir.join("downloads");
    let cached_names: Vec<String> = std::fs::read_dir(downloads)
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
            .all(|name| !name.ends_with("remote-bin") && !name.ends_with("meta.toml")),
        "rejected binary must not fetch artifact or sidecar: {cached_names:?}"
    );
}

#[test]
fn install_ignores_binary_entries_when_resolving_tar_gz() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_root = tmp.path().join("repo");
    let repo_url = write_local_repo(&repo_root);
    let index_path = repo_root.join("v1/index.toml");
    let mut index = std::fs::read_to_string(&index_path).expect("read index");
    let env = anolisa_env::EnvService::detect();

    for version in ["0.2.0", "9.0.0"] {
        index.push_str(&format!(
            r#"

[[entries]]
component = "agentsight"
version = "{version}"
channel = "stable"
artifact_type = "binary"
backend = "raw"
url = "legacy-agentsight-{version}"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
            sha = "0".repeat(64),
        ));
    }
    std::fs::write(index_path, index).expect("write mixed index");

    let mut a = args("agentsight");
    a.repo = Some(repo_url.clone());
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("tar_gz entry must remain installable");

    let layout = FsLayout::system(Some(prefix));
    assert!(layout.bin_dir.join("agentsight").exists());
    let store = load_v5_store(&layout);
    let installation = store
        .find(ObjectKind::Component, "agentsight")
        .expect("installed object");
    assert_eq!(
        owned_artifact(installation).version,
        "0.2.0",
        "the higher binary-only 9.0.0 entry must be ignored by resolution"
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

    let store = load_v5_store(&layout);
    assert_eq!(store.install_mode, StateInstallMode::System);
    assert_eq!(store.prefix, layout.prefix);
    let installation = store
        .find(ObjectKind::Component, "agentsight")
        .expect("component object must be recorded");
    assert_eq!(installation.status, LifecycleStatus::Installed);
    // `owned_artifact` panics on a delegated binding, so this also asserts
    // the raw family was recorded as the authority.
    let artifact = owned_artifact(installation);
    assert_eq!(artifact.version, "0.2.0");
    assert_eq!(artifact.files.len(), 2);
    assert!(
        artifact.files.iter().any(|file| file.path == manifest_path),
        "installed manifest must be tracked as an owned file"
    );
    assert!(
        artifact
            .distribution_source
            .as_deref()
            .is_some_and(|u| u.starts_with(&repo_url)),
        "distribution_source must record the resolved artifact URL"
    );
    assert_eq!(
        artifact.raw_package.as_deref(),
        Some("agentsight"),
        "raw_package must record the resolved package so update can reuse it"
    );
    assert!(
        artifact.services.iter().all(|s| !s.enabled),
        "install must not mark services enabled"
    );
    assert_eq!(store.operations.len(), 1);
    assert!(store.operations[0].id.starts_with("op-install-"));
}

#[test]
fn system_raw_install_rechecks_native_absence_under_lock() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo(&tmp.path().join("repo"));
    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix));
    let fake = FakeInstaller::new(
        "agentsight",
        pkg_info("agentsight", "0.2.0", Some("1.al8"), "x86_64"),
    )
    .package_appears_under_lock(layout.lock_file.clone());
    let mut a = args("agentsight");
    a.repo = Some(repo_url);

    let err = install_component_with_deps("agentsight", &a, &ctx, &fake, &NoTxn, true)
        .expect_err("an external RPM appearing before raw placement must block install");

    assert!(err.reason().contains("appeared"), "got: {}", err.reason());
    assert!(
        !layout.bin_dir.join("agentsight").exists(),
        "a refused race must not place owned files"
    );
    assert!(
        load_v5_store(&layout)
            .find(ObjectKind::Component, "agentsight")
            .is_none(),
        "a refused race must not claim an owned record"
    );
}

#[test]
fn deb_host_raw_install_rechecks_native_absence_under_lock() {
    // The deb-family relaxation covers a missing rpm binary only. With
    // working tooling on a deb-family host the planning probe resolves a
    // native package, so an RPM appearing between planning and placement
    // must still trip the locked recheck — the family never mutes obtained
    // presence evidence.
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo(&tmp.path().join("repo"));
    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix));
    let fake = FakeInstaller::new(
        "agentsight",
        pkg_info("agentsight", "0.2.0", Some("1.al8"), "x86_64"),
    )
    .package_appears_under_lock(layout.lock_file.clone());
    let mut a = args("agentsight");
    a.repo = Some(repo_url);

    let err = install_component_with_deps_and_env(
        "agentsight",
        &a,
        &ctx,
        &deb_host_env(),
        &RpmdbProbe::absent(),
        &fake,
        &NoTxn,
        true,
    )
    .expect_err("an external RPM appearing before raw placement must block install");

    assert!(err.reason().contains("appeared"), "got: {}", err.reason());
    assert!(
        load_v5_store(&layout)
            .find(ObjectKind::Component, "agentsight")
            .is_none(),
        "a refused race must not claim an owned record"
    );
}

#[test]
fn deb_host_tooling_appearing_under_lock_still_refuses() {
    // The race the planning degrade must not hide: rpm tooling is missing
    // while the deb-family plan resolves (CommandMissing degrades to
    // NotProbed), then tooling plus an external same-named RPM appear
    // before raw placement. The probe identity survives planning, so the
    // locked recheck re-probes, obtains presence evidence, and refuses.
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo(&tmp.path().join("repo"));
    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix));
    let fake = FakeInstaller::new(
        "agentsight",
        pkg_info("agentsight", "0.2.0", Some("1.al8"), "x86_64"),
    )
    .tooling_appears_under_lock(layout.lock_file.clone());
    let mut a = args("agentsight");
    a.repo = Some(repo_url);

    let err = install_component_with_deps_and_env(
        "agentsight",
        &a,
        &ctx,
        &deb_host_env(),
        &RpmdbProbe::absent(),
        &fake,
        &NoTxn,
        true,
    )
    .expect_err("presence evidence gained under the lock must block the raw install");

    assert!(err.reason().contains("appeared"), "got: {}", err.reason());
    assert!(
        load_v5_store(&layout)
            .find(ObjectKind::Component, "agentsight")
            .is_none(),
        "a refused race must not claim an owned record"
    );
}

#[test]
fn deb_host_rpmdb_growing_under_lock_still_refuses() {
    // The degraded CommandMissing policy is a planning-time snapshot: an
    // external process may install RPMs with a binary this process cannot
    // see (absolute path outside PATH), so its queries keep failing while
    // an rpmdb grows on disk. The locked recheck must re-verify the
    // filesystem evidence instead of trusting the captured verdict.
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    // The rpmdb grows under the host probe root, deliberately separate
    // from the install prefix: evidence follows the host, not the layout.
    let host_root = tmp.path().join("host");
    let repo_url = write_local_repo(&tmp.path().join("repo"));
    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix.clone()));
    let rpmdb = RpmdbProbe::with_roots(host_root.clone(), None);
    let fake = FakeInstaller::new(
        "agentsight",
        pkg_info("agentsight", "0.2.0", Some("1.al8"), "x86_64"),
    )
    .rpmdb_appears_under_lock(
        layout.lock_file.clone(),
        host_root.join("var/lib/rpm/rpmdb.sqlite"),
    );
    let mut a = args("agentsight");
    a.repo = Some(repo_url);

    let err = install_component_with_deps_and_env(
        "agentsight",
        &a,
        &ctx,
        &deb_host_env(),
        &rpmdb,
        &fake,
        &NoTxn,
        true,
    )
    .expect_err("an rpmdb appearing under the lock must block the raw install");

    assert!(
        err.reason().contains("rpm database appeared"),
        "got: {}",
        err.reason()
    );
    assert!(
        load_v5_store(&layout)
            .find(ObjectKind::Component, "agentsight")
            .is_none(),
        "a refused race must not claim an owned record"
    );
}

#[test]
fn deb_host_package_override_survives_planning_degrade() {
    // Same race as above but with an explicit `--package` naming an RPM
    // that differs from the component: the override is already the known
    // probe identity, so the CommandMissing degrade must not collapse it
    // to the component name. The locked recheck then probes the RPM the
    // user named, sees it appeared with the tooling, and refuses.
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo(&tmp.path().join("repo"));
    // The raw index is keyed by the backend-native package name, so the
    // override needs an alternate publication of the same artifact under
    // `custom-rpm` for the pre-lock artifact resolution to succeed.
    let index_path = tmp.path().join("repo/v1/index.toml");
    let index = std::fs::read_to_string(&index_path).expect("read index");
    let alternate = index
        .split("[[entries]]")
        .nth(1)
        .expect("fixture entry")
        .replace("component = \"agentsight\"", "component = \"custom-rpm\"");
    std::fs::write(index_path, format!("{index}\n[[entries]]{alternate}"))
        .expect("write alternate publication");
    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix));
    let fake = FakeInstaller::new(
        "custom-rpm",
        pkg_info("custom-rpm", "0.2.0", Some("1.al8"), "x86_64"),
    )
    .tooling_appears_under_lock(layout.lock_file.clone());
    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    a.package = Some("custom-rpm".to_string());

    let err = install_component_with_deps_and_env(
        "agentsight",
        &a,
        &ctx,
        &deb_host_env(),
        &RpmdbProbe::absent(),
        &fake,
        &NoTxn,
        true,
    )
    .expect_err("the overridden RPM appearing under the lock must block the raw install");

    assert!(
        err.reason().contains("'custom-rpm' appeared"),
        "the recheck must probe the --package identity, got: {}",
        err.reason()
    );
    assert!(
        load_v5_store(&layout)
            .find(ObjectKind::Component, "agentsight")
            .is_none(),
        "a refused race must not claim an owned record"
    );
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
    let store = load_v5_store(&layout);
    assert!(
        store.find(ObjectKind::Component, "agentsight").is_some(),
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
    let store = load_v5_store(&layout);
    let installation = store
        .find(ObjectKind::Component, "agentsight")
        .expect("component recorded");
    let artifact = owned_artifact(installation);
    assert_eq!(artifact.services.len(), 1);
    assert_eq!(artifact.services[0].name, "agentsight.service");
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
        let store = load_v5_store(&layout);
        assert!(
            store.find(ObjectKind::Component, "agentsight").is_none(),
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
    let store = load_v5_store(&layout);
    assert!(
        store.find(ObjectKind::Component, "remote-only").is_some(),
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

    let store = load_v5_store(&layout);
    let installation = store
        .find(ObjectKind::Component, "agentsight")
        .expect("component object must be recorded");
    let env = anolisa_env::EnvService::detect();
    assert_eq!(
        owned_artifact(installation).distribution_source.as_deref(),
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

    let store = load_v5_store(&layout);
    let installation = store
        .find(ObjectKind::Component, "agentsight")
        .expect("component object must be recorded");
    let env = anolisa_env::EnvService::detect();
    assert_eq!(
        owned_artifact(installation).distribution_source.as_deref(),
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
    // Two published versions, so the refusal must enumerate the complete
    // list (highest-first), not just whichever entry resolution saw last.
    let repo_url = write_local_repo_component_versions(
        &tmp.path().join("repo"),
        "agentsight",
        &["0.1.0", "0.2.0"],
        &["system"],
    );

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    a.version = Some("9.9.9".to_string());
    let err = handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix)))
        .expect_err("must fail to resolve");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("9.9.9"), "got: {}", err.reason());
    // The pin refusal must name what the repository actually publishes, so
    // the caller can correct the request without a second query.
    assert!(
        err.reason().contains("not published"),
        "pin refusal must be a dedicated message: {}",
        err.reason()
    );
    assert!(
        err.reason().contains("installable versions: 0.2.0, 0.1.0"),
        "pin refusal must list every installable version, highest first: {}",
        err.reason()
    );
}

#[test]
fn install_pinned_unsupported_artifact_type_is_reported_as_not_installable() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_root = tmp.path().join("repo");
    let repo_url = write_local_repo(&repo_root);
    // Publish 9.0.0 as a binary-only entry: present in the repository, but
    // outside what the raw backend can install.
    let index_path = repo_root.join("v1/index.toml");
    let mut index = std::fs::read_to_string(&index_path).expect("read index");
    let env = anolisa_env::EnvService::detect();
    index.push_str(&format!(
        r#"
[[entries]]
component = "agentsight"
version = "9.0.0"
channel = "stable"
artifact_type = "binary"
backend = "raw"
url = "legacy-agentsight-9.0.0"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
        sha = "0".repeat(64),
    ));
    std::fs::write(index_path, index).expect("write mixed index");

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    a.version = Some("9.0.0".to_string());
    let err = handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix)))
        .expect_err("must fail to resolve");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    // A published-but-binary-only pin must not be misreported as
    // unpublished; the refusal names the artifact-type gap instead.
    assert!(
        !err.reason().contains("not published"),
        "published version must not be reported as unpublished: {}",
        err.reason()
    );
    assert!(
        err.reason()
            .contains("artifact types the raw backend cannot install"),
        "refusal must name the artifact-type gap: {}",
        err.reason()
    );
    assert!(
        err.reason().contains("installable versions: 0.2.0"),
        "refusal must still list installable versions: {}",
        err.reason()
    );
}

#[test]
fn install_pinned_version_places_exact_published_version() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_versions(
        &tmp.path().join("repo"),
        "agentsight",
        &["0.1.0", "0.2.0"],
        &["system"],
    );

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    a.version = Some("0.1.0".to_string());
    handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix.clone())))
        .expect("pinned install must succeed");

    let layout = FsLayout::system(Some(prefix));
    assert!(layout.bin_dir.join("agentsight").exists());
    let store = load_v5_store(&layout);
    let installation = store
        .find(ObjectKind::Component, "agentsight")
        .expect("installed object");
    let artifact = owned_artifact(installation);
    assert_eq!(
        artifact.version, "0.1.0",
        "the pin must beat the higher published 0.2.0"
    );
    assert!(
        artifact
            .distribution_source
            .as_deref()
            .is_some_and(|url| url.ends_with("agentsight-0.1.0.tar.gz")),
        "distribution_source must record the pinned artifact URL: {:?}",
        artifact.distribution_source
    );
}

#[test]
fn install_pinned_version_dry_run_previews_without_writing_files() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_versions(
        &tmp.path().join("repo"),
        "agentsight",
        &["0.1.0", "0.2.0"],
        &["system"],
    );

    let mut a = args("agentsight");
    a.repo = Some(repo_url);
    a.version = Some("0.1.0".to_string());
    let mut ctx = ctx_with_prefix(false, Some(prefix.clone()));
    ctx.dry_run = true;
    handle_with_fake_rpm(a, &ctx).expect("pinned dry-run must succeed");

    let layout = FsLayout::system(Some(prefix));
    assert!(
        !layout.bin_dir.join("agentsight").exists(),
        "dry-run must not install the binary"
    );
    assert!(
        !layout.state_dir.join("installed.toml").exists(),
        "dry-run must not write state"
    );
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
    let manifest_sha = format!("{:x}", Sha256::digest(manifest.as_bytes()));
    std::fs::write(v1.join("meta.toml"), &manifest).expect("write sidecar manifest");
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
manifest_digest = "sha256:{manifest_sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(v1.join("index.toml"), index).expect("write index");
    format!("file://{}", v1.display())
}

fn write_published_batch_conflict_repo(root: &Path) -> String {
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dirs");
    let env = anolisa_env::EnvService::detect();
    let mut index =
        String::from("schema_version = 1\nchannel = \"stable\"\npublisher = \"test\"\n");

    for (component, conflicts) in [("cosh", &[][..]), ("cosh-ng", &["cosh"][..])] {
        let version = "1.0.0";
        let version_dir = v1.join(component).join(version);
        let artifact_dir = version_dir.join(&env.os).join(&env.arch);
        std::fs::create_dir_all(&artifact_dir).expect("create artifact dirs");

        let manifest =
            component_manifest_toml_with_conflicts(component, version, &["user"], conflicts);
        std::fs::write(version_dir.join("meta.toml"), &manifest).expect("write sidecar manifest");
        let manifest_sha = format!("{:x}", Sha256::digest(manifest.as_bytes()));
        let bin_path = format!("bin/{component}");
        let payload = format!("#!/bin/sh\necho {component}\n");
        let artifact = build_tar_gz(&[
            (".anolisa/component.toml", manifest.as_bytes()),
            (bin_path.as_str(), payload.as_bytes()),
        ]);
        let artifact_name = format!(
            "{component}-{version}-{os}-{arch}.tar.gz",
            os = env.os,
            arch = env.arch
        );
        std::fs::write(artifact_dir.join(&artifact_name), &artifact).expect("write artifact");
        let artifact_sha = format!("{:x}", Sha256::digest(&artifact));
        let url = format!(
            "{component}/{version}/{os}/{arch}/{artifact_name}",
            os = env.os,
            arch = env.arch
        );
        index.push_str(&format!(
            r#"
[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{url}"
os = "{os}"
arch = "{arch}"
install_modes = ["user"]
sha256 = "{artifact_sha}"
manifest_digest = "sha256:{manifest_sha}"
"#,
            os = env.os,
            arch = env.arch,
        ));
    }
    std::fs::write(v1.join("index.toml"), index).expect("write distribution index");
    std::fs::write(
        v1.join("components.toml"),
        r#"schema_version = 1

[[components]]
name = "cosh"

[[components.backends]]
kind = "raw"
package = "cosh"

[[components]]
name = "cosh-ng"

[[components.backends]]
kind = "raw"
package = "cosh-ng"
"#,
    )
    .expect("write component index");
    format!("file://{}", v1.display())
}

fn seed_installed_component(layout: &FsLayout, component: &str, version: &str) {
    std::fs::create_dir_all(&layout.state_dir).expect("create state dir");
    let mut state = anolisa_core::InstalledState {
        install_mode: StateInstallMode::System,
        prefix: layout.prefix.clone(),
        ..Default::default()
    };
    state.upsert_object(InstalledObject {
        kind: ObjectKind::Component,
        name: component.to_string(),
        version: version.to_string(),
        status: ObjectStatus::Installed,
        manifest_digest: None,
        distribution_source: Some(format!("file:///repo/v1/{component}.tar.gz")),
        raw_package: Some(component.to_string()),
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
}

#[test]
fn install_conflict_blocks_when_conflicting_component_is_installed() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let layout = FsLayout::system(Some(prefix.clone()));

    seed_installed_component(&layout, "cosh", "2.6.0");

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
fn install_dry_run_reports_conflict_without_downloading_artifact() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let layout = FsLayout::system(Some(prefix.clone()));
    seed_installed_component(&layout, "cosh", "2.6.0");
    let repo_url = write_local_repo_with_conflicts(
        &tmp.path().join("repo"),
        "cosh-ng",
        "0.11.0",
        &["system"],
        &["cosh"],
    );

    let mut a = args("cosh-ng");
    a.repo = Some(repo_url);
    let mut ctx = ctx_with_prefix(false, Some(prefix));
    ctx.dry_run = true;
    let err = handle_with_fake_rpm(a, &ctx).expect_err("dry-run must report the conflict");

    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(
        err.reason()
            .contains("conflicts with installed component 'cosh'"),
        "got: {}",
        err.reason()
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
        cached_names.iter().any(|name| name.ends_with("meta.toml")),
        "dry-run must fetch the lightweight contract; cache entries: {cached_names:?}"
    );
    assert!(
        cached_names
            .iter()
            .all(|name| !name.ends_with("cosh-ng.tar.gz")),
        "dry-run must not fetch the install artifact; cache entries: {cached_names:?}"
    );
}

#[test]
fn install_all_dry_run_rejects_conflicts_between_planned_components() {
    let sandbox = TestSandbox::new();
    let repo_url = write_published_batch_conflict_repo(sandbox.repo_root());
    let ctx = sandbox.context_with(
        InstallMode::User,
        TestContextOptions {
            dry_run: true,
            ..Default::default()
        },
    );
    let layout = common::resolve_layout(&ctx);
    std::fs::create_dir_all(&layout.etc_dir).expect("create config dir");
    std::fs::write(
        layout.etc_dir.join("repo.toml"),
        format!(
            "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"{repo_url}\"\n"
        ),
    )
    .expect("write repo config");
    let batch_args = InstallArgs {
        component: None,
        all: true,
        fail_fast: false,
        version: None,
        backend: Some("raw".to_string()),
        repo: None,
        package: None,
    };

    let err = handle_all(batch_args, &ctx)
        .expect_err("the second component must conflict with the first planned component");

    assert!(matches!(err, CliError::BatchPartial { .. }), "got: {err}");
    assert!(
        !layout.state_dir.join("installed.toml").exists(),
        "dry-run must not persist simulated batch state"
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
    assert_eq!(
        cached_names
            .iter()
            .filter(|name| name.ends_with("meta.toml"))
            .count(),
        2,
        "both batch contracts must be checked; cache entries: {cached_names:?}"
    );
    assert!(
        cached_names.iter().all(|name| !name.ends_with(".tar.gz")),
        "dry-run must not fetch batch artifacts; cache entries: {cached_names:?}"
    );
}

#[test]
fn install_dry_run_rejects_mismatched_sidecar_digest() {
    let tmp = tempdir().expect("tmpdir");
    let repo_root = tmp.path().join("repo");
    let repo_url =
        write_local_repo_with_conflicts(&repo_root, "cosh-ng", "0.11.0", &["system"], &["cosh"]);
    std::fs::write(
        repo_root.join("v1/meta.toml"),
        component_manifest_toml("cosh-ng", "0.11.0", &["system"]),
    )
    .expect("replace sidecar manifest");

    let mut a = args("cosh-ng");
    a.repo = Some(repo_url);
    let prefix = tmp.path().join("sys");
    let layout = FsLayout::system(Some(prefix.clone()));
    let mut ctx = ctx_with_prefix(false, Some(prefix));
    ctx.dry_run = true;
    let err = handle_with_fake_rpm(a, &ctx).expect_err("digest mismatch must fail dry-run");

    assert_eq!(err.code(), "EXECUTION_FAILED");
    assert!(err.reason().contains("sha256 mismatch"), "got: {err}");
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
            .all(|name| !name.ends_with("cosh-ng.tar.gz")),
        "digest failure must not fetch the install artifact; cache entries: {cached_names:?}"
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
    let store = load_v5_store(&layout);
    let installation = store
        .find(ObjectKind::Component, "cosh-ng")
        .expect("cosh-ng must be recorded");
    assert_eq!(installation.status, LifecycleStatus::Installed);
    assert_eq!(owned_artifact(installation).version, "0.11.0");
}

/// A future-shaped row for the *queried* component must refuse resolution
/// instead of silently answering with an older parsable version: with
/// 1.0.0 parsable and 2.0.0 unparsable, "latest" would otherwise select
/// 1.0.0 — a silent downgrade for update, a stale install for install.
#[test]
fn skipped_newer_entry_refuses_latest_instead_of_downgrading() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_versions(
        &tmp.path().join("repo"),
        "sec-core",
        &["1.0.0"],
        &["system"],
    );
    // Publish 2.0.0 with an entry shape this build cannot represent.
    let env = anolisa_env::EnvService::detect();
    let index_path = tmp.path().join("repo/v1/index.toml");
    let mut index = std::fs::read_to_string(&index_path).expect("read index");
    index.push_str(&format!(
        r#"
[[entries]]
component = "sec-core"
version = "2.0.0"
channel = "stable"
artifact_type = "hologram_v9"
backend = "raw"
url = "sec-core-2.0.0.tar.gz"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
"#,
        os = env.os,
        arch = env.arch,
    ));
    std::fs::write(&index_path, index).expect("write index");

    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix));
    let inputs = |version: Option<&'static str>| ResolveInputs {
        component: "sec-core".to_string(),
        package: "sec-core".to_string(),
        backend: "raw".to_string(),
        base_url: repo_url.clone(),
        version,
        warnings: Vec::new(),
    };

    // Latest: refuse — the unreadable 2.0.0 may be the answer.
    let err = match resolve_raw(&ctx, &layout, &env, inputs(None)) {
        Ok(_) => panic!("latest must refuse, not fall back to 1.0.0"),
        Err(err) => err,
    };
    assert!(
        err.reason().contains("cannot parse") && err.reason().contains("self-update"),
        "refusal must explain the unparsable entry and hint self-update, got: {}",
        err.reason()
    );
    assert!(
        !err.reason().contains("not found"),
        "must be a refusal, not a not-found, got: {}",
        err.reason()
    );

    // Pinned 2.0.0: equally refused (that exact row is unreadable).
    let err = match resolve_raw(&ctx, &layout, &env, inputs(Some("2.0.0"))) {
        Ok(_) => panic!("pinned 2.0.0 must refuse"),
        Err(err) => err,
    };
    assert!(
        err.reason().contains("cannot parse"),
        "got: {}",
        err.reason()
    );

    // Pinned 1.0.0: an explicit choice of the parsable version resolves,
    // and the skipped row stays visible as a warning.
    let resolution =
        resolve_raw(&ctx, &layout, &env, inputs(Some("1.0.0"))).expect("pinned 1.0.0 resolves");
    assert_eq!(resolution.entry.version, "1.0.0");
    assert!(
        resolution
            .warnings
            .iter()
            .any(|w| w.contains("skipped index entry")),
        "skipped row must surface as a warning, got: {:?}",
        resolution.warnings
    );
}

/// A future-shaped row for an unrelated component neither blocks nor
/// degrades resolution of the queried one; it only surfaces as a warning.
#[test]
fn skipped_unrelated_entry_keeps_component_resolvable() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_versions(
        &tmp.path().join("repo"),
        "cosh",
        &["1.2.3"],
        &["system"],
    );
    let env = anolisa_env::EnvService::detect();
    let index_path = tmp.path().join("repo/v1/index.toml");
    let mut index = std::fs::read_to_string(&index_path).expect("read index");
    index.push_str(&format!(
        r#"
[[entries]]
component = "future-thing"
version = "0.1.0"
channel = "stable"
artifact_type = "hologram_v9"
backend = "raw"
url = "future-thing-0.1.0.tar.gz"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
"#,
        os = env.os,
        arch = env.arch,
    ));
    std::fs::write(&index_path, index).expect("write index");

    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix));
    let resolution = resolve_raw(
        &ctx,
        &layout,
        &env,
        ResolveInputs {
            component: "cosh".to_string(),
            package: "cosh".to_string(),
            backend: "raw".to_string(),
            base_url: repo_url,
            version: None,
            warnings: Vec::new(),
        },
    )
    .expect("unrelated skipped row must not block");
    assert_eq!(resolution.entry.version, "1.2.3");
    assert!(
        resolution
            .warnings
            .iter()
            .any(|w| w.contains("future-thing")),
        "skipped row must surface as a warning, got: {:?}",
        resolution.warnings
    );
}

/// A skipped row whose selectors are damaged (valid TOML, wrong shape —
/// here `install_modes = [1]`) must still block "latest" for its
/// component: partial recovery of the selector would under-block and
/// reopen the silent downgrade this gate exists to prevent.
#[test]
fn skipped_row_with_malformed_selector_still_blocks_latest() {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().join("sys");
    let repo_url = write_local_repo_component_versions(
        &tmp.path().join("repo"),
        "sec-core",
        &["1.0.0"],
        &["system"],
    );
    let env = anolisa_env::EnvService::detect();
    let index_path = tmp.path().join("repo/v1/index.toml");
    let mut index = std::fs::read_to_string(&index_path).expect("read index");
    index.push_str(&format!(
        r#"
[[entries]]
component = "sec-core"
version = "2.0.0"
channel = "stable"
artifact_type = "hologram_v9"
backend = "raw"
url = "sec-core-2.0.0.tar.gz"
os = "{os}"
arch = "{arch}"
install_modes = [1]
"#,
        os = env.os,
        arch = env.arch,
    ));
    std::fs::write(&index_path, index).expect("write index");

    let ctx = ctx_with_prefix(false, Some(prefix.clone()));
    let layout = FsLayout::system(Some(prefix));
    let err = match resolve_raw(
        &ctx,
        &layout,
        &env,
        ResolveInputs {
            component: "sec-core".to_string(),
            package: "sec-core".to_string(),
            backend: "raw".to_string(),
            base_url: repo_url,
            version: None,
            warnings: Vec::new(),
        },
    ) {
        Ok(_) => panic!("malformed selector must not unblock a silent downgrade to 1.0.0"),
        Err(err) => err,
    };
    assert!(
        err.reason().contains("cannot parse") && err.reason().contains("self-update"),
        "got: {}",
        err.reason()
    );
}
