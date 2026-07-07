//! `anolisa install` — install a component through a configured backend.
//!
//! `install` takes a component noun and resolves it through the configured
//! backend. The resolution chain — repo.toml loading, backend selection
//! (`--backend` > `default_backend`), base_url variable substitution, and
//! package-name mapping (`--package` > `package_map` > scope > component
//! name) — feeds the **raw** backend executor: fetch the distribution index
//! from the raw repository root, resolve an artifact, then execute by
//! downloading it with mandatory sha256 verification, loading the install
//! contract, installing the declared files, and recording state plus a
//! central-log audit entry.
//!
//! The `rpm` backend supports two actions. **Adopt** (issue #958): in system
//! mode, when a component is already present as a system RPM, `install`
//! records it as `rpm-observed` state without downloading or running
//! `dnf install`. **Delegated install** (issue #959): in system mode, when the
//! component is *not* yet present, `install` delegates the file transaction to
//! `dnf install` and records it as `rpm-managed` state (ANOLISA owns the
//! removal). The backend decision is two-layered — pick a backend name
//! (`--backend` > existing state > system RPM presence > `default_backend`),
//! then pick an action by `(backend, rpmdb hit, install mode)`. `npm` remains
//! NOT_IMPLEMENTED.
//!
//! Deliberately out of scope for this milestone: execution-policy gating and
//! health checks.

use clap::Parser;

use crate::context::CliContext;
use crate::response::CliError;

mod rpm;
pub(crate) use rpm::*;

mod dispatch;
pub(crate) use dispatch::*;

mod batch;
pub(crate) use batch::*;

mod raw;
pub(crate) use raw::*;

mod render;
pub(crate) use render::*;
mod io_util;
pub(crate) use io_util::*;

#[cfg(test)]
mod tests;

const COMMAND: &str = "install";
const ANOLISA_RPM_REPO_ID: &str = "anolisa-configured";

#[derive(Debug, Parser)]
// `--version` here means the *component* version (the `cargo install`
// convention), so the auto-generated CLI-version flag must be disabled
// to free the name. `anolisa --version` still works at the top level.
#[command(disable_version_flag = true)]
#[command(group(
    clap::ArgGroup::new("target")
        .required(true)
        .args(["component", "all"]),
))]
pub struct InstallArgs {
    /// Component name to install
    #[arg(value_name = "COMPONENT")]
    pub component: Option<String>,
    /// Install every component in the component index (mutually exclusive with COMPONENT)
    #[arg(long, conflicts_with_all = ["component", "version", "package"])]
    pub all: bool,
    /// With --all, stop on the first failure instead of continuing
    #[arg(long, requires = "all")]
    pub fail_fast: bool,
    /// Install a specific version instead of the latest in the channel
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,
    /// Backend override (raw | rpm | npm); defaults to repo.toml default_backend
    #[arg(long, value_name = "BACKEND")]
    pub backend: Option<String>,
    /// One-off base_url override for the selected backend
    #[arg(long, value_name = "URL")]
    pub repo: Option<String>,
    /// Override the backend-native package name for the component
    #[arg(long, value_name = "NAME")]
    pub package: Option<String>,
}

mod types;
// Re-export shared types for external consumers (update.rs, adopt.rs, etc.)
// and for tests accessing via `super::*`.
pub(crate) use types::*;

mod provision;
pub(crate) use provision::*;

pub fn handle(args: InstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    if args.fail_fast && !args.all {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: "--fail-fast is only meaningful with --all".to_string(),
        });
    }
    if args.all {
        return handle_all(args, ctx);
    }
    // clap ArgGroup guarantees at least one of `component` / `--all`; with
    // `--all` ruled out above, `component` is necessarily Some.
    let component = args
        .component
        .clone()
        .expect("clap ArgGroup ensures component is set when --all is absent");
    handle_one(component, args, ctx).map(|_| ())
}

#[cfg(test)]
mod unit_tests {
    use super::tests::*;
    use super::*;
    use clap::Parser;

    #[test]
    fn install_cli_rejects_multiple_components() {
        let err = InstallArgs::try_parse_from(["install", "agentsight", "tokenless"])
            .expect_err("must reject extra positional arguments");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn install_all_and_component_are_mutually_exclusive() {
        let err = InstallArgs::try_parse_from(["install", "--all", "tokenless"])
            .expect_err("must reject --all with positional");
        assert!(
            err.kind() == clap::error::ErrorKind::ArgumentConflict
                || err.to_string().contains("cannot be used with")
        );
    }

    #[test]
    fn install_all_conflicts_with_package() {
        let err = InstallArgs::try_parse_from(["install", "--all", "--package", "foo"])
            .expect_err("must reject --all with --package");
        assert!(
            err.kind() == clap::error::ErrorKind::ArgumentConflict
                || err.to_string().contains("cannot be used with")
        );
    }

    #[test]
    fn install_all_conflicts_with_version() {
        let err = InstallArgs::try_parse_from(["install", "--all", "--version", "1.0.0"])
            .expect_err("must reject --all with --version");
        assert!(
            err.kind() == clap::error::ErrorKind::ArgumentConflict
                || err.to_string().contains("cannot be used with")
        );
    }

    #[test]
    fn install_fail_fast_without_all_is_rejected() {
        // clap still parses it (ArgGroup + requires limitation), but
        // handle() now rejects at runtime.
        let a = InstallArgs::try_parse_from(["install", "tokenless", "--fail-fast"])
            .expect("clap allows this parse");
        assert!(!a.all);
        assert!(a.fail_fast);

        let ctx = ctx_with_prefix(false, None);
        let err = handle(a, &ctx).expect_err("handle should reject --fail-fast without --all");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
    }

    #[test]
    fn install_all_parses_successfully() {
        let a = InstallArgs::try_parse_from(["install", "--all"]).expect("should parse");
        assert!(a.all);
        assert!(a.component.is_none());
    }

    #[test]
    fn install_all_with_fail_fast_parses_successfully() {
        let a =
            InstallArgs::try_parse_from(["install", "--all", "--fail-fast"]).expect("should parse");
        assert!(a.all);
        assert!(a.fail_fast);
    }
}
