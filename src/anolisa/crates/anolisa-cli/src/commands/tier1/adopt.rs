//! `anolisa adopt <component>` — record an already-installed system RPM as
//! `rpm-observed` ANOLISA state.
//!
//! Adoption is the explicit counterpart to `install`'s implicit system-mode
//! adoption: it fetches nothing and runs no `dnf`/`rpm` transaction — it reads
//! rpmdb and writes the observation. It is a system-scope action: a unique
//! installed RPM for the component is recorded with `Ownership::RpmObserved`,
//! while ambiguous, absent, or already-tracked (non-observed) components are
//! refused with guidance toward the right command.

use clap::Parser;

use anolisa_core::state::{ObjectKind, Ownership};
use anolisa_platform::pkg_query::PackageQuery;
use anolisa_platform::rpm_query::RpmPackageQuery;

use crate::commands::common;
use crate::commands::tier1::install::{RpmSituation, execute_adopt, probe_rpm_situation};
use crate::context::{CliContext, InstallMode};
use crate::repo_config::RepoConfig;
use crate::response::CliError;

/// Command label for JSON envelopes and error routing.
const COMMAND: &str = "adopt";

/// Arguments for `anolisa adopt <component>`.
#[derive(Debug, Parser)]
pub struct AdoptArgs {
    /// Component to record as an existing system RPM
    #[arg(value_name = "COMPONENT")]
    pub component: String,
    /// Pin the RPM package name when the component maps to several candidates
    #[arg(long, value_name = "NAME")]
    pub package: Option<String>,
}

/// Dispatch `adopt <component>`: probe rpmdb and record the unique installed RPM
/// as `rpm-observed`.
///
/// # Errors
///
/// Returns [`CliError`] in user mode, when the component is already tracked
/// under a non-observed ownership, when no / ambiguous / multi-version RPM is
/// found, or when the state write fails.
pub fn handle(args: AdoptArgs, ctx: &CliContext) -> Result<(), CliError> {
    let query = RpmPackageQuery::system();
    adopt_with_query(&args.component, args.package.as_deref(), ctx, &query)
}

/// Core of [`handle`] with the package query injected so tests drive every
/// branch without a live rpmdb. Adopt runs no dnf transaction, so only a
/// [`PackageQuery`] is required.
// pub(crate): driven by the cross-command MVP lifecycle test (#963).
pub(crate) fn adopt_with_query(
    target: &str,
    cli_override: Option<&str>,
    ctx: &CliContext,
    query: &dyn PackageQuery,
) -> Result<(), CliError> {
    let command = format!("adopt {target}");

    // Adoption records a *system* RPM into system-scope state; user mode has no
    // system rpmdb to observe into. Refuse rather than record a system package
    // against user-scope state (mirrors install's `--backend rpm` user-mode
    // rejection).
    if ctx.install_mode != InstallMode::System {
        return Err(CliError::InvalidArgument {
            command,
            reason: format!(
                "adopt records a system RPM and requires system scope; re-run with --install-mode system (got {} mode)",
                ctx.install_mode.as_str()
            ),
        });
    }

    let installed = common::load_installed_state(ctx, COMMAND)?;

    // Tracked-component gate (pre-lock fast-fail for a clear, early message).
    // Re-adopting a component that is already `rpm-observed` is a refresh and
    // falls through to execute_adopt, which upserts. A component owned under any
    // other provenance must not be silently downgraded to rpm-observed — that is
    // an ownership migration, left for later — so refuse and point at the right
    // tool. This is the friendly path; execute_adopt re-enforces the same policy
    // atomically under the lock (covering a concurrent managed install).
    if let Some(obj) = installed.find_object(ObjectKind::Component, target) {
        match obj.effective_ownership() {
            Ownership::RpmObserved => {}
            Ownership::RpmManaged => {
                return Err(CliError::InvalidArgument {
                    command,
                    reason: format!(
                        "component '{target}' is already tracked as rpm-managed; run `anolisa repair {target}` to refresh its state from rpmdb"
                    ),
                });
            }
            Ownership::RawManaged => {
                return Err(CliError::InvalidArgument {
                    command,
                    reason: format!(
                        "component '{target}' is already tracked as a raw install; run `anolisa uninstall {target}` first to re-adopt it as an rpm-observed system package"
                    ),
                });
            }
        }
    }

    let layout = common::resolve_layout(ctx);
    // repo.toml only feeds the package-name map (`[backends.rpm].package_map`).
    // It is supplementary: --package, the provides contract, and the default
    // name still resolve a candidate without it, so a missing/unreadable config
    // degrades to "no rpm backend config" rather than failing the adopt.
    let repo_config = RepoConfig::load(&layout).ok();
    let rpm_backend = repo_config.as_ref().and_then(|c| c.backends.get("rpm"));

    match probe_rpm_situation(target, cli_override, rpm_backend, ctx, query, &command)? {
        // Exactly one installed RPM: record it (or preview on --dry-run). reused
        // verbatim from the install path, including the lock-held re-validation.
        RpmSituation::Adoptable { package, info } => {
            execute_adopt(ctx, &layout, &command, target, package, info, query)?;
            Ok(())
        }
        // Nothing installed under this name: adopt never installs, so point at
        // install for the delegated `dnf install` path.
        RpmSituation::Absent { package } => Err(CliError::InvalidArgument {
            command,
            reason: format!(
                "no installed RPM '{package}' found for component '{target}'; adopt only records an already-installed system RPM — run `anolisa --install-mode system install {target}` to install it"
            ),
        }),
        // Several provider packages: the user must disambiguate.
        RpmSituation::Ambiguous(names) => Err(CliError::InvalidArgument {
            command,
            reason: format!(
                "multiple installed RPMs provide component '{target}': {}; cannot adopt unambiguously — pin one with `--package <name>`",
                names.join(", ")
            ),
        }),
        // One name, several installed versions: a drift the user must resolve.
        RpmSituation::MultiVersion(package) => Err(CliError::InvalidArgument {
            command,
            reason: format!(
                "RPM package '{package}' has multiple installed versions; refusing to adopt a single version automatically — resolve the duplicate first"
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus, RpmMetadata,
    };
    use anolisa_platform::pkg_query::{PackageInfo, PackageQueryError, PackageVersion};

    /// In-memory [`PackageQuery`] for the adopt tests. Adopt runs no transaction,
    /// so a query alone drives every branch (probe + origin lookup).
    #[derive(Default)]
    struct FakeQuery {
        /// package name → installed info reported by `query_installed`.
        installed: Vec<(String, PackageInfo)>,
        /// package names that report several installed versions.
        multi_version: Vec<String>,
        /// capability → provider package names for `what_provides_installed`.
        provides: Vec<(String, Vec<String>)>,
        /// package → source repo for `installed_origin`.
        origins: Vec<(String, String)>,
    }

    impl PackageQuery for FakeQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if self.multi_version.iter().any(|p| p == package) {
                return Err(PackageQueryError::UnexpectedOutput {
                    command: "rpm".to_string(),
                    detail: "2 installed versions".to_string(),
                });
            }
            Ok(self
                .installed
                .iter()
                .find(|(p, _)| p == package)
                .map(|(_, info)| info.clone()))
        }
        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }
        fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
            Ok(self
                .origins
                .iter()
                .find(|(p, _)| p == package)
                .map(|(_, o)| o.clone()))
        }
        fn what_provides_installed(
            &self,
            capability: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            Ok(self
                .provides
                .iter()
                .find(|(c, _)| c == capability)
                .map(|(_, v)| v.clone())
                .unwrap_or_default())
        }
    }

    fn pkg_info(name: &str, version: &str, release: Option<&str>, arch: &str) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: PackageVersion {
                epoch: None,
                version: version.to_string(),
                release: release.map(str::to_string),
            },
            arch: arch.to_string(),
            origin: None,
        }
    }

    fn ctx(prefix: PathBuf, install_mode: InstallMode, dry_run: bool) -> CliContext {
        CliContext {
            install_mode,
            prefix: Some(prefix),
            json: false,
            dry_run,
            verbose: false,
            quiet: true,
            no_color: true,
        }
    }

    /// A tracked component object with the given provenance.
    fn component_object(name: &str, ownership: Ownership, status: ObjectStatus) -> InstalledObject {
        let is_rpm = ownership.is_rpm();
        InstalledObject {
            kind: ObjectKind::Component,
            name: name.to_string(),
            version: "1.0.0-1.al8".to_string(),
            status,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some(if is_rpm { "rpm" } else { "raw" }.to_string()),
            ownership: Some(ownership),
            rpm_metadata: is_rpm.then(|| RpmMetadata {
                package_name: name.to_string(),
                evr: Some("1.0.0-1.al8".to_string()),
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            }),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: matches!(ownership, Ownership::RawManaged | Ownership::RpmManaged),
            adopted: matches!(ownership, Ownership::RpmObserved),
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        }
    }

    /// Write a seed state (creating the state dir) so the lock-held write path
    /// has somewhere to land.
    fn seed(ctx: &CliContext, objs: Vec<InstalledObject>) {
        let layout = common::resolve_layout(ctx);
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let mut state = InstalledState {
            install_mode: StateInstallMode::System,
            prefix: layout.prefix.clone(),
            ..Default::default()
        };
        for obj in objs {
            state.upsert_object(obj);
        }
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state");
    }

    fn load_state(ctx: &CliContext) -> InstalledState {
        let layout = common::resolve_layout(ctx);
        InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state")
    }

    /// A unique installed RPM with no prior state is recorded as `rpm-observed`.
    #[test]
    fn adopt_records_unique_rpm_as_observed() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(&c, Vec::new());
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"),
            )],
            origins: vec![("copilot-shell".to_string(), "@System".to_string())],
            ..Default::default()
        };
        adopt_with_query("copilot-shell", None, &c, &q).expect("adopt ok");

        let after = load_state(&c);
        let obj = after
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("object recorded");
        assert_eq!(obj.effective_ownership(), Ownership::RpmObserved);
        assert_eq!(obj.status, ObjectStatus::Adopted);
        let meta = obj.rpm_metadata.as_ref().expect("rpm metadata");
        assert_eq!(meta.package_name, "copilot-shell");
        assert_eq!(meta.evr.as_deref(), Some("2.2.0-1.al8"));
        assert!(
            after
                .operations
                .iter()
                .any(|o| o.command == "adopt copilot-shell"),
            "an operation record must be appended",
        );
    }

    /// Re-adopting an already `rpm-observed` component refreshes its EVR and
    /// keeps it rpm-observed (idempotent).
    #[test]
    fn adopt_refreshes_existing_rpm_observed() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            )],
        );
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.0.0", Some("1.al8"), "x86_64"),
            )],
            ..Default::default()
        };
        adopt_with_query("copilot-shell", None, &c, &q).expect("refresh ok");

        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("object")
            .clone();
        assert_eq!(obj.effective_ownership(), Ownership::RpmObserved);
        assert_eq!(
            obj.rpm_metadata.and_then(|m| m.evr).as_deref(),
            Some("2.0.0-1.al8"),
            "EVR refreshed from rpmdb",
        );
    }

    /// Re-adopting an rpm-observed component with `--package` pointing at a
    /// *different* RPM is a package-identity migration, not a refresh: it must be
    /// refused under the lock (not silently overwrite `rpm_metadata.package_name`),
    /// and steer the user through forget→adopt.
    #[test]
    fn adopt_refuses_repointing_observed_to_different_package() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            )],
        );
        // Existing observed package is `copilot-shell`; the user pins a
        // different installed package via --package.
        let q = FakeQuery {
            installed: vec![(
                "anolisa-other".to_string(),
                pkg_info("anolisa-other", "9.9.9", Some("1.al8"), "x86_64"),
            )],
            ..Default::default()
        };
        let err = adopt_with_query("copilot-shell", Some("anolisa-other"), &c, &q)
            .expect_err("repointing to a different package must be refused");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("copilot-shell")
                && err.reason().contains("anolisa-other")
                && err.reason().contains("forget"),
            "refusal must name both packages and point at forget: {}",
            err.reason(),
        );
        // The state must be untouched — no repoint, no EVR bump.
        let meta = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("object")
            .rpm_metadata
            .clone()
            .expect("rpm metadata");
        assert_eq!(
            meta.package_name, "copilot-shell",
            "package identity must be preserved when the repoint is refused",
        );
        assert_eq!(meta.evr.as_deref(), Some("1.0.0-1.al8"), "EVR unchanged");
    }

    /// The repoint refusal must also fire on `--dry-run`: the preview cannot
    /// promise `Would adopt...` for a switch the real run would reject. Mirrors
    /// the real-run test above with a dry-run context.
    #[test]
    fn adopt_dry_run_refuses_repointing_observed_to_different_package() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            )],
        );
        let q = FakeQuery {
            installed: vec![(
                "anolisa-other".to_string(),
                pkg_info("anolisa-other", "9.9.9", Some("1.al8"), "x86_64"),
            )],
            ..Default::default()
        };
        let err = adopt_with_query("copilot-shell", Some("anolisa-other"), &c, &q)
            .expect_err("dry-run must refuse the repoint, matching the real run");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("copilot-shell")
                && err.reason().contains("anolisa-other")
                && err.reason().contains("forget"),
            "dry-run refusal must match the real run: {}",
            err.reason(),
        );
        // Dry-run never writes, so the record is untouched regardless.
        let meta = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("object")
            .rpm_metadata
            .clone()
            .expect("rpm metadata");
        assert_eq!(meta.package_name, "copilot-shell");
    }

    /// A raw-managed component is not silently downgraded; adopt points at
    /// uninstall.
    #[test]
    fn adopt_refuses_raw_managed() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RawManaged,
                ObjectStatus::Installed,
            )],
        );
        let err = adopt_with_query("copilot-shell", None, &c, &FakeQuery::default())
            .expect_err("raw must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("uninstall"),
            "raw refusal points at uninstall: {}",
            err.reason()
        );
    }

    /// An rpm-managed component is refused; adopt points at repair.
    #[test]
    fn adopt_refuses_rpm_managed() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            vec![component_object(
                "copilot-shell",
                Ownership::RpmManaged,
                ObjectStatus::Installed,
            )],
        );
        let err = adopt_with_query("copilot-shell", None, &c, &FakeQuery::default())
            .expect_err("rpm-managed must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("repair"),
            "rpm-managed refusal points at repair: {}",
            err.reason()
        );
    }

    /// Adoption is system-scope; user mode is refused.
    #[test]
    fn adopt_refuses_in_user_mode() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::User, false);
        let err = adopt_with_query("copilot-shell", None, &c, &FakeQuery::default())
            .expect_err("user mode must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("system"),
            "user-mode refusal mentions system scope: {}",
            err.reason()
        );
    }

    /// No installed RPM under the name: adopt does not install, points at install.
    #[test]
    fn adopt_refuses_absent_package() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let err = adopt_with_query("copilot-shell", None, &c, &FakeQuery::default())
            .expect_err("absent package must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("install copilot-shell"),
            "absent refusal points at the install command: {}",
            err.reason()
        );
    }

    /// Multiple provider packages cannot be adopted unambiguously.
    #[test]
    fn adopt_refuses_ambiguous_candidates() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let q = FakeQuery {
            provides: vec![(
                "anolisa-component(copilot-shell)".to_string(),
                vec!["pkg-a".to_string(), "pkg-b".to_string()],
            )],
            ..Default::default()
        };
        let err =
            adopt_with_query("copilot-shell", None, &c, &q).expect_err("ambiguous must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("--package"),
            "ambiguous refusal points at --package: {}",
            err.reason()
        );
    }

    /// A same-name multi-version rpmdb is refused rather than adopted blindly.
    #[test]
    fn adopt_refuses_multi_version() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"),
            )],
            multi_version: vec!["copilot-shell".to_string()],
            ..Default::default()
        };
        let err = adopt_with_query("copilot-shell", None, &c, &q)
            .expect_err("multi-version must be refused");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("multiple installed versions"));
    }

    /// `--dry-run` previews without writing any state.
    #[test]
    fn adopt_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.2.0", Some("1.al8"), "x86_64"),
            )],
            ..Default::default()
        };
        adopt_with_query("copilot-shell", None, &c, &q).expect("dry-run ok");
        let layout = common::resolve_layout(&c);
        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "dry-run must not write state",
        );
    }

    /// `--package` pins the RPM name, bypassing the candidate chain.
    #[test]
    fn adopt_with_package_override_adopts_named() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(&c, Vec::new());
        let q = FakeQuery {
            installed: vec![(
                "custom-pkg".to_string(),
                pkg_info("custom-pkg", "3.0.0", Some("1"), "x86_64"),
            )],
            ..Default::default()
        };
        adopt_with_query("copilot-shell", Some("custom-pkg"), &c, &q).expect("adopt ok");
        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("object")
            .clone();
        assert_eq!(
            obj.rpm_metadata.map(|m| m.package_name).as_deref(),
            Some("custom-pkg"),
            "the pinned package is recorded",
        );
    }

    /// `AdoptArgs` parses the positional component and the optional `--package`.
    #[test]
    fn adopt_parses_positional_and_package_flag() {
        use clap::Parser;
        let args = AdoptArgs::try_parse_from(["adopt", "copilot-shell", "--package", "pkg-x"])
            .expect("parse");
        assert_eq!(args.component, "copilot-shell");
        assert_eq!(args.package.as_deref(), Some("pkg-x"));

        let bare = AdoptArgs::try_parse_from(["adopt", "copilot-shell"]).expect("parse");
        assert_eq!(bare.package, None);
    }
}
