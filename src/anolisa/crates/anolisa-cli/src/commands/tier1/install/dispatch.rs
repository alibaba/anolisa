//! Command dispatch: backend selection (Layer 1/2), raw/RPM routing, and
//! compatibility checks for the `install` command.

use anolisa_core::state::{InstalledObject, InstalledState, ObjectKind};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::privilege;
use anolisa_platform::rpm_query::RpmPackageQuery;
use anolisa_platform::rpm_repo::DnfRepoSource;
use anolisa_platform::rpm_transaction::RpmTransaction;

use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::commands::tier1::rpm_install;
use crate::context::{CliContext, InstallMode};
use crate::repo_config::{
    BackendConfig, HostVars, RepoConfig, RepoConfigError, normalize_override_url,
};
use crate::resolution::{
    BackendKind, ComponentIndex, ComponentResolver, ResolutionSet, ResolutionUse, ResolveOptions,
    load_optional_component_index,
};
use crate::response::CliError;

use super::InstallArgs;
use super::raw::{build_install_preview, execute_raw, prepare_raw_execution, resolve_raw};
use super::render::render_plan;
use super::render::repo_config_err;
use super::rpm::{RpmExec, RpmSituation, probe_rpm_situation, route_rpm_adopt};
use super::types::*;

use super::{ANOLISA_RPM_REPO_ID, COMMAND};
pub(crate) fn handle_one(
    component: String,
    args: InstallArgs,
    ctx: &CliContext,
) -> Result<InstallOutcome, CliError> {
    let layout = common::resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config = common::load_repo_config(ctx, &layout, COMMAND, RepoPersistPolicy::Require)?;
    let identity = load_install_identity(component, ctx)?;
    let rpm_repo = if rpm_repo_required(
        &identity.component,
        &args,
        &identity.installed,
        &repo_config,
    ) {
        configured_rpm_repo_source(&repo_config, &env)?
    } else {
        None
    };

    // Production uses the real rpm/dnf-backed query and transaction; tests
    // inject fakes via `handle_one_with_exec`. The real backends receive the
    // repo.toml RPM source so availability probes and install transactions do
    // not silently fall back to the host's enabled system repos.
    let query = match rpm_repo.clone() {
        Some(repo) => RpmPackageQuery::system_with_repo(repo),
        None => RpmPackageQuery::system(),
    };
    let txn = match rpm_repo {
        Some(repo) => RpmTransaction::system_with_repo(repo),
        None => RpmTransaction::system(),
    };
    let exec = RpmExec::new(&query, &txn, privilege::is_root());
    handle_one_with_config(identity, args, ctx, &exec, layout, env, repo_config)
}

/// Core of [`handle_one`] with the RPM execution dependencies injected, so
/// tests can drive the adopt and delegated-install paths without a live
/// rpmdb/dnf or real privileges.
// pub(crate): driven by the cross-command MVP lifecycle test (#963).
#[cfg(test)]
pub(crate) fn handle_one_with_exec(
    component: String,
    args: InstallArgs,
    ctx: &CliContext,
    exec: &RpmExec,
) -> Result<InstallOutcome, CliError> {
    let layout = common::resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config = common::load_repo_config(ctx, &layout, COMMAND, RepoPersistPolicy::Require)?;
    let identity = load_install_identity(component, ctx)?;
    handle_one_with_config(identity, args, ctx, exec, layout, env, repo_config)
}

fn handle_one_with_config(
    identity: InstallIdentity,
    args: InstallArgs,
    ctx: &CliContext,
    exec: &RpmExec,
    layout: FsLayout,
    env: anolisa_env::EnvFacts,
    repo_config: RepoConfig,
) -> Result<InstallOutcome, CliError> {
    let InstallIdentity {
        requested_component,
        component,
        installed,
    } = identity;
    let command = format!("install {requested_component}");
    let mut initial_claims = vec![requested_component.as_str(), component.as_str()];
    if let Some(package) = args.package.as_deref() {
        initial_claims.push(package);
    }
    rpm_install::reject_pending_claim(&layout, &installed, &initial_claims, &command)?;

    let mut rpm_component_index: Option<ComponentIndex> = None;

    // ── Layer 1: pick the backend name + its source (§4). ──
    //
    // Priority: explicit --backend > existing state > system RPM presence
    // (system mode only) > default_backend. The system-RPM probe runs only
    // when nothing earlier decided AND we are in system mode, so user mode,
    // an explicit --backend, and existing-state hits never shell out to
    // rpm/dnf. The default/auto-detect system path DOES probe rpm; when that
    // probe cannot run because rpm/dnf is absent it fail-fasts with a
    // `--backend raw` hint (§7.1) rather than silently installing raw over a
    // possibly-unobserved system RPM.
    let mut adopt_situation: Option<RpmSituation> = None;
    let (backend_name, source): (String, BackendSource) =
        if let Some(explicit) = args.backend.as_deref() {
            if let Some(warning) = RepoConfig::backend_name_deprecation_warning(explicit) {
                eprintln!("warning: {warning}");
            }
            (
                RepoConfig::canonical_backend_name(explicit).to_string(),
                BackendSource::Explicit,
            )
        } else if let Some(label) = installed
            .find_object(ObjectKind::Component, &component)
            .and_then(installed_backend_label)
        {
            // Provenance is sticky: a re-`install` of an adopted rpm-observed
            // component lands on `rpm` here and is routed to adopt-refresh by
            // layer 2, rather than being rejected by the raw trunk.
            (label.to_string(), BackendSource::ExistingState)
        } else if ctx.install_mode == InstallMode::System {
            rpm_component_index = load_optional_component_index(&layout, &env, &repo_config);
            let situation = probe_rpm_situation(
                &component,
                args.package.as_deref(),
                repo_config.backends.get("rpm"),
                rpm_component_index.as_ref(),
                ResolutionUse::Install,
                exec.query,
                &command,
            )?;
            if matches!(
                situation,
                RpmSituation::Absent { .. } | RpmSituation::NotAnolisaComponent
            ) {
                // Absent or not an ANOLISA RPM component + no `--backend`: fall
                // through to the default backend. If that is `rpm`, layer 2
                // re-probes and either delegates a `dnf install` or rejects the
                // non-component; if it is `raw`, the raw trunk installs. Either
                // way the probe's `adopt_situation` is dropped — there is no
                // installed system RPM to adopt.
                (repo_config.default_backend.clone(), BackendSource::Default)
            } else {
                adopt_situation = Some(situation);
                ("rpm".to_string(), BackendSource::SystemRpm)
            }
        } else {
            (repo_config.default_backend.clone(), BackendSource::Default)
        };

    // ── Layer 2: pick the action by (backend, rpmdb, mode) (§7.1). ──
    if backend_name == "rpm" {
        if rpm_component_index.is_none() {
            rpm_component_index = load_optional_component_index(&layout, &env, &repo_config);
        }
        return route_rpm_adopt(
            &component,
            &args,
            ctx,
            &command,
            &layout,
            &repo_config,
            &installed,
            source,
            adopt_situation,
            rpm_component_index.as_ref(),
            exec,
        );
    }

    handle_raw_install(
        component,
        args,
        ctx,
        &command,
        &layout,
        &env,
        &repo_config,
        &installed,
        &backend_name,
    )
}

/// Load the routing snapshot and resolve package aliases before constructing
/// backend clients, so repository selection and execution use one identity.
struct InstallIdentity {
    requested_component: String,
    component: String,
    installed: InstalledState,
}

fn load_install_identity(
    requested_component: String,
    ctx: &CliContext,
) -> Result<InstallIdentity, CliError> {
    let installed = common::load_installed_state(ctx, COMMAND)?;
    let component = common::lookup_component_name(&requested_component, &installed, ctx, COMMAND);
    Ok(InstallIdentity {
        requested_component,
        component,
        installed,
    })
}

pub(crate) fn configured_rpm_repo_source(
    repo_config: &RepoConfig,
    env: &anolisa_env::EnvFacts,
) -> Result<Option<DnfRepoSource>, CliError> {
    let Some(backend) = repo_config.backends.get("rpm") else {
        return Ok(None);
    };
    let host = HostVars {
        os: env.os.clone(),
        arch: env.arch.clone(),
    };
    let base_url = repo_config
        .resolved_base_url("rpm", backend, &host)
        .map_err(|err| repo_config_err(err, true))?;
    Ok(Some(DnfRepoSource::new(
        ANOLISA_RPM_REPO_ID,
        base_url,
        backend.gpgcheck,
    )))
}

pub(crate) fn require_configured_rpm_backend(
    repo_config: &RepoConfig,
    command: &str,
) -> Result<(), CliError> {
    if repo_config.backends.contains_key("rpm") {
        Ok(())
    } else {
        Err(repo_config_err(
            RepoConfigError::BackendNotConfigured {
                name: "rpm".to_string(),
            },
            true,
        )
        .with_command(command))
    }
}

pub(crate) fn rpm_repo_required(
    component: &str,
    args: &InstallArgs,
    installed: &InstalledState,
    repo_config: &RepoConfig,
) -> bool {
    if args
        .backend
        .as_deref()
        .map(RepoConfig::canonical_backend_name)
        == Some("rpm")
    {
        return true;
    }
    if args.backend.is_none() && repo_config.default_backend == "rpm" {
        return true;
    }
    installed
        .find_object(ObjectKind::Component, component)
        .and_then(installed_backend_label)
        == Some("rpm")
}

/// Existing raw-backend trunk: repo.toml → base_url → package → resolve →
/// (dry-run preview | download + execute). Backends other than `raw` that
/// reach here have no executor yet and return a not-implemented hint.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_raw_install(
    component: String,
    args: InstallArgs,
    ctx: &CliContext,
    command: &str,
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
    installed: &InstalledState,
    backend_name: &str,
) -> Result<InstallOutcome, CliError> {
    // Re-resolve through `select_backend` so the configured `[backends.<name>]`
    // table (base_url, package_map, scope) is in hand. This stays on the raw
    // path only; the rpm/adopt branch above never calls it (no table required).
    let (backend_name, backend) = repo_config
        .select_backend(Some(backend_name))
        .map_err(|err| repo_config_err(err, true))?;

    ensure_component_backend_compatible(installed, &component, backend_name, command)?;

    // Backend gate: only raw can execute today. The selection above already
    // validated the name/configuration, so this is purely "executor missing".
    if backend_name != "raw" {
        return Err(CliError::not_implemented_with_hint(
            format!("install --backend {backend_name}"),
            format!(
                "the '{backend_name}' backend is configured but its executor is not implemented yet — only 'raw' can install today",
            ),
        ));
    }

    let mut warnings: Vec<String> = Vec::new();
    let base_url = match args.repo.as_deref() {
        Some(override_url) => {
            let normalized =
                normalize_override_url(override_url).map_err(|err| repo_config_err(err, true))?;
            if normalized.starts_with("http://") {
                warnings.push(format!(
                    "--repo uses plaintext http ({normalized}) — artifacts are still sha256-verified on the raw backend, but the index itself is unauthenticated",
                ));
            }
            normalized
        }
        None => {
            let host = HostVars {
                os: env.os.clone(),
                arch: env.arch.clone(),
            };
            repo_config
                .resolved_base_url(backend_name, backend, &host)
                // Variable errors are fixed by editing [vars] in repo.toml.
                .map_err(|err| repo_config_err(err, true))?
        }
    };
    let (component, package) = resolve_raw_identity(
        layout,
        env,
        repo_config,
        backend,
        component,
        args.package.as_deref(),
    );

    let resolved = resolve_raw(
        ctx,
        layout,
        env,
        ResolveInputs {
            component,
            package,
            backend: backend_name.to_string(),
            base_url,
            version: args.version.as_deref(),
            warnings,
        },
    )?;

    rpm_install::reject_pending_claim(
        layout,
        installed,
        &[resolved.component.as_str(), resolved.package.as_str()],
        command,
    )?;

    if ctx.dry_run {
        let preview = build_install_preview(ctx, layout, installed, resolved)?;
        render_plan(ctx, &preview)?;
        return Ok(InstallOutcome::Installed);
    }

    let prepared = prepare_raw_execution(ctx, layout, resolved)?;
    execute_raw(ctx, layout, command, prepared)?;
    Ok(InstallOutcome::Installed)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_raw_identity(
    layout: &FsLayout,
    env: &anolisa_env::EnvFacts,
    repo_config: &RepoConfig,
    backend: &BackendConfig,
    component: String,
    cli_override: Option<&str>,
) -> (String, String) {
    if cli_override.is_some() || backend.package_map.contains_key(&component) {
        let package = repo_config.package_name(backend, &component, cli_override);
        return (component, package);
    }

    let component_index = load_optional_component_index(layout, env, repo_config);
    let resolver = ComponentResolver::new(component_index.as_ref(), None, None);
    match resolver.resolve(
        &component,
        BackendKind::Raw,
        ResolutionUse::Install,
        ResolveOptions::default(),
    ) {
        Ok(ResolutionSet::Unique(target)) => (target.component, target.package),
        _ => {
            let package = repo_config.package_name(backend, &component, cli_override);
            (component, package)
        }
    }
}

pub(crate) fn ensure_component_backend_compatible(
    state: &InstalledState,
    component: &str,
    requested_backend: &str,
    command: &str,
) -> Result<(), CliError> {
    let Some(obj) = state.find_object(ObjectKind::Component, component) else {
        return Ok(());
    };

    match installed_backend_label(obj) {
        Some(installed_backend) if installed_backend == requested_backend => Ok(()),
        Some(installed_backend) => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is already installed via backend '{installed_backend}'; reinstalling it via backend '{requested_backend}' is not allowed — uninstall it first or use backend '{installed_backend}'",
            ),
        }),
        None => Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is already installed but its install backend is unknown; uninstall it before installing via backend '{requested_backend}'",
            ),
        }),
    }
}

pub(crate) fn installed_backend_label(obj: &InstalledObject) -> Option<&str> {
    obj.install_backend
        .as_deref()
        .map(RepoConfig::canonical_backend_name)
        .or_else(|| infer_backend_from_distribution_source(obj.distribution_source.as_deref()))
}

pub(crate) fn infer_backend_from_distribution_source(source: Option<&str>) -> Option<&'static str> {
    let source = source?;
    if source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("file://")
    {
        Some("raw")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::handle;
    use super::super::tests::*;
    use super::*;
    use crate::repo_config::RepoConfig;
    use anolisa_platform::fs_layout::FsLayout;
    use tempfile::tempdir;

    #[test]
    fn install_unknown_component_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("no-such-component");
        a.repo = Some(write_empty_repo(&tmp.path().join("repo")));

        let err =
            handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("no-such-component"));
    }

    #[test]
    fn install_unsupported_mode_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("agentsight");
        a.repo = Some(write_local_repo_component(
            &tmp.path().join("repo"),
            "agentsight",
            "0.2.0",
            &["user"],
        ));

        let err =
            handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason().contains("install mode is not supported"),
            "got: {}",
            err.reason()
        );
    }

    #[test]
    fn install_manifest_mode_mismatch_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("sys");
        let mut a = args("agentsight");
        a.repo = Some(write_local_repo_component_with_modes(
            &tmp.path().join("repo"),
            "agentsight",
            "0.2.0",
            &["system"],
            &["user"],
        ));

        let err =
            handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(
            err.reason()
                .contains("inconsistent with the distribution index")
                && err.reason().contains("system-mode support"),
            "got: {}",
            err.reason()
        );
    }

    #[test]
    fn install_unconfigured_backend_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.backend = Some("npm".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("npm"), "got: {}", err.reason());
        assert!(
            err.reason().contains("repo.toml"),
            "reason must point at repo.toml: {}",
            err.reason()
        );
    }

    #[test]
    fn install_unknown_backend_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.backend = Some("pip".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("pip"));
    }

    #[test]
    fn install_configured_npm_backend_is_not_implemented() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix.clone()));
        std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
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

        let mut a = args("agentsight");
        a.backend = Some("npm".to_string());
        let err = handle(a, &ctx_with_prefix(false, Some(prefix))).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(err.reason().contains("npm"), "got: {}", err.reason());
    }

    #[test]
    fn install_invalid_repo_override_is_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let mut a = args("agentsight");
        a.repo = Some("ftp://example.com/repo".to_string());
        let err = handle_with_fake_rpm(a, &ctx_with_prefix(false, Some(tmp.path().to_path_buf())))
            .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("ftp"), "got: {}", err.reason());
    }

    #[test]
    fn configured_rpm_repo_source_uses_repo_toml_backend() {
        let repo = RepoConfig::from_toml_str(
            r#"schema_version = 1
default_backend = "rpm"
[vars]
releasever = "4"
[backends.rpm]
base_url = "http://repo.example/alinux/$releasever/agentic-os/$basearch/os/"
insecure = true
gpgcheck = false
"#,
        )
        .expect("parse repo");
        let source = configured_rpm_repo_source(&repo, &linux_env())
            .expect("resolve rpm repo")
            .expect("rpm repo exists");
        assert_eq!(source.id(), ANOLISA_RPM_REPO_ID);
        assert_eq!(
            source.base_url(),
            "http://repo.example/alinux/4/agentic-os/x86_64/os"
        );
        assert_eq!(source.gpgcheck(), Some(false));
    }

    #[test]
    fn raw_default_does_not_require_rpm_repo_resolution() {
        let (_tmp, ctx) = system_ctx_with_raw_repo(false);
        let repo = RepoConfig::from_toml_str(
            r#"schema_version = 1
default_backend = "raw"
[backends.raw]
base_url = "https://example.com/anolisa"
[backends.rpm]
base_url = "https://repo.example/alinux/$releasever/agentic-os/$basearch/os/"
"#,
        )
        .expect("parse repo without resolving rpm variables");
        let a = args("copilot-shell");
        let installed = common::load_installed_state(&ctx, COMMAND).expect("load state");
        assert!(
            !rpm_repo_required("copilot-shell", &a, &installed, &repo),
            "raw default with no rpm state must not resolve the rpm backend"
        );
    }

    #[test]
    fn managed_rpm_package_alias_requires_configured_repo() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().join("install-root");
        let layout = FsLayout::system(Some(prefix.clone()));
        let raw_root = tmp.path().join("raw-repo");
        std::fs::create_dir_all(raw_root.join("v1")).expect("create raw repo");
        std::fs::create_dir_all(&layout.etc_dir).expect("create etc dir");
        std::fs::create_dir_all(&layout.state_dir).expect("create state dir");
        std::fs::write(
            raw_root.join("v1/components.toml"),
            r#"schema_version = 1

[[components]]
name = "cosh"

[[components.backends]]
kind = "rpm"
package = "copilot-shell"

[[components.aliases]]
kind = "rpm-package"
name = "copilot-shell"
"#,
        )
        .expect("write component index");
        std::fs::write(
            layout.etc_dir.join("repo.toml"),
            format!(
                r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "file://{}"

[backends.rpm]
base_url = "https://repo.example/anolisa"
"#,
                raw_root.display()
            ),
        )
        .expect("write repo config");

        std::fs::write(
            layout.state_dir.join("installed.toml"),
            format!(
                r#"schema_version = 4
updated_at = "2026-07-14T00:00:00Z"
install_mode = "system"
prefix = "{}"
anolisa_version = "test"

[[objects]]
kind = "component"
name = "cosh"
version = "2.2.0-1.al8"
status = "installed"
install_backend = "rpm"
ownership = "rpm_managed"
installed_at = "2026-07-14T00:00:00Z"
"#,
                layout.prefix.display()
            ),
        )
        .expect("write state");

        let ctx = ctx_with_prefix(false, Some(prefix));
        let repo = RepoConfig::load(&layout, false).expect("load repo").config;
        let identity = load_install_identity("copilot-shell".to_string(), &ctx)
            .expect("resolve package alias");

        assert_eq!(identity.component, "cosh");
        assert!(
            rpm_repo_required(
                &identity.component,
                &args("copilot-shell"),
                &identity.installed,
                &repo
            ),
            "an existing rpm-managed component must select the configured RPM repo even when addressed by package alias"
        );
    }
}
