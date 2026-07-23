//! Tests for `list <component> --versions` version discovery.

use std::path::{Path, PathBuf};

use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError};
use clap::Parser;
use tempfile::tempdir;

use crate::commands::state_view::{StateView, StateVisibility};
use crate::commands::tier1::list::ListArgs;
use crate::commands::tier1::list::versions::{VersionSources, build_versions_payload};
use crate::context::{CliContext, InstallMode};
use crate::repo_config::RepoConfig;

use super::support::{pkg_info, sample_index_with_aliases};

/// Package query double with a controllable repoquery answer — the shared
/// [`super::support::FakeRpmQuery`] models the rpmdb only.
#[derive(Default)]
struct FakeRepoQuery {
    available: Vec<PackageInfo>,
    installed: Option<PackageInfo>,
}

impl PackageQuery for FakeRepoQuery {
    fn query_installed(&self, _package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
        Ok(self.installed.clone())
    }

    fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
        Ok(self.available.clone())
    }

    fn what_provides_installed(&self, _capability: &str) -> Result<Vec<String>, PackageQueryError> {
        Ok(Vec::new())
    }
}

fn system_ctx(root: &Path, prefix: PathBuf) -> CliContext {
    crate::test_support::context_for_root(
        root,
        InstallMode::System,
        Some(prefix),
        crate::test_support::TestContextOptions::default(),
    )
}

/// Raw v1 repo whose index publishes two cosh versions; only the index
/// matters — `matching_versions` never downloads artifacts.
fn write_two_version_raw_repo(root: &Path) -> String {
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dirs");
    let env = anolisa_env::EnvService::detect();
    let entry = |version: &str| {
        format!(
            r#"
[[entries]]
component = "cosh"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "cosh-{version}.tar.gz"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{sha}"
"#,
            os = env.os,
            arch = env.arch,
            sha = "0".repeat(64),
        )
    };
    let index = format!(
        "schema_version = 1\nchannel = \"stable\"\npublisher = \"test\"\n{}{}",
        entry("0.1.0"),
        entry("0.2.0"),
    );
    std::fs::write(v1.join("index.toml"), index).expect("write index");
    format!("file://{}", v1.display())
}

fn repo_config_with(raw_base_url: &str) -> RepoConfig {
    RepoConfig::from_toml_str(&format!(
        r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "{raw_base_url}"

[backends.rpm]
base_url = "https://repo.example/anolisa"
gpgcheck = false
"#
    ))
    .expect("parse repo config")
}

#[test]
fn versions_payload_lists_raw_and_rpm_versions_with_installed_marker() {
    let tmp = tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path(), tmp.path().join("sys"));
    let layout = crate::commands::common::resolve_layout(&ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config = repo_config_with(&write_two_version_raw_repo(&tmp.path().join("repo")));
    let index = sample_index_with_aliases();
    let view = StateView::load(&ctx, "list", StateVisibility::UserPlusSystem).expect("view");
    let rpm_query = FakeRepoQuery {
        available: vec![
            pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"),
            pkg_info("copilot-shell", "2.3.0", Some("1.al8"), "x86_64"),
        ],
        installed: Some(pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64")),
    };
    let sources = VersionSources {
        layout: &layout,
        env: &env,
        repo_config: &repo_config,
        index: &index,
        view: &view,
        rpm_query: &rpm_query,
    };

    let payload = build_versions_payload("cosh", &ctx, &sources).expect("payload");

    assert_eq!(payload.component, "cosh");
    assert!(payload.warnings.is_empty(), "got: {:?}", payload.warnings);
    assert_eq!(payload.backends.len(), 2);

    let raw = &payload.backends[0];
    assert_eq!(raw.backend, "raw");
    assert_eq!(raw.package, "cosh");
    // Highest-first: the head is what a bare `install` would pick.
    let raw_versions: Vec<&str> = raw.versions.iter().map(|r| r.version.as_str()).collect();
    assert_eq!(raw_versions, ["0.2.0", "0.1.0"]);
    assert!(raw.versions.iter().all(|r| !r.installed));

    let rpm = &payload.backends[1];
    assert_eq!(rpm.backend, "rpm");
    assert_eq!(rpm.package, "copilot-shell");
    let rpm_versions: Vec<(&str, bool)> = rpm
        .versions
        .iter()
        .map(|r| (r.version.as_str(), r.installed))
        .collect();
    assert_eq!(
        rpm_versions,
        [("2.3.0-1.al8", false), ("2.2.0-1.al8", true)],
        "EVRs must be highest-first with the installed one marked"
    );
}

#[test]
fn versions_resolves_component_aliases() {
    let tmp = tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path(), tmp.path().join("sys"));
    let layout = crate::commands::common::resolve_layout(&ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config = repo_config_with(&write_two_version_raw_repo(&tmp.path().join("repo")));
    let index = sample_index_with_aliases();
    let view = StateView::load(&ctx, "list", StateVisibility::UserPlusSystem).expect("view");
    let rpm_query = FakeRepoQuery::default();
    let sources = VersionSources {
        layout: &layout,
        env: &env,
        repo_config: &repo_config,
        index: &index,
        view: &view,
        rpm_query: &rpm_query,
    };

    // `cosh-old` is a declared rpm-package alias of `cosh`.
    let payload = build_versions_payload("cosh-old", &ctx, &sources).expect("payload");
    assert_eq!(payload.component, "cosh");
}

#[test]
fn versions_unknown_component_is_invalid_argument() {
    let tmp = tempdir().expect("tmpdir");
    let ctx = system_ctx(tmp.path(), tmp.path().join("sys"));
    let layout = crate::commands::common::resolve_layout(&ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config = repo_config_with("https://example.com/anolisa");
    let index = sample_index_with_aliases();
    let view = StateView::load(&ctx, "list", StateVisibility::UserPlusSystem).expect("view");
    let rpm_query = FakeRepoQuery::default();
    let sources = VersionSources {
        layout: &layout,
        env: &env,
        repo_config: &repo_config,
        index: &index,
        view: &view,
        rpm_query: &rpm_query,
    };

    let err = build_versions_payload("no-such-component", &ctx, &sources)
        .expect_err("unknown component must refuse");
    assert_eq!(err.code(), "INVALID_ARGUMENT");
    assert!(err.reason().contains("no-such-component"));
}

// ── clap surface pairing ─────────────────────────────────────────────

#[test]
fn versions_flag_requires_component() {
    let err = ListArgs::try_parse_from(["list", "--versions"])
        .expect_err("--versions without COMPONENT must be rejected");
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

#[test]
fn component_positional_requires_versions_flag() {
    let err = ListArgs::try_parse_from(["list", "cosh"])
        .expect_err("bare COMPONENT without --versions must be rejected");
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

#[test]
fn installed_filter_conflicts_with_versions() {
    let err = ListArgs::try_parse_from(["list", "cosh", "--versions", "--installed"])
        .expect_err("--installed makes no sense for a version listing");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}
