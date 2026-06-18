//! `anolisa repair <component>` — reconcile ANOLISA state with rpmdb reality.
//!
//! When a user runs `dnf update`/`downgrade` outside ANOLISA, the recorded EVR
//! drifts from rpmdb (surfaced as `drifted` by `anolisa status`). `repair`
//! reads rpmdb, confirms the package identity is still valid, and refreshes the
//! ANOLISA state record (version, EVR, arch, source repo). It runs **no**
//! dnf/rpm transaction and never switches backend — only rpmdb reads plus a
//! state write.
//!
//! A package that has been `rpm -e`'d cannot be repaired: there is nothing to
//! refresh from, so `repair` refuses and points at `anolisa forget`. Raw
//! components have no rpmdb to reconcile against and are not handled yet.

use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::lock::InstallLock;
use anolisa_core::state::{ObjectKind, OperationRecord, Ownership, RpmMetadata};
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError};
use anolisa_platform::rpm_query::RpmPackageQuery;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::tier1::install::rpm_package_candidates;
use crate::context::CliContext;
use crate::repo_config::RepoConfig;
use crate::response::{CliError, render_json};

/// Command label for JSON envelopes and error routing.
const COMMAND: &str = "repair";

/// Arguments for `anolisa repair <component>`.
#[derive(Debug, Parser)]
pub struct RepairArgs {
    /// Component whose ANOLISA state should be refreshed from rpmdb
    #[arg(value_name = "COMPONENT")]
    pub component: String,
}

/// Wire shape for a `repair <component>` result (`--json`) and its dry-run
/// preview.
#[derive(Serialize)]
struct RepairPayload {
    component: String,
    package: String,
    /// Always `rpm`: repair never switches backend.
    backend: &'static str,
    /// `rpm-observed` or `rpm-managed`; preserved across the repair.
    ownership: &'static str,
    install_mode: String,
    /// EVR ANOLISA had recorded; `None` for a legacy row with no metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    from_version: Option<String>,
    /// EVR read back from rpmdb (the value state is reconciled to).
    to_version: String,
    /// Whether state was actually written (false on dry-run).
    refreshed: bool,
    /// Whether the rpmdb EVR differed from what ANOLISA had recorded.
    changed: bool,
    dry_run: bool,
    /// `None` on dry-run (nothing recorded).
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    warnings: Vec<String>,
}

/// Dispatch `repair <component>`: build the real rpm-backed query and reconcile.
///
/// # Errors
///
/// Returns [`CliError`] when the component is absent, raw-backed (unsupported),
/// the package is gone from rpmdb, the rpmdb read is ambiguous, or the state
/// write fails.
pub fn handle(args: RepairArgs, ctx: &CliContext) -> Result<(), CliError> {
    let query = RpmPackageQuery::system();
    repair_with_query(&args.component, ctx, &query)
}

/// Core of [`handle`] with the package query injected so tests drive the
/// reconcile path without a live rpmdb. Repair runs no dnf transaction, so only
/// a [`PackageQuery`] is required.
fn repair_with_query(
    target: &str,
    ctx: &CliContext,
    query: &dyn PackageQuery,
) -> Result<(), CliError> {
    let command = format!("repair {target}");
    let installed = common::load_installed_state(ctx, COMMAND)?;

    let obj = installed
        .find_object(ObjectKind::Component, target)
        .ok_or_else(|| CliError::InvalidArgument {
            command: command.clone(),
            reason: format!(
                "component '{target}' is not installed — nothing to repair (run `anolisa status` to see what is installed)"
            ),
        })?;

    let ownership = obj.effective_ownership();
    // Raw components have no rpmdb to reconcile against. Keep them on the same
    // not-implemented boundary the update path uses for raw.
    if !ownership.is_rpm() {
        return Err(CliError::not_implemented_with_hint(
            command,
            "raw component repair is not implemented yet; only RPM-backed components can be repaired today",
        ));
    }

    // Resolve the package to reconcile against. A recorded package name is the
    // identity to confirm; when absent (a legacy row with no rpm_metadata), fall
    // back to the adopt candidate chain so repair can backfill the metadata the
    // update path refuses to run without.
    let package = resolve_repair_package(target, obj.rpm_metadata.as_ref(), ctx, query, &command)?;
    let recorded_evr = obj.rpm_metadata.as_ref().and_then(|m| m.evr.clone());
    let ownership_label = ownership.label();

    // rpmdb query — the truth repair reconciles to.
    let info = match query.query_installed(&package) {
        Ok(Some(info)) => info,
        // rpm -e: nothing to refresh from. repair cannot conjure the package
        // back, so point at forget (or reinstall) rather than fabricating state.
        Ok(None) => {
            return Err(CliError::Runtime {
                command,
                reason: format!(
                    "RPM package '{package}' for component '{target}' is recorded in ANOLISA state but is not present in rpmdb — it may have been removed with `rpm -e`; run `anolisa forget {target}` to drop the stale state, or reinstall"
                ),
            });
        }
        // rpm could not be reduced to a single installed version (duplicates, a
        // malformed `--qf` row, or none on a zero exit): an ambiguous reconcile
        // target. Refuse with the backend's own detail rather than asserting one
        // specific cause.
        Err(PackageQueryError::UnexpectedOutput { detail, .. }) => {
            return Err(CliError::Runtime {
                command,
                reason: format!(
                    "rpm returned unexpected output for package '{package}': {detail}; refusing to refresh until it resolves to a single installed version"
                ),
            });
        }
        Err(PackageQueryError::CommandMissing { .. }) => {
            return Err(rpm_tooling_missing_error(&command));
        }
        Err(err) => return Err(rpm_query_err(err, &command)),
    };

    let to_evr = info.version.to_string();
    let changed = recorded_evr.as_deref() != Some(to_evr.as_str());

    // source_repo is supplementary metadata: a failed origin lookup degrades to
    // `None` with a warning and never fails the repair (mirrors adopt).
    let mut warnings: Vec<String> = Vec::new();
    let source_repo = match query.installed_origin(&package) {
        Ok(origin) => origin,
        Err(err) => {
            warnings.push(format!(
                "could not determine source repo for '{package}': {err}"
            ));
            None
        }
    };

    if ctx.dry_run {
        let payload = RepairPayload {
            component: target.to_string(),
            package,
            backend: "rpm",
            ownership: ownership_label,
            install_mode: ctx.install_mode.as_str().to_string(),
            from_version: recorded_evr,
            to_version: to_evr,
            refreshed: false,
            changed,
            dry_run: true,
            operation_id: None,
            warnings,
        };
        render_repair(ctx, &payload);
        return Ok(());
    }

    let operation_id = persist_repair(
        ctx,
        target,
        &package,
        ownership,
        &info,
        &to_evr,
        source_repo.as_deref(),
        &command,
        &warnings,
    )?;

    let payload = RepairPayload {
        component: target.to_string(),
        package,
        backend: "rpm",
        ownership: ownership_label,
        install_mode: ctx.install_mode.as_str().to_string(),
        from_version: recorded_evr,
        to_version: to_evr,
        refreshed: true,
        changed,
        dry_run: false,
        operation_id: Some(operation_id),
        warnings,
    };
    render_repair(ctx, &payload);
    Ok(())
}

/// Resolve the RPM package name `repair` should reconcile against.
///
/// A recorded, non-empty package name is the identity to confirm. When it is
/// absent — a legacy row written before `rpm_metadata` existed — fall back to
/// the adopt candidate chain ([`rpm_package_candidates`]) so repair can backfill
/// the metadata that `update` refuses to run without.
fn resolve_repair_package(
    component: &str,
    meta: Option<&RpmMetadata>,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    command: &str,
) -> Result<String, CliError> {
    if let Some(name) = meta
        .map(|m| m.package_name.as_str())
        .filter(|n| !n.is_empty())
    {
        return Ok(name.to_string());
    }

    // Legacy row with no recorded package name: resolve via the same candidate
    // chain adopt uses. repo.toml / catalog are best-effort inputs (mirrors
    // status::observed_record): a load failure just drops that precedence tier.
    let layout = common::resolve_layout(ctx);
    let repo_config = RepoConfig::load(&layout).ok();
    let rpm_backend = repo_config.as_ref().and_then(|c| c.backends.get("rpm"));
    let manifest = common::load_bundled_catalog(ctx, COMMAND)
        .ok()
        .and_then(|cat| cat.component(component).cloned());

    let candidates =
        match rpm_package_candidates(None, manifest.as_ref(), rpm_backend, query, component) {
            Ok(candidates) => candidates,
            Err(PackageQueryError::CommandMissing { .. }) => {
                return Err(rpm_tooling_missing_error(command));
            }
            Err(err) => return Err(rpm_query_err(err, command)),
        };
    if candidates.len() >= 2 {
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!(
                "multiple candidate RPMs for component '{component}': {}; cannot repair unambiguously — reinstall to pin one, or fix the manifest/package_map mapping",
                candidates.join(", ")
            ),
        });
    }
    // `rpm_package_candidates` always backfills the default name, so this is the
    // single-candidate case.
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| CliError::Runtime {
            command: command.to_string(),
            reason: format!("could not resolve an RPM package name for component '{component}'"),
        })
}

/// Persist the reconciled RPM metadata under the install lock, then append an
/// audit record. Ownership and `install_backend` are left untouched — repair
/// never switches backend. Returns the operation id.
#[allow(clippy::too_many_arguments)]
fn persist_repair(
    ctx: &CliContext,
    component: &str,
    package: &str,
    ownership: Ownership,
    info: &PackageInfo,
    to_evr: &str,
    source_repo: Option<&str>,
    command: &str,
    warnings: &[String],
) -> Result<String, CliError> {
    let layout = common::resolve_layout(ctx);
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state = common::load_installed_state(ctx, command)?;

    // Re-validate under the lock: the component must still exist and still be
    // RPM-owned. A concurrent uninstall/forget or backend change between the
    // pre-lock read and here must not be clobbered by a stale repair record.
    let obj = state
        .find_object_mut(ObjectKind::Component, component)
        .ok_or_else(|| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' disappeared from state during repair; no changes recorded"
            ),
        })?;
    if !obj.effective_ownership().is_rpm() {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is no longer an RPM component in state; refusing to record an RPM repair"
            ),
        });
    }
    // A recorded package name must be unchanged under the lock: `query_installed`
    // ran against `package` (snapshotted before the lock), so a concurrent
    // re-point to a different RPM would graft this EVR onto the wrong package.
    // An empty/absent prior name is a legacy backfill and allowed.
    if let Some(recorded) = obj
        .rpm_metadata
        .as_ref()
        .map(|m| m.package_name.as_str())
        .filter(|n| !n.is_empty())
        && recorded != package
    {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' RPM package identity changed during repair (expected package '{package}'); refusing to record an EVR against a different package — run `anolisa status {component}`"
            ),
        });
    }

    let now = now_iso8601();
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-repair-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );

    // Reconcile the recorded version to rpmdb truth. ownership / install_backend
    // / status are deliberately untouched: repair refreshes facts, it does not
    // re-decide the lifecycle.
    obj.version = to_evr.to_string();
    obj.last_operation_id = Some(operation_id.clone());
    match obj.rpm_metadata.as_mut() {
        Some(meta) => {
            // Backfill the name for a legacy row; a no-op when already set.
            meta.package_name = package.to_string();
            meta.evr = Some(to_evr.to_string());
            meta.arch = Some(info.arch.clone());
            // Only overwrite source_repo when freshly determined — a failed
            // origin lookup must not erase a previously-good value.
            if let Some(repo) = source_repo {
                meta.source_repo = Some(repo.to_string());
            }
        }
        None => {
            obj.rpm_metadata = Some(RpmMetadata {
                package_name: package.to_string(),
                evr: Some(to_evr.to_string()),
                arch: Some(info.arch.clone()),
                source_repo: source_repo.map(str::to_string),
            });
        }
    }

    state.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: now.clone(),
        finished_at: Some(now.clone()),
    });

    let state_path = layout.state_dir.join("installed.toml");
    state.save(&state_path).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to save state: {err}"),
    })?;

    // Audit log is best-effort: the repair already persisted, so a log failure
    // downgrades to a warning instead of unwinding.
    let log = CentralLog::open(layout.central_log.clone());
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.clone()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some(component.to_string()),
        severity: Severity::Info,
        message: format!(
            "refreshed ANOLISA state for component {component} to {to_evr} from rpmdb package {package} ({ownership_label})",
            ownership_label = ownership.label(),
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: now.clone(),
        finished_at: Some(now),
        status: Some(LogStatus::Ok),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: warnings.to_vec(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    Ok(operation_id)
}

/// Human/JSON renderer for a repair result.
fn render_repair(ctx: &CliContext, payload: &RepairPayload) {
    if ctx.json {
        // Errors here are unreachable for a plain Serialize struct; ignore the
        // Result so an (already-persisted) repair is not reported as failed.
        let _ = render_json(COMMAND, payload);
        return;
    }
    if ctx.quiet {
        return;
    }
    let color = Palette::new(ctx.no_color);
    let from = payload.from_version.as_deref().unwrap_or("(none recorded)");
    if payload.dry_run {
        println!(
            "{} {} {} {}",
            color.command("repair"),
            payload.component,
            color.muted(format!("({}, {})", payload.ownership, payload.package)),
            color.muted("(dry-run — nothing written)"),
        );
        if payload.changed {
            println!(
                "{} {} → {}",
                color.label("would refresh:"),
                from,
                payload.to_version
            );
        } else {
            println!(
                "{} state already matches rpmdb ({})",
                color.label("would refresh:"),
                payload.to_version,
            );
        }
    } else if payload.changed {
        println!(
            "{} {} {} → {}",
            color.ok("✓ repaired"),
            payload.component,
            from,
            payload.to_version,
        );
    } else {
        println!(
            "{} {} already matches rpmdb ({})",
            color.ok("✓"),
            payload.component,
            payload.to_version,
        );
    }
    // Remind the operator that an observed row is a pre-existing system RPM.
    if payload.ownership == "rpm-observed" {
        println!(
            "    {} {} is a system RPM observed by ANOLISA; dnf owns the file transaction",
            color.label("note:"),
            payload.package,
        );
    }
    render_warnings(&payload.warnings, &color);
}

/// Map a [`PackageQueryError`] onto a CLI runtime error (the benign
/// not-installed / multi-version branches are split off by the caller).
fn rpm_query_err(err: PackageQueryError, command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("rpm query failed: {err}"),
    }
}

/// Warn-and-exit error when `rpm`/`dnf` is absent: an RPM component cannot be
/// reconciled without the package manager.
fn rpm_tooling_missing_error(command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: "rpm/dnf not found: cannot reconcile an RPM-backed component without the package manager. Install rpm/dnf and retry".to_string(),
    }
}

/// Render any accumulated warnings to stderr, one per line.
fn render_warnings(warnings: &[String], color: &Palette) {
    for w in warnings {
        eprintln!("{} {w}", color.warn("warning:"));
    }
}

/// RFC3339 UTC timestamp, seconds precision (matches the install/update paths).
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus,
    };
    use anolisa_platform::pkg_query::PackageVersion;

    use crate::context::InstallMode;

    /// Configurable in-memory [`PackageQuery`] for the repair tests. Repair runs
    /// no transaction, so a query alone drives every path.
    struct FakeQuery {
        package: String,
        installed: Option<PackageInfo>,
        origin: Option<String>,
        multi_version: bool,
        command_missing: bool,
    }

    impl FakeQuery {
        fn new(package: &str, installed: Option<PackageInfo>) -> Self {
            Self {
                package: package.to_string(),
                installed,
                origin: None,
                multi_version: false,
                command_missing: false,
            }
        }
        fn with_origin(mut self, origin: &str) -> Self {
            self.origin = Some(origin.to_string());
            self
        }
        fn multi_version(mut self) -> Self {
            self.multi_version = true;
            self
        }
        fn command_missing(mut self) -> Self {
            self.command_missing = true;
            self
        }
    }

    impl PackageQuery for FakeQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if self.command_missing {
                return Err(PackageQueryError::CommandMissing {
                    command: "rpm".to_string(),
                });
            }
            if package != self.package {
                return Ok(None);
            }
            if self.multi_version {
                return Err(PackageQueryError::UnexpectedOutput {
                    command: "rpm".to_string(),
                    detail: "2 installed versions".to_string(),
                });
            }
            Ok(self.installed.clone())
        }
        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }
        fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
            if package == self.package {
                Ok(self.origin.clone())
            } else {
                Ok(None)
            }
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

    /// An RPM-backed component object (observed or managed).
    fn rpm_object(
        component: &str,
        package: &str,
        evr: &str,
        ownership: Ownership,
        status: ObjectStatus,
    ) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: evr.to_string(),
            status,
            manifest_digest: None,
            distribution_source: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(ownership),
            rpm_metadata: Some(RpmMetadata {
                package_name: package.to_string(),
                evr: Some(evr.to_string()),
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            }),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: !matches!(ownership, Ownership::RpmObserved),
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

    /// A raw-managed component object (no rpm metadata).
    fn raw_object(component: &str, version: &str) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: version.to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("https://example.com/x".to_string()),
            install_backend: Some("raw".to_string()),
            ownership: Some(Ownership::RawManaged),
            rpm_metadata: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        }
    }

    fn seed(ctx: &CliContext, obj: InstalledObject) {
        let layout = common::resolve_layout(ctx);
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let mut state = InstalledState {
            install_mode: match ctx.install_mode {
                InstallMode::System => StateInstallMode::System,
                InstallMode::User => StateInstallMode::User,
            },
            prefix: layout.prefix.clone(),
            ..Default::default()
        };
        state.upsert_object(obj);
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state");
    }

    fn load_state(ctx: &CliContext) -> InstalledState {
        let layout = common::resolve_layout(ctx);
        InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state")
    }

    /// A drifted rpm-observed component refreshes its EVR/arch/source from rpmdb
    /// while ownership, backend, and lifecycle status are preserved.
    #[test]
    fn repair_refreshes_drifted_evr_and_keeps_ownership() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        // rpmdb has moved on to 2.3.0 via a manual dnf update.
        let rpm = FakeQuery::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.3.0",
                Some("1.al8"),
                "x86_64",
            )),
        )
        .with_origin("alinux-updates");

        repair_with_query("copilot-shell", &c, &rpm).expect("repair ok");

        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("present");
        assert_eq!(obj.version, "2.3.0-1.al8", "version reconciled to rpmdb");
        assert_eq!(
            obj.ownership,
            Some(Ownership::RpmObserved),
            "ownership kept"
        );
        assert_eq!(obj.install_backend.as_deref(), Some("rpm"), "backend kept");
        assert_eq!(obj.status, ObjectStatus::Adopted, "status unchanged");
        let meta = obj.rpm_metadata.expect("metadata");
        assert_eq!(meta.evr.as_deref(), Some("2.3.0-1.al8"));
        assert_eq!(meta.source_repo.as_deref(), Some("alinux-updates"));
        assert_ne!(obj.last_operation_id.as_deref(), Some("op-prior"));
    }

    /// A failed origin lookup must not erase a previously-good source_repo.
    #[test]
    fn repair_keeps_prior_source_repo_when_origin_unknown() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        // No origin configured on the fake -> installed_origin yields None.
        let rpm = FakeQuery::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.3.0",
                Some("1.al8"),
                "x86_64",
            )),
        );
        repair_with_query("copilot-shell", &c, &rpm).expect("repair ok");
        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("present");
        assert_eq!(
            obj.rpm_metadata.expect("meta").source_repo.as_deref(),
            Some("@System"),
            "prior source_repo preserved when origin re-lookup is empty",
        );
    }

    /// `rpm -e`'d package: repair refuses and points at forget; state untouched.
    #[test]
    fn repair_on_missing_package_points_at_forget() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeQuery::new("anolisa-copilot-shell", None);
        let err =
            repair_with_query("copilot-shell", &c, &rpm).expect_err("removed package must error");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("forget"),
            "reason must point at forget: {}",
            err.reason()
        );
        assert_eq!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .map(|o| o.version.clone())
                .as_deref(),
            Some("2.2.0-1.al8"),
            "state must be untouched",
        );
    }

    /// A same-name multi-version rpmdb is an ambiguous reconcile target.
    #[test]
    fn repair_multi_version_is_refused() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmManaged,
                ObjectStatus::Installed,
            ),
        );
        let rpm = FakeQuery::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.2.0",
                Some("1.al8"),
                "x86_64",
            )),
        )
        .multi_version();
        let err =
            repair_with_query("copilot-shell", &c, &rpm).expect_err("multi-version must error");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(err.reason().contains("unexpected output"));
        assert!(err.reason().contains("2 installed versions"));
    }

    /// Missing rpm/dnf tooling surfaces as an actionable runtime error.
    #[test]
    fn repair_without_rpm_tooling_errors() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeQuery::new("anolisa-copilot-shell", None).command_missing();
        let err =
            repair_with_query("copilot-shell", &c, &rpm).expect_err("missing tooling must error");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(err.reason().contains("rpm/dnf not found"));
    }

    /// Raw components are not repairable yet -> NOT_IMPLEMENTED.
    #[test]
    fn repair_raw_component_is_not_implemented() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::User, false);
        seed(&c, raw_object("copilot-shell", "9.9.9"));
        let rpm = FakeQuery::new("anolisa-copilot-shell", None);
        let err = repair_with_query("copilot-shell", &c, &rpm)
            .expect_err("raw repair is not implemented");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
    }

    /// An absent component routes to INVALID_ARGUMENT (exit 2).
    #[test]
    fn repair_unknown_component_routes_to_invalid_argument() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let rpm = FakeQuery::new("anolisa-copilot-shell", None);
        let err =
            repair_with_query("copilot-shell", &c, &rpm).expect_err("absent component must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(err.reason().contains("not installed"));
    }

    /// Dry-run previews the reconcile without writing state.
    #[test]
    fn repair_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeQuery::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.3.0",
                Some("1.al8"),
                "x86_64",
            )),
        );
        repair_with_query("copilot-shell", &c, &rpm).expect("dry-run ok");
        assert_eq!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .map(|o| o.version.clone())
                .as_deref(),
            Some("2.2.0-1.al8"),
            "dry-run must not refresh the recorded version",
        );
    }

    /// Repair on an already-matching component is a no-op refresh: it succeeds,
    /// records an operation, and leaves the version in place.
    #[test]
    fn repair_no_op_when_already_matches() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.3.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeQuery::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.3.0",
                Some("1.al8"),
                "x86_64",
            )),
        );
        repair_with_query("copilot-shell", &c, &rpm).expect("repair ok");
        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("present");
        assert_eq!(obj.version, "2.3.0-1.al8");
        assert_ne!(obj.last_operation_id.as_deref(), Some("op-prior"));
    }

    /// A legacy RPM row with no recorded metadata is repaired by resolving the
    /// default package name and backfilling `rpm_metadata` from rpmdb.
    #[test]
    fn repair_backfills_metadata_for_legacy_row() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        // RPM-owned (managed) row, but rpm_metadata absent (pre-v3 shape).
        let mut obj = rpm_object(
            "legacy-rpm",
            "",
            "0.0.0",
            Ownership::RpmManaged,
            ObjectStatus::Installed,
        );
        obj.rpm_metadata = None;
        seed(&c, obj);
        // Candidate chain falls through to the default `anolisa-<component>`.
        let rpm = FakeQuery::new(
            "anolisa-legacy-rpm",
            Some(pkg_info(
                "anolisa-legacy-rpm",
                "1.0.0",
                Some("1.al8"),
                "x86_64",
            )),
        )
        .with_origin("@System");
        repair_with_query("legacy-rpm", &c, &rpm).expect("repair ok");
        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "legacy-rpm")
            .cloned()
            .expect("present");
        let meta = obj.rpm_metadata.expect("metadata backfilled");
        assert_eq!(meta.package_name, "anolisa-legacy-rpm");
        assert_eq!(meta.evr.as_deref(), Some("1.0.0-1.al8"));
        assert_eq!(obj.version, "1.0.0-1.al8");
    }

    /// CLI surface: `repair <component>` parses to the positional.
    #[test]
    fn repair_parses_positional_component() {
        use clap::Parser as _;
        let a = RepairArgs::try_parse_from(["repair", "copilot-shell"]).expect("parse");
        assert_eq!(a.component, "copilot-shell");
    }
}
