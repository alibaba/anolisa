//! `anolisa status [COMPONENT]` — read-only view of installed components.
//!
//! Reads `installed.toml` via the shared [`crate::commands::common`] helper
//! and lists every `Component`-kind object, or filters down to a single
//! name. A missing state file is the expected fresh-install case and yields
//! an empty result; an unknown component name surfaces a synthetic
//! `not_installed` record rather than an error (launch spec §7.1).
//!
//! This handler reports state-on-disk plus live read-only probes. Every
//! persisted field in [`ComponentRecord`] is projected straight from
//! [`InstalledObject`]; synthesized data is limited to read-only rpmdb,
//! integrity, adapter, and manifest health observations.

use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde::Serialize;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anolisa_core::adapter::claim::ClaimStatus;
#[cfg(test)]
use anolisa_core::adapter::manager::AdapterSourceStatus;
use anolisa_core::adapter::manager::ScanEntry;
use anolisa_core::path_safety::{PathBoundaryError, validate_owned_path};
use anolisa_core::{
    Catalog, HealthEntry, HealthSpec, InstalledObject, IntegrityStatus, RpmMetadata, ServiceState,
    check_owned_file, service_for_install_mode as service_factory,
};
#[cfg(test)]
use anolisa_core::{InstalledState, ObjectKind};

/// Wall-clock ceiling for a single manifest command-kind probe. `status`
/// is read-only; a hostile or buggy probe must not be able to hang the
/// CLI. 5s is generous for the smoke-test probes the spec describes
/// (`<bindir>/agentsight --help`) while keeping the worst case bounded.
const COMMAND_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Polling cadence for the command-probe wait loop. Mirrors the hook
/// runner — sub-second responsiveness for fast probes without burning
/// CPU.
const COMMAND_PROBE_POLL: Duration = Duration::from_millis(25);

/// Single-character glyphs that turn the command string into a shell
/// expression. Manifest probes are validated *not* to contain any of
/// these — we never run them through `sh -c`, so anything that requires
/// a shell to interpret is, by definition, not a valid probe.
const SHELL_METACHARS: &[char] = &[
    ';', '|', '&', '>', '<', '$', '`', '\\', '{', '}', '(', ')', '*', '?', '~', '!', '\n', '\r',
    '\'', '"',
];
use anolisa_env::EnvService;
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageQuery, PackageQueryError};
use anolisa_platform::rpm_query::RpmPackageQuery;

use crate::color::{Palette, pad_right};
use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::commands::state_view::{ScopedStateRoot, StateScope, StateView, StateVisibility};
use crate::commands::tier1::install::rpm_package_candidates_with_index;
use crate::context::{CliContext, InstallMode};
use crate::repo_config::BackendConfig;
use crate::resolution::{ComponentIndex, ResolutionUse, load_optional_component_index};
use crate::response::{CliError, render_json};

const COMMAND: &str = "status";

#[derive(Parser)]
pub struct StatusArgs {
    /// Show detail for a specific component (omit for aggregate view).
    pub component: Option<String>,
}

/// Summary of one adapter associated with a component, derived from
/// `AdapterManager::scan()`. Included in the component status record
/// when adapter declarations/resources/receipts exist.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AdapterSummaryRecord {
    pub(crate) component: String,
    pub(crate) framework: String,
    pub(crate) declared: bool,
    pub(crate) resource_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resource_root: Option<String>,
    pub(crate) driver_available: bool,
    pub(crate) framework_detected: bool,
    pub(crate) enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) claim_status: Option<ClaimStatus>,
    /// Source health for enabled adapter receipts. Present only for receipt
    /// rows so JSON consumers can identify orphaned user receipts separately
    /// from adapters that are simply not enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_status: Option<String>,
    /// Explanation for a missing adapter source, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_reason: Option<String>,
}

/// JSON-shaped record for a single component, used in both the wire
/// envelope and the human renderer. Fields are projected straight from
/// the matching [`InstalledObject`] on disk; optional/empty fields are
/// skipped when absent so synthetic `not_installed` records stay compact.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ComponentRecord {
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) scope: String,
    pub(crate) active: bool,
    pub(crate) mutable_by_current_invocation: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) shadowed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) state_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) installed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_operation_id: Option<String>,
    /// Feature flags the install record marks as enabled.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) enabled_features: Vec<String>,
    /// Last-known health probe entries persisted in state. Empty until a
    /// background probe wires up — but still surfaced verbatim today so
    /// users see whatever the install runner recorded.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) health: Vec<HealthEntry>,
    /// Associated adapter summaries from `AdapterManager::scan()`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) adapters: Vec<AdapterSummaryRecord>,
    /// RPM package name, set only for `observed` rows (rpmdb hit, no state)
    /// and adopted `rpm-observed` rows (§8). Absent for raw components.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rpm_package: Option<String>,
    /// Full EVR of the RPM, paired with [`rpm_package`](Self::rpm_package).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rpm_evr: Option<String>,
    /// Source repo/label that supplied the RPM (e.g. `@System`); `None` when
    /// it could not be determined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rpm_source_repo: Option<String>,
    /// System packages auto-installed by the provisioner during component
    /// install. Empty when no packages were provisioned.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) provisioned_packages: Vec<String>,
}

#[derive(Serialize)]
struct StatusPayload {
    components: Vec<ComponentRecord>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct ScopedCatalogs {
    entries: Vec<ScopedCatalog>,
}

#[derive(Debug)]
struct ScopedCatalog {
    state_path: PathBuf,
    catalog: Catalog,
}

impl ScopedCatalogs {
    pub(crate) fn load(view: &StateView, command: &str) -> Self {
        let entries = view
            .visible_roots
            .iter()
            .filter_map(|root| {
                common::load_catalog_for_layout(
                    &root.layout,
                    install_mode_for_scope(root.scope),
                    command,
                )
                .ok()
                .map(|catalog| ScopedCatalog {
                    state_path: root.state_path.clone(),
                    catalog,
                })
            })
            .collect();
        Self { entries }
    }

    #[cfg(test)]
    fn from_entries(entries: Vec<(PathBuf, Catalog)>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(state_path, catalog)| ScopedCatalog {
                    state_path,
                    catalog,
                })
                .collect(),
        }
    }

    pub(crate) fn catalog_for(&self, root: &ScopedStateRoot) -> Option<&Catalog> {
        self.entries
            .iter()
            .find(|entry| entry.state_path == root.state_path)
            .map(|entry| &entry.catalog)
    }
}

pub fn handle(args: StatusArgs, ctx: &CliContext) -> Result<(), CliError> {
    let mut view = StateView::load(ctx, COMMAND, StateVisibility::UserPlusSystem)?;
    migrate_view_states(&mut view);
    let layout = common::resolve_layout(ctx);
    // Catalogs are best-effort: if manifests are missing or malformed for a
    // scoped root, status still reports state-on-disk plus integrity. Manifest
    // health checks are additive and must not mask a working install.
    let catalogs = ScopedCatalogs::load(&view, COMMAND);
    let adapter_scan = common::build_adapter_manager(ctx).scan().ok();

    let query = RpmPackageQuery::system();
    let selected_component = args
        .component
        .as_deref()
        .map(|target| lookup_component_name_from_view(target, &view, ctx));

    // repo_config / component_index are still needed for the observed-record
    // probe below (system mode only). Name resolution above is handled by
    // common::lookup_component_name which loads its own config.
    let repo_config = (ctx.install_mode == InstallMode::System && args.component.is_some())
        .then(|| {
            common::load_repo_config(ctx, &layout, COMMAND, RepoPersistPolicy::BestEffort).ok()
        })
        .flatten();
    let rpm_backend = repo_config.as_ref().and_then(|c| c.backends.get("rpm"));
    let env = EnvService::detect();
    let component_index = repo_config
        .as_ref()
        .and_then(|cfg| load_optional_component_index(&layout, &env, cfg));

    let mut records = select_components_from_view(
        &view,
        &catalogs,
        selected_component.as_deref(),
        adapter_scan.as_ref().map(|r| r.entries.as_slice()),
        Some(&query),
    );

    // Read-only Observed report (§8): when a named component is absent from
    // ANOLISA state but present in rpmdb (system mode), upgrade the synthetic
    // `not_installed` row to `observed` with the package/EVR/repo. This never
    // writes state — adopting is `install`'s job.
    if let Some(target) = selected_component.as_deref()
        && ctx.install_mode == InstallMode::System
        && records.len() == 1
        && records[0].status == "not_installed"
    {
        if let Some(observed) =
            observed_record(target, rpm_backend, component_index.as_ref(), &query)
        {
            records = vec![observed];
        }
    }

    if ctx.json {
        return render_json(
            COMMAND,
            StatusPayload {
                components: records,
                warnings: view.warnings,
            },
        );
    }

    if !ctx.quiet {
        render_warnings(&view.warnings);
        render_human(&records, ctx.verbose, ctx.no_color);
    }
    Ok(())
}

pub(crate) fn migrate_view_states(view: &mut StateView) {
    for root in &mut view.visible_roots {
        common::migrate_v3_symlinks(&mut root.state, &root.layout);
    }
    if let Some(root) = view.visible_roots.iter().find(|root| root.writable) {
        view.writable = root.clone();
    }
}

fn lookup_component_name_from_view(input: &str, view: &StateView, ctx: &CliContext) -> String {
    if view
        .visible_components()
        .iter()
        .any(|record| record.object.name == input)
    {
        return input.to_string();
    }
    common::lookup_component_name(input, &view.writable.state, ctx, COMMAND)
}

const fn install_mode_for_scope(scope: StateScope) -> InstallMode {
    match scope {
        StateScope::User => InstallMode::User,
        StateScope::System => InstallMode::System,
    }
}

/// Pure selector: project [`InstalledState`] down to component records,
/// optionally filtered to a single name. Extracted so tests can exercise
/// the filtering/synthetic-not-installed logic without mocking
/// `CliContext` or touching the filesystem.
#[cfg(test)]
pub(crate) fn select_components(
    state: &InstalledState,
    layout: &FsLayout,
    catalog: Option<&Catalog>,
    install_mode: &str,
    name: Option<&str>,
    adapter_scan: Option<&[ScanEntry]>,
) -> Vec<ComponentRecord> {
    let installed: Vec<&InstalledObject> = state
        .objects
        .iter()
        .filter(|o| o.kind == ObjectKind::Component)
        .collect();

    match name {
        None => installed
            .iter()
            .map(|o| {
                let mut rec = record_from_object(layout, catalog, install_mode, o);
                apply_current_scope(&mut rec, layout, install_mode);
                rec.adapters = adapter_summaries_for(&o.name, adapter_scan);
                rec
            })
            .collect(),
        Some(target) => match installed.iter().find(|o| o.name == target) {
            Some(obj) => {
                let mut rec = record_from_object(layout, catalog, install_mode, obj);
                apply_current_scope(&mut rec, layout, install_mode);
                rec.adapters = adapter_summaries_for(&obj.name, adapter_scan);
                vec![rec]
            }
            None => vec![not_installed_record(target)],
        },
    }
}

pub(crate) fn select_components_from_view(
    view: &StateView,
    catalogs: &ScopedCatalogs,
    name: Option<&str>,
    adapter_scan: Option<&[ScanEntry]>,
    rpm_query: Option<&dyn PackageQuery>,
) -> Vec<ComponentRecord> {
    let visible_components = view.visible_components();
    let selected: Vec<_> = match name {
        None => visible_components
            .iter()
            .copied()
            .filter(|record| record.active)
            .collect(),
        Some(target) => visible_components
            .iter()
            .copied()
            .filter(|record| record.object.name == target)
            .collect(),
    };

    let Some(target_records) = (!selected.is_empty()).then_some(selected) else {
        return name
            .map(|target| vec![not_installed_record(target)])
            .unwrap_or_default();
    };

    let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    target_records
        .into_iter()
        .map(|record| {
            let catalog = catalogs.catalog_for(record.root);
            let mut component = record_from_object(
                &record.root.layout,
                catalog,
                record.scope().label(),
                record.object,
            );
            component.adapters = adapter_summaries_for(&component.name, adapter_scan);
            apply_record_scope(&mut component, &record);
            if let Some(query) = rpm_query {
                apply_rpm_drift_to_record(
                    &mut component,
                    record.object.rpm_metadata.as_ref(),
                    query,
                    &checked_at,
                );
            }
            component
        })
        .collect()
}

fn apply_record_scope(
    component: &mut ComponentRecord,
    record: &crate::commands::state_view::ScopedInstalledObject<'_>,
) {
    component.scope = record.scope().label().to_string();
    component.active = record.active;
    component.mutable_by_current_invocation = record.mutable_by_current_invocation;
    component.shadowed_by = record
        .shadowed_by
        .map(StateScope::label)
        .map(str::to_string);
    component.state_path = Some(record.root.state_path.display().to_string());
}

#[cfg(test)]
fn apply_current_scope(component: &mut ComponentRecord, layout: &FsLayout, install_mode: &str) {
    component.scope = install_mode.to_string();
    component.active = true;
    component.mutable_by_current_invocation = true;
    component.shadowed_by = None;
    component.state_path = Some(
        layout
            .state_dir
            .join("installed.toml")
            .display()
            .to_string(),
    );
}

fn not_installed_record(name: &str) -> ComponentRecord {
    ComponentRecord {
        name: name.to_string(),
        status: "not_installed".to_string(),
        scope: "none".to_string(),
        active: false,
        mutable_by_current_invocation: false,
        shadowed_by: None,
        state_path: None,
        version: None,
        installed_at: None,
        last_operation_id: None,
        enabled_features: Vec::new(),
        health: Vec::new(),
        adapters: Vec::new(),
        rpm_package: None,
        rpm_evr: None,
        rpm_source_repo: None,
        provisioned_packages: Vec::new(),
    }
}

/// Probe rpmdb for `component` and, if a matching system RPM is installed,
/// build an `observed` record (§8). Returns `None` when nothing is installed
/// or the host has no rpm tooling — the caller keeps the `not_installed` row.
///
/// Read-only and best-effort: a single hard query failure on a candidate
/// stops the probe (returns `None`) rather than failing `status`. `query` is
/// injected so tests can drive this without a live rpmdb.
fn observed_record(
    component: &str,
    rpm_backend: Option<&BackendConfig>,
    component_index: Option<&ComponentIndex>,
    query: &dyn PackageQuery,
) -> Option<ComponentRecord> {
    // Same resolver as adopt (§5), minus the CLI `--package` override (status
    // takes no such flag).
    let candidates = rpm_package_candidates_with_index(
        None,
        rpm_backend,
        component_index,
        query,
        component,
        ResolutionUse::StatusObserved,
    )
    .ok()?;
    for target in candidates {
        let Ok(Some(info)) = query.query_installed(&target.package) else {
            // Not this candidate (absent), or a hard error / multi-version
            // drift: status does not adjudicate drift, so move on.
            continue;
        };
        let evr = info.version.to_string();
        let source_repo = query.installed_origin(&target.package).ok().flatten();
        return Some(ComponentRecord {
            name: target.component,
            status: "observed".to_string(),
            scope: "system".to_string(),
            active: false,
            mutable_by_current_invocation: false,
            shadowed_by: None,
            state_path: None,
            version: Some(evr.clone()),
            installed_at: None,
            last_operation_id: None,
            enabled_features: Vec::new(),
            health: Vec::new(),
            adapters: Vec::new(),
            rpm_package: Some(info.name),
            rpm_evr: Some(evr),
            rpm_source_repo: source_repo,
            provisioned_packages: Vec::new(),
        });
    }
    None
}

/// Live rpmdb-drift classification for an in-state RPM component (#960).
///
/// `None` (from [`probe_rpm_drift`]) means "no drift to report" — the recorded
/// status stands. The two variants are the manual-mutation cases the proposal
/// calls out: a `dnf update`/`downgrade` (or a same-name multi-version rpmdb)
/// surfaces as [`Drifted`](RpmDrift::Drifted); an `rpm -e` surfaces as
/// [`Missing`](RpmDrift::Missing).
// pub(crate): the cross-command MVP lifecycle test (#963) asserts on these variants.
pub(crate) enum RpmDrift {
    /// rpmdb holds the package at a different version than ANOLISA recorded, or
    /// holds several versions at once. `reason` explains which.
    Drifted { reason: String },
    /// rpmdb no longer holds the package at all.
    Missing,
}

/// Compare an RPM component's recorded EVR against live rpmdb reality.
///
/// `status` is read-only and best-effort: an unrunnable or anomalous query
/// (`rpm`/`dnf` missing, spawn/permission/parse failure) yields `None` rather
/// than crying drift on a read we cannot trust. A same-name multi-version rpmdb
/// is a genuine divergence from the single recorded version, so it classifies
/// as drift. `query` is injected so tests drive this without a live rpmdb.
// pub(crate): driven by the cross-command MVP lifecycle test (#963).
pub(crate) fn probe_rpm_drift(meta: &RpmMetadata, query: &dyn PackageQuery) -> Option<RpmDrift> {
    match query.query_installed(&meta.package_name) {
        Ok(Some(info)) => {
            let live = info.version.to_string();
            match meta.evr.as_deref() {
                // Recorded EVR diverges from rpmdb: a manual dnf update/downgrade.
                Some(recorded) if recorded != live => Some(RpmDrift::Drifted {
                    reason: format!(
                        "rpmdb reports {live} for package {} but ANOLISA state records {recorded}",
                        meta.package_name
                    ),
                }),
                // EVR matches, or none recorded to compare against: no drift.
                _ => None,
            }
        }
        // State records the package but rpmdb no longer has it: an `rpm -e` drift.
        Ok(None) => Some(RpmDrift::Missing),
        // rpm returned output we can't reduce to a single installed version
        // (several versions, a malformed `--qf` row, or none on a zero exit).
        // The recorded version can no longer be trusted as-is, so surface it as
        // drift carrying the backend's own detail rather than guessing the cause.
        Err(PackageQueryError::UnexpectedOutput { detail, .. }) => Some(RpmDrift::Drifted {
            reason: format!(
                "rpmdb returned unexpected output for package {}: {detail}",
                meta.package_name
            ),
        }),
        // rpm/dnf absent, or a spawn/permission/query failure: cannot prove
        // drift on an unrunnable query, so keep the recorded status untouched.
        Err(_) => None,
    }
}

fn apply_rpm_drift_to_record(
    record: &mut ComponentRecord,
    rpm_meta: Option<&RpmMetadata>,
    query: &dyn PackageQuery,
    checked_at: &str,
) {
    // Only adjudicate drift on a clean live projection; never demote a
    // failed/degraded/disabled record (see fn doc). This also bounds the
    // rpm -q probes below to the records that can actually drift.
    if !matches!(record.status.as_str(), "installed" | "adopted") {
        return;
    }
    let Some(meta) = rpm_meta else {
        return;
    };
    let (status, reason) = match probe_rpm_drift(meta, query) {
        Some(RpmDrift::Drifted { reason }) => ("drifted", reason),
        Some(RpmDrift::Missing) => (
            "missing",
            format!(
                "package {} recorded in ANOLISA state is no longer present in rpmdb",
                meta.package_name
            ),
        ),
        None => return,
    };
    record.status = status.to_string();
    record.health.push(HealthEntry {
        name: "rpm:drift".to_string(),
        status: status.to_string(),
        checked_at: checked_at.to_string(),
        reason: Some(reason),
    });
}

/// Build adapter summary records for `component` from the scan entries.
fn adapter_summaries_for(component: &str, scan: Option<&[ScanEntry]>) -> Vec<AdapterSummaryRecord> {
    let Some(entries) = scan else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|e| e.component == component)
        .map(|e| AdapterSummaryRecord {
            component: e.component.clone(),
            framework: e.framework.clone(),
            declared: e.declared,
            resource_present: e.resource_root.is_some(),
            resource_root: e.resource_root.as_ref().map(|p| p.display().to_string()),
            driver_available: e.driver_available,
            framework_detected: e.framework_detected,
            enabled: e.enabled,
            claim_status: e.claim_status,
            source_status: e.source_status.map(|status| status.label().to_string()),
            source_reason: e.source_reason.clone(),
        })
        .collect()
}

fn record_from_object(
    layout: &FsLayout,
    catalog: Option<&Catalog>,
    install_mode: &str,
    obj: &InstalledObject,
) -> ComponentRecord {
    // Start from the state's last-known health entries, then layer the
    // live integrity probe on top. The integrity probe is authoritative
    // for owned-file existence and sha256; it can escalate the wire
    // status from `installed` to `degraded` or `failed` without us
    // touching the on-disk state.
    let base_status = common::object_status_str(obj.status).to_string();
    let mut health = obj.health.clone();
    let (integrity_entries, integrity_status) = integrity_probe(layout, obj, &base_status);
    health.extend(integrity_entries);

    // Layer manifest-driven health checks on top. Each entry can escalate
    // the wire status independently of integrity (a missing service unit
    // can fail an otherwise-clean install). Probes are skipped when no
    // catalog is loaded — fresh checkouts without a packaged catalog still
    // get integrity-only behavior.
    //
    // RpmManaged and RpmObserved objects both use the file layout selected by
    // RPM macros rather than ANOLISA's raw-backend layout. Manifest health
    // probes assume the raw layout, so applying them to either RPM ownership
    // can spuriously escalate a valid package. RPM status remains adjudicated
    // by the rpmdb drift probe after this projection.
    let manifest_status = match catalog {
        Some(cat) if !obj.effective_ownership().is_rpm() => {
            let (manifest_entries, escalated) =
                manifest_health_probe(layout, cat, install_mode, obj, &integrity_status);
            health.extend(manifest_entries);
            escalated
        }
        _ => integrity_status,
    };

    // Surface RPM provenance for adopted rpm-observed rows so human/JSON
    // output shows the package/EVR/repo behind the `adopted` status.
    let (rpm_package, rpm_evr, rpm_source_repo) = match &obj.rpm_metadata {
        Some(meta) => (
            Some(meta.package_name.clone()),
            meta.evr.clone(),
            meta.source_repo.clone(),
        ),
        None => (None, None, None),
    };

    ComponentRecord {
        name: obj.name.clone(),
        status: manifest_status,
        scope: "none".to_string(),
        active: true,
        mutable_by_current_invocation: false,
        shadowed_by: None,
        state_path: None,
        version: Some(obj.version.clone()),
        installed_at: Some(obj.installed_at.clone()),
        last_operation_id: obj.last_operation_id.clone(),
        enabled_features: obj.enabled_features.clone(),
        health,
        adapters: Vec::new(),
        rpm_package,
        rpm_evr,
        rpm_source_repo,
        provisioned_packages: obj.provisioned_packages.clone(),
    }
}

/// Probe the integrity of every file owned by `component` and return
/// synthesized [`HealthEntry`] items plus the (possibly escalated) wire
/// status label.
///
/// Escalation rules (only move toward more-broken, never back):
/// - any [`IntegrityStatus::is_failure`] result → `"failed"`
/// - any [`IntegrityStatus::Unverified`] result on an otherwise-clean
///   component → `"degraded"`
/// - otherwise the base status (`installed`/`disabled`/etc) is preserved
///
/// Status is left untouched when the component is already `disabled`
/// or `not_installed`: probing a disabled component and demoting it
/// to `degraded` would be a regression in the meaning of `disabled`.
fn integrity_probe(
    layout: &FsLayout,
    component: &InstalledObject,
    base_status: &str,
) -> (Vec<HealthEntry>, String) {
    let mut entries: Vec<HealthEntry> = Vec::new();
    let mut had_failure = false;
    let mut had_unverified = false;
    let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    for file in &component.files {
        let result = check_owned_file(layout, file);
        if result == IntegrityStatus::Skipped {
            continue;
        }
        if result.is_failure() {
            had_failure = true;
        } else if matches!(result, IntegrityStatus::Unverified) {
            had_unverified = true;
        }
        entries.push(HealthEntry {
            name: format!("integrity:{}", file.path.display()),
            status: result.label().to_string(),
            checked_at: checked_at.clone(),
            reason: None,
        });
    }

    // Only escalate from "installed"/"adopted" — keep "disabled"/"failed"
    // as-is so a disabled component does not get demoted by a stale
    // sha256 mismatch on disk.
    let escalated = match base_status {
        "installed" | "adopted" if had_failure => "failed".to_string(),
        "installed" | "adopted" if had_unverified => "degraded".to_string(),
        _ => base_status.to_string(),
    };
    (entries, escalated)
}

/// Look up the component's manifest in the layered catalog and run each
/// declared `[[health_checks]]` entry. Three kinds are supported today
/// (file/command/systemd); unknown kinds are reported verbatim with
/// `status = "unsupported_kind"` so a future probe doesn't get silently
/// dropped.
///
/// Escalation rules (status moves only toward more-broken):
/// - required check fails → `"failed"`
/// - optional check fails → `"degraded"`
/// - service backend not supported (user mode, container, non-Linux) → entry
///   marked `"not_supported"` and degrades to `"degraded"` (we can't prove
///   the unit is up, but we have no positive failure either)
/// - on `"disabled"`/`"failed"`/`"not_installed"` the wire status is left
///   alone — the same rationale as integrity_probe.
fn manifest_health_probe(
    layout: &FsLayout,
    catalog: &Catalog,
    install_mode: &str,
    component: &InstalledObject,
    base_status: &str,
) -> (Vec<HealthEntry>, String) {
    let mut entries: Vec<HealthEntry> = Vec::new();
    let mut had_failure = false;
    let mut had_degrade = false;
    let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    // Lazily build the service manager — most checks won't need it, and
    // a `EnvService::detect()` call in user mode shells out to `uname -r`.
    let mut service_manager: Option<Box<dyn anolisa_core::ServiceManager>> = None;

    // No manifest in the catalog — silent. This is the out-of-tree
    // component case (the alpha contract); the integrity probe already
    // covers everything the state file knows about.
    if let Some(manifest) = catalog.component(&component.name) {
        for check in &manifest.health_checks {
            let optional = check.optional.unwrap_or(false);
            let entry_name = format!(
                "{}:{}:{}",
                component.name,
                check.kind,
                check.name.as_deref().unwrap_or("(unnamed)")
            );

            let outcome = match check.kind.as_str() {
                "file" => probe_file_check(layout, check),
                "command" => probe_command_check(layout, check),
                "systemd" => {
                    if service_manager.is_none() {
                        let env = EnvService::detect();
                        service_manager = Some(service_factory(install_mode, &env));
                    }
                    let mgr = service_manager.as_deref().expect("service manager built");
                    probe_systemd_check(mgr, check)
                }
                other => HealthOutcome {
                    label: "unsupported_kind".to_string(),
                    reason: Some(format!("manifest health kind '{other}' is not supported")),
                    state: HealthCheckState::Unsupported,
                },
            };

            match outcome.state {
                HealthCheckState::Ok => {}
                HealthCheckState::Unsupported => {
                    had_degrade = true;
                }
                HealthCheckState::Failed if optional => {
                    had_degrade = true;
                }
                HealthCheckState::Failed => {
                    had_failure = true;
                }
            }
            entries.push(HealthEntry {
                name: entry_name,
                status: outcome.label,
                checked_at: checked_at.clone(),
                reason: outcome.reason,
            });
        }
    }

    let escalated = match base_status {
        "installed" | "adopted" if had_failure => "failed".to_string(),
        "installed" | "adopted" if had_degrade => "degraded".to_string(),
        // Already escalated by the integrity probe — preserve "failed" /
        // "degraded" rather than letting a manifest "ok" downgrade it.
        _ => base_status.to_string(),
    };
    (entries, escalated)
}

#[derive(Debug)]
enum HealthCheckState {
    Ok,
    Failed,
    Unsupported,
}

#[derive(Debug)]
struct HealthOutcome {
    label: String,
    reason: Option<String>,
    state: HealthCheckState,
}

/// Resolve `{bindir}` / `{datadir}` / `{etcdir}` placeholders in a
/// manifest path. Manifests are written against logical roots so the
/// same string works in system and user mode; the layout supplies the
/// concrete path for the active install mode.
fn expand_layout_placeholders(input: &str, layout: &FsLayout) -> String {
    input
        .replace("{bindir}", &layout.bin_dir.display().to_string())
        .replace("{datadir}", &layout.datadir.display().to_string())
        .replace("{etcdir}", &layout.etc_dir.display().to_string())
        .replace("{statedir}", &layout.state_dir.display().to_string())
}

/// File-kind probe with two security guards layered on top of the
/// "does this file exist" question:
///
///   1. `validate_owned_path` — a manifest pointing the probe at
///      `/etc/passwd` or a `..`-traversal must NOT trigger a stat that
///      could leak existence to a passive attacker via timing or
///      surface a sensitive file in the wire output. External paths
///      degrade to `out_of_bounds` / `Unsupported` so the component
///      goes `degraded` (not `failed`) — the manifest is misauthored,
///      not the install.
///   2. `symlink_metadata` instead of `Path::exists()` — `exists()`
///      follows symlinks, which means a manifest whose `probe` resolves
///      to a path under `bin_dir` could still have someone plant
///      `<bin_dir>/probe -> /etc/shadow` and turn `status` into a
///      symlink-follow primitive. We treat symlinks themselves as
///      `unsupported_target` so probes have to be authored against
///      real files.
fn probe_file_check(layout: &FsLayout, spec: &HealthSpec) -> HealthOutcome {
    let raw = spec
        .probe
        .as_deref()
        .or(spec.command.as_deref())
        .unwrap_or("");
    if raw.is_empty() {
        return HealthOutcome {
            label: "invalid_check".to_string(),
            reason: Some("file check missing 'probe' (or 'command') path".to_string()),
            state: HealthCheckState::Failed,
        };
    }
    let expanded = expand_layout_placeholders(raw, layout);
    let path = std::path::Path::new(&expanded);
    if let Err(err) = validate_owned_path(layout, path) {
        return HealthOutcome {
            label: "out_of_bounds".to_string(),
            reason: Some(format!(
                "manifest probe path '{expanded}' rejected: {}",
                boundary_reason(&err)
            )),
            state: HealthCheckState::Unsupported,
        };
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => HealthOutcome {
            label: "unsupported_target".to_string(),
            reason: Some(format!(
                "manifest probe path '{expanded}' is a symlink — refusing to follow"
            )),
            state: HealthCheckState::Unsupported,
        },
        Ok(meta) if !meta.file_type().is_file() => HealthOutcome {
            // Non-regular targets (directory, fifo, socket, char/block
            // device) cannot honestly satisfy a `kind = "file"` check.
            // Returning `ok` for a directory turned a misauthored
            // manifest into a silent green light; surfacing
            // `not_regular_file` makes the manifest bug visible in the
            // wire output without escalating the component to `failed`
            // (the install itself is fine — the manifest is wrong).
            label: "not_regular_file".to_string(),
            reason: Some(format!(
                "manifest probe path '{expanded}' is not a regular file"
            )),
            state: HealthCheckState::Unsupported,
        },
        Ok(_) => HealthOutcome {
            label: "ok".to_string(),
            reason: Some(format!("file present at {expanded}")),
            state: HealthCheckState::Ok,
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HealthOutcome {
            label: "missing_file".to_string(),
            reason: Some(format!("expected file not present at {expanded}")),
            state: HealthCheckState::Failed,
        },
        Err(err) => HealthOutcome {
            label: "stat_error".to_string(),
            reason: Some(format!("stat failed for '{expanded}': {err}")),
            state: HealthCheckState::Failed,
        },
    }
}

fn boundary_reason(err: &PathBoundaryError) -> String {
    match err {
        PathBoundaryError::External { path } => {
            format!("'{}' is outside ANOLISA-owned roots", path.display())
        }
        PathBoundaryError::Traversal { path } => {
            format!("'{}' contains '.' or '..' segments", path.display())
        }
    }
}

/// Command-kind probe, hardened against the two attack surfaces a naïve
/// `sh -c <manifest_string>` exposes:
///
///   1. **Arbitrary shell**: the probe runs *without* a shell. We split
///      the command string on ASCII whitespace and run the first token
///      as the executable, the rest as argv. Anything containing shell
///      metacharacters (`;|&><$\` etc.) is refused — `status` is a
///      read-only verb and must not let a third-party manifest run a
///      pipeline, redirect to a file, or expand variables.
///   2. **Arbitrary executable**: the executable must be an absolute
///      path under an ANOLISA-owned root (after `{bindir}` placeholder
///      expansion). A manifest that probes via `/usr/bin/curl` or a
///      bare `true` is refused — the only commands `status` may run
///      are ones the framework itself shipped + path-safety vetted.
///   3. **Hang**: the spawned child is bounded by `COMMAND_PROBE_TIMEOUT`.
///      A runaway probe is killed and the entry surfaces as `timeout`,
///      escalating to `degraded` so the wire status reflects "couldn't
///      verify" rather than the misleading "passed".
fn probe_command_check(layout: &FsLayout, spec: &HealthSpec) -> HealthOutcome {
    let raw = spec.command.as_deref().unwrap_or("");
    if raw.is_empty() {
        return HealthOutcome {
            label: "invalid_check".to_string(),
            reason: Some("command check missing 'command' string".to_string()),
            state: HealthCheckState::Failed,
        };
    }
    let expanded = expand_layout_placeholders(raw, layout);

    if let Some(meta) = expanded.chars().find(|c| SHELL_METACHARS.contains(c)) {
        return HealthOutcome {
            label: "invalid_check".to_string(),
            reason: Some(format!(
                "manifest probe '{expanded}' contains shell metacharacter '{meta}' — \
                 commands run without a shell, declare a single executable + plain args",
            )),
            state: HealthCheckState::Unsupported,
        };
    }

    let mut tokens = expanded.split_ascii_whitespace();
    let exe = match tokens.next() {
        Some(e) => e,
        None => {
            return HealthOutcome {
                label: "invalid_check".to_string(),
                reason: Some("manifest probe is empty after placeholder expansion".to_string()),
                state: HealthCheckState::Failed,
            };
        }
    };
    let args: Vec<&str> = tokens.collect();

    let exe_path = std::path::Path::new(exe);
    if !exe_path.is_absolute() {
        return HealthOutcome {
            label: "out_of_bounds".to_string(),
            reason: Some(format!(
                "manifest probe executable '{exe}' is not absolute — declare \
                 the full `{{bindir}}/...` path",
            )),
            state: HealthCheckState::Unsupported,
        };
    }
    if let Err(err) = validate_owned_path(layout, exe_path) {
        return HealthOutcome {
            label: "out_of_bounds".to_string(),
            reason: Some(format!(
                "manifest probe executable '{exe}' rejected: {}",
                boundary_reason(&err)
            )),
            state: HealthCheckState::Unsupported,
        };
    }

    let mut child = match Command::new(exe_path)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => {
            return HealthOutcome {
                label: "command_error".to_string(),
                reason: Some(format!("failed to spawn '{expanded}': {err}")),
                state: HealthCheckState::Failed,
            };
        }
    };

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return HealthOutcome {
                        label: "ok".to_string(),
                        reason: Some(format!("`{expanded}` exited 0")),
                        state: HealthCheckState::Ok,
                    };
                }
                return HealthOutcome {
                    label: "command_failed".to_string(),
                    reason: Some(format!(
                        "`{expanded}` exited with status {}",
                        status
                            .code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "signal".to_string()),
                    )),
                    state: HealthCheckState::Failed,
                };
            }
            Ok(None) => {
                if started.elapsed() > COMMAND_PROBE_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return HealthOutcome {
                        label: "timeout".to_string(),
                        reason: Some(format!(
                            "`{expanded}` exceeded {}s probe timeout",
                            COMMAND_PROBE_TIMEOUT.as_secs(),
                        )),
                        state: HealthCheckState::Unsupported,
                    };
                }
                std::thread::sleep(COMMAND_PROBE_POLL);
            }
            Err(err) => {
                return HealthOutcome {
                    label: "command_error".to_string(),
                    reason: Some(format!("wait failed for '{expanded}': {err}")),
                    state: HealthCheckState::Failed,
                };
            }
        }
    }
}

fn probe_systemd_check(
    manager: &dyn anolisa_core::ServiceManager,
    spec: &HealthSpec,
) -> HealthOutcome {
    let unit = match spec.unit.as_deref() {
        Some(u) if !u.is_empty() => u,
        _ => {
            return HealthOutcome {
                label: "invalid_check".to_string(),
                reason: Some("systemd check missing 'unit' name".to_string()),
                state: HealthCheckState::Failed,
            };
        }
    };
    if !manager.supported() {
        let reason = manager
            .unsupported_reason()
            .unwrap_or("service manager not supported in this environment")
            .to_string();
        return HealthOutcome {
            label: "not_supported".to_string(),
            reason: Some(reason),
            state: HealthCheckState::Unsupported,
        };
    }
    match manager.probe_service(unit) {
        Ok(outcome) => match outcome.state {
            ServiceState::Active => HealthOutcome {
                label: "ok".to_string(),
                reason: Some(format!("unit '{unit}' is active")),
                state: HealthCheckState::Ok,
            },
            ServiceState::NotInstalled => HealthOutcome {
                label: "not_installed".to_string(),
                reason: Some(format!("unit '{unit}' is not installed")),
                state: HealthCheckState::Failed,
            },
            ServiceState::NotSupported => HealthOutcome {
                label: "not_supported".to_string(),
                reason: outcome
                    .message
                    .is_empty()
                    .then(|| "service manager unsupported".to_string())
                    .or(Some(outcome.message.clone())),
                state: HealthCheckState::Unsupported,
            },
            other => HealthOutcome {
                label: other.as_str().to_string(),
                reason: Some(format!("unit '{unit}' state '{}'", other.as_str())),
                state: HealthCheckState::Failed,
            },
        },
        Err(err) => HealthOutcome {
            label: "probe_error".to_string(),
            reason: Some(format!("probe failed for '{unit}': {err}")),
            state: HealthCheckState::Failed,
        },
    }
}

fn render_human(records: &[ComponentRecord], verbose: bool, no_color: bool) {
    let color = Palette::new(no_color);
    if records.is_empty() {
        println!("{}", color.muted("no installed components"));
        return;
    }

    println!(
        "{}",
        color.header(format!(
            "{:<28}  {:<8}  {:<14}  {:<10}  {}",
            "NAME", "SCOPE", "STATUS", "VERSION", "INSTALLED_AT"
        ))
    );
    for record in records {
        let version = record.version.as_deref().unwrap_or("-");
        let installed_at = record.installed_at.as_deref().unwrap_or("-");
        println!(
            "{name:<28}  {scope:<8}  {status:<14}  {version:<10}  {installed_at}",
            name = record.name,
            scope = record.scope,
            status = color.status(pad_right(&record.status, 14)),
            version = version,
            installed_at = color.muted(installed_at),
        );
        for (label, value) in scope_metadata_pairs(record) {
            println!("    {} {}", color.label(format!("{label}:")), value);
        }
        // RPM provenance for observed / adopted rpm-observed rows (§8).
        if let Some(pkg) = record.rpm_package.as_deref() {
            let repo = record.rpm_source_repo.as_deref().unwrap_or("unknown repo");
            let evr = record.rpm_evr.as_deref().unwrap_or("-");
            println!("    {} {pkg} ({evr}, {repo})", color.label("rpm package:"),);
        }
        // Observed = present in rpmdb but not yet tracked; point at adopt.
        if record.status == "observed" {
            println!(
                "    {} run 'sudo anolisa adopt {}' to adopt as rpm-observed",
                color.label("hint:"),
                record.name,
            );
        }
        // Drift / missing point at the repair / forget remediation (#960).
        if record.status == "drifted" {
            println!(
                "    {} run 'anolisa repair {}' to refresh ANOLISA state from rpmdb",
                color.label("hint:"),
                record.name,
            );
        } else if record.status == "missing" {
            println!(
                "    {} reinstall, or run 'anolisa forget {name}' to drop the stale state (repair cannot refresh a package gone from rpmdb)",
                color.label("hint:"),
                name = record.name,
            );
        }
        if verbose {
            if let Some(op) = record.last_operation_id.as_deref() {
                println!("    {} {}", color.label("last_operation_id:"), color.id(op));
            }
            if !record.enabled_features.is_empty() {
                println!(
                    "    {} {}",
                    color.label("enabled_features:"),
                    record.enabled_features.join(", ")
                );
            }
            if !record.provisioned_packages.is_empty() {
                println!(
                    "    {} {}",
                    color.label("provisioned_packages:"),
                    record.provisioned_packages.join(", ")
                );
            }
            for entry in &record.health {
                println!(
                    "    {} {} @ {}",
                    color.label(format!("health[{}]:", entry.name)),
                    color.status(&entry.status),
                    color.muted(&entry.checked_at)
                );
            }
        }
        if !record.adapters.is_empty() {
            println!("    {}", color.label("Associated Adapters:"));
            for adapter in &record.adapters {
                println!("      {}/{}", adapter.component, adapter.framework);
                println!(
                    "        {} {}",
                    color.label("Resource:"),
                    if adapter.resource_present {
                        "present"
                    } else {
                        "missing"
                    }
                );
                println!(
                    "        {} {}",
                    color.label("Framework:"),
                    if adapter.framework_detected {
                        "detected"
                    } else {
                        "not detected"
                    }
                );
                println!(
                    "        {} {}",
                    color.label("Driver:"),
                    if adapter.driver_available {
                        "available"
                    } else {
                        "missing"
                    }
                );
                println!(
                    "        {} {}",
                    color.label("State:"),
                    color.status(adapter_state_label(adapter))
                );
            }
        }
    }
}

fn scope_metadata_pairs(record: &ComponentRecord) -> Vec<(&'static str, String)> {
    let mut pairs = vec![
        ("active", record.active.to_string()),
        (
            "mutable_by_current_invocation",
            record.mutable_by_current_invocation.to_string(),
        ),
    ];
    if let Some(scope) = record.shadowed_by.as_deref() {
        pairs.push(("shadowed_by", scope.to_string()));
    }
    if let Some(path) = record.state_path.as_deref() {
        pairs.push(("state_path", path.to_string()));
    }
    pairs
}

fn render_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}

fn adapter_state_label(adapter: &AdapterSummaryRecord) -> &'static str {
    match (
        adapter.enabled,
        adapter.claim_status,
        adapter.source_status.as_deref(),
    ) {
        (_, Some(ClaimStatus::CleanupFailed), _) => "cleanup_failed",
        (true, _, Some("missing")) => "orphaned",
        (true, Some(ClaimStatus::Enabled), _) => "enabled",
        (true, None, _) => "enabled",
        (false, _, _) => "not enabled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::state_view::{ScopedStateRoot, StateScope, StateView};
    use crate::repo_config::RepoConfig;
    use crate::resolution::resolve_rpm_component_name;
    use anolisa_core::{
        FileOwner, HealthEntry, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
        OwnedFile, OwnedFileKind, SubscriptionScope,
    };
    use std::path::{Path, PathBuf};

    /// Build a system-mode FsLayout rooted under `prefix` and pre-create
    /// `bin_dir` so the path-safety guard in [`anolisa_core::check_owned_file`]
    /// has a canonical root to anchor on. Tests place owned files under
    /// `layout.bin_dir` to stay inside the ANOLISA-owned roots.
    fn test_layout(prefix: &Path) -> FsLayout {
        let layout = FsLayout::system(Some(prefix.to_path_buf()));
        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin_dir");
        layout
    }

    /// Convenience for tests that don't exercise integrity at all — they
    /// only care about projection/filtering and the layout will never be
    /// touched. Uses a throwaway prefix that we don't bother creating.
    fn dummy_layout() -> FsLayout {
        FsLayout::system(Some(PathBuf::from("/tmp/anolisa-status-tests-noop")))
    }

    /// Baseline component install record. Owned `files` default to empty
    /// so projection-only tests never touch the filesystem; integrity
    /// tests attach files explicitly before upserting.
    fn component_object(name: &str, version: &str, status: ObjectStatus) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: name.to_string(),
            version: version.to_string(),
            status,
            manifest_digest: Some("sha256:abc".to_string()),
            distribution_source: Some("builtin".to_string()),
            raw_package: None,
            install_backend: None,
            ownership: None,
            rpm_metadata: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-20260601-001".to_string()),
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

    fn scoped_status_view(user_state: InstalledState, system_state: InstalledState) -> StateView {
        let user_root = ScopedStateRoot {
            scope: StateScope::User,
            layout: FsLayout::user_with_overrides(
                PathBuf::from("/tmp/anolisa-home"),
                None,
                None,
                Some(PathBuf::from("/tmp/anolisa-user-state")),
                None,
                None,
            ),
            state_path: PathBuf::from("/tmp/anolisa-user-state/installed.toml"),
            writable: true,
            state: user_state,
        };
        let system_root = ScopedStateRoot {
            scope: StateScope::System,
            layout: FsLayout::system(Some(PathBuf::from("/tmp/anolisa-system"))),
            state_path: PathBuf::from("/tmp/anolisa-system-state/installed.toml"),
            writable: false,
            state: system_state,
        };
        StateView {
            writable: user_root.clone(),
            visible_roots: vec![user_root, system_root],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn user_status_view_includes_system_component() {
        let user_state = InstalledState::default();
        let mut system_state = InstalledState::default();
        system_state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        let view = scoped_status_view(user_state, system_state);

        let records = select_components_from_view(
            &view,
            &ScopedCatalogs::default(),
            Some("agentsight"),
            None,
            None,
        );

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "agentsight");
        assert_eq!(records[0].status, "installed");
        assert_eq!(records[0].scope, "system");
        assert!(records[0].active);
        assert!(!records[0].mutable_by_current_invocation);
        assert_eq!(
            records[0].state_path.as_deref(),
            Some("/tmp/anolisa-system-state/installed.toml")
        );
    }

    #[test]
    fn scoped_status_view_uses_catalog_from_record_root() {
        let user_home = tempfile::tempdir().expect("user home");
        let user_layout = FsLayout::user_with_overrides(
            user_home.path().join("home"),
            Some(user_home.path().join("data")),
            Some(user_home.path().join("config")),
            Some(user_home.path().join("state")),
            Some(user_home.path().join("cache")),
            Some(user_home.path().join("runtime")),
        );
        std::fs::create_dir_all(&user_layout.bin_dir).expect("user bin dir");

        let system_prefix = tempfile::tempdir().expect("system prefix");
        let system_layout = test_layout(system_prefix.path());
        let system_probe = system_layout.bin_dir.join("agentsight");
        std::fs::write(&system_probe, b"binary").expect("write system probe");

        let user_manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            user_layout.bin_dir.join("agentsight").display()
        );
        let system_manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            system_probe.display()
        );
        let (user_catalog, _user_catalog_guard) =
            catalog_with_component("agentsight", &user_manifest);
        let (system_catalog, _system_catalog_guard) =
            catalog_with_component("agentsight", &system_manifest);

        let user_state_path = user_layout.state_dir.join("installed.toml");
        let system_state_path = system_layout.state_dir.join("installed.toml");
        let user_root = ScopedStateRoot {
            scope: StateScope::User,
            layout: user_layout,
            state_path: user_state_path.clone(),
            writable: true,
            state: InstalledState::default(),
        };
        let mut system_state = InstalledState::default();
        system_state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        let system_root = ScopedStateRoot {
            scope: StateScope::System,
            layout: system_layout,
            state_path: system_state_path.clone(),
            writable: false,
            state: system_state,
        };
        let view = StateView {
            writable: user_root.clone(),
            visible_roots: vec![user_root, system_root],
            warnings: Vec::new(),
        };
        let catalogs = ScopedCatalogs::from_entries(vec![
            (user_state_path, user_catalog),
            (system_state_path, system_catalog),
        ]);

        let records = select_components_from_view(&view, &catalogs, Some("agentsight"), None, None);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].scope, "system");
        assert_eq!(records[0].status, "installed");
        let health = records[0]
            .health
            .iter()
            .find(|entry| entry.name == "agentsight:file:binary")
            .expect("system catalog health entry present");
        assert_eq!(health.status, "ok");
        assert!(
            health
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains(&system_probe.display().to_string())),
            "health reason must use system manifest path: {:?}",
            health.reason
        );
    }

    #[test]
    fn named_status_view_reports_shadowed_system_record() {
        let mut user_state = InstalledState::default();
        user_state.upsert_object(component_object(
            "agentsight",
            "0.2.0",
            ObjectStatus::Installed,
        ));
        let mut system_state = InstalledState::default();
        system_state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        let view = scoped_status_view(user_state, system_state);

        let records = select_components_from_view(
            &view,
            &ScopedCatalogs::default(),
            Some("agentsight"),
            None,
            None,
        );

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].scope, "user");
        assert!(records[0].active);
        assert_eq!(records[1].scope, "system");
        assert!(!records[1].active);
        assert_eq!(records[1].shadowed_by.as_deref(), Some("user"));
    }

    #[test]
    fn system_status_view_projects_only_visible_system_root() {
        let mut system_state = InstalledState::default();
        system_state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        let system_root = ScopedStateRoot {
            scope: StateScope::System,
            layout: FsLayout::system(Some(PathBuf::from("/tmp/anolisa-system"))),
            state_path: PathBuf::from("/tmp/anolisa-system-state/installed.toml"),
            writable: true,
            state: system_state,
        };
        let view = StateView {
            writable: system_root.clone(),
            visible_roots: vec![system_root],
            warnings: Vec::new(),
        };

        let records =
            select_components_from_view(&view, &ScopedCatalogs::default(), None, None, None);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "agentsight");
        assert_eq!(records[0].scope, "system");
    }

    /// A missing `installed.toml` is the fresh-install case and must
    /// surface as an empty result, not an error. Verifies the helper
    /// stack (`InstalledState::load` -> `select_components`) collapses
    /// "no file" to "no components".
    #[test]
    fn missing_state_file_yields_empty_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");
        let state = InstalledState::load(&path).expect("missing file is not an error");
        let records = select_components(&state, &dummy_layout(), None, "system", None, None);
        assert!(records.is_empty());
    }

    #[test]
    fn unfiltered_listing_returns_all_components() {
        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        state.upsert_object(component_object(
            "tokenless",
            "0.2.0",
            ObjectStatus::Partial,
        ));

        let records = select_components(&state, &dummy_layout(), None, "system", None, None);
        assert_eq!(records.len(), 2);
        let names: Vec<&str> = records.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"agentsight"));
        assert!(names.contains(&"tokenless"));
        // Partial maps to the wire-friendly `degraded` label.
        let tokenless = records
            .iter()
            .find(|r| r.name == "tokenless")
            .expect("present");
        assert_eq!(tokenless.status, "degraded");
    }

    #[test]
    fn filter_miss_yields_synthetic_not_installed_record() {
        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &dummy_layout(),
            None,
            "system",
            Some("ws-ckpt"),
            None,
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "ws-ckpt");
        assert_eq!(records[0].status, "not_installed");
        assert!(records[0].version.is_none());
        assert!(records[0].installed_at.is_none());
        assert!(records[0].last_operation_id.is_none());
        assert!(records[0].enabled_features.is_empty());
    }

    #[test]
    fn filter_hit_returns_stored_record() {
        let mut state = InstalledState::default();
        // No owned files -> integrity probe is a no-op so the
        // state-projected record passes through clean.
        let mut obj = component_object("agentsight", "0.3.1", ObjectStatus::Installed);
        obj.enabled_features = vec!["bpf-events".to_string()];
        obj.health = vec![HealthEntry {
            name: "binary".to_string(),
            status: "ok".to_string(),
            checked_at: "2026-06-01T10:01:00Z".to_string(),
            reason: None,
        }];
        state.upsert_object(obj);

        let records = select_components(
            &state,
            &dummy_layout(),
            None,
            "system",
            Some("agentsight"),
            None,
        );
        assert_eq!(records.len(), 1);
        let only = &records[0];
        assert_eq!(only.name, "agentsight");
        assert_eq!(only.status, "installed");
        assert_eq!(only.version.as_deref(), Some("0.3.1"));
        assert_eq!(only.installed_at.as_deref(), Some("2026-06-01T10:00:00Z"));
        assert_eq!(only.last_operation_id.as_deref(), Some("op-20260601-001"));
        // State-projected fields must reach the wire record verbatim.
        assert_eq!(only.enabled_features, vec!["bpf-events"]);
        assert_eq!(only.health.len(), 1);
        assert_eq!(only.health[0].name, "binary");
        assert_eq!(only.health[0].status, "ok");
    }

    /// Component whose owned files are all present on disk with matching
    /// sha256 stays `installed` and the wire record gains one
    /// `integrity:<path>` health entry per file with `status = "ok"`.
    #[test]
    fn integrity_probe_present_file_with_matching_sha_keeps_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let file_path = layout.bin_dir.join("agentsight");
        std::fs::write(&file_path, b"payload").expect("write");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: file_path.clone(),
            owner: FileOwner::Anolisa,
            sha256: Some(
                "239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5".to_string(),
            ),
            kind: OwnedFileKind::File,
            referent: None,
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"), None);
        let only = &records[0];
        assert_eq!(only.status, "installed");
        // Exactly one integrity entry, status "ok", with the path in the name.
        let integrity: Vec<&HealthEntry> = only
            .health
            .iter()
            .filter(|h| h.name.starts_with("integrity:"))
            .collect();
        assert_eq!(integrity.len(), 1);
        assert_eq!(integrity[0].status, "ok");
        assert!(integrity[0].name.ends_with("agentsight"));
    }

    /// Missing owned file on disk escalates the component status to
    /// `"failed"` and emits a `missing_file` health entry. The original
    /// `installed` ObjectStatus is NOT mutated — escalation is purely
    /// at the wire layer (`status` field).
    #[test]
    fn integrity_probe_missing_file_escalates_to_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let missing_path = layout.bin_dir.join("anolisa-integrity-missing");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: missing_path,
            owner: FileOwner::Anolisa,
            sha256: Some("deadbeef".to_string()),
            kind: OwnedFileKind::File,
            referent: None,
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"), None);
        let only = &records[0];
        assert_eq!(only.status, "failed", "missing file -> failed");
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(integrity.status, "missing_file");
    }

    /// Tampered file (sha256 mismatch) escalates to `"failed"` and
    /// emits a `sha256_mismatch` health entry — distinct from
    /// `missing_file` so the user can tell which kind of drift occurred.
    #[test]
    fn integrity_probe_sha_mismatch_escalates_to_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let file_path = layout.bin_dir.join("agentsight");
        std::fs::write(&file_path, b"tampered-payload").expect("write");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: file_path,
            owner: FileOwner::Anolisa,
            sha256: Some(
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            ),
            kind: OwnedFileKind::File,
            referent: None,
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"), None);
        let only = &records[0];
        assert_eq!(only.status, "failed", "sha mismatch -> failed");
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(integrity.status, "sha256_mismatch");
    }

    /// File exists but no sha256 was recorded -> degrade (not fail). We
    /// can't prove tampering either way; "degraded" signals the user
    /// should treat the install with skepticism without claiming it's
    /// broken.
    #[test]
    fn integrity_probe_unverified_file_degrades_status() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let file_path = layout.bin_dir.join("agentsight");
        std::fs::write(&file_path, b"payload").expect("write");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: file_path,
            owner: FileOwner::Anolisa,
            sha256: None,
            kind: OwnedFileKind::File,
            referent: None,
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"), None);
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(integrity.status, "unverified");
    }

    /// A disabled component MUST stay disabled even if its owned files
    /// are gone — `disabled` is a deliberate state set by the user, not
    /// a drift signal we should overwrite from a sha probe.
    #[test]
    fn integrity_probe_does_not_escalate_disabled_component() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let missing_path = layout.bin_dir.join("anolisa-integrity-still-disabled");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Disabled);
        comp.files = vec![OwnedFile {
            path: missing_path,
            owner: FileOwner::Anolisa,
            sha256: Some("deadbeef".to_string()),
            kind: OwnedFileKind::File,
            referent: None,
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"), None);
        let only = &records[0];
        assert_eq!(only.status, "disabled");
        // The integrity entry is still surfaced so users can see the drift,
        // even though the wire status stays disabled.
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(integrity.status, "missing_file");
    }

    /// A forged `installed.toml` pointing an `owner = anolisa` file at a
    /// path outside the ANOLISA-owned roots must be refused by `status`
    /// without any stat or read happening. We point at `/etc/shadow` —
    /// if the path-safety guard fell through, integrity would either
    /// open the file (worst case) or report `MissingFile` on a host where
    /// it doesn't exist. `out_of_bounds` is the only status that proves
    /// the guard fired before IO.
    #[test]
    fn integrity_probe_refuses_path_outside_owned_roots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: PathBuf::from("/etc/shadow"),
            owner: FileOwner::Anolisa,
            sha256: Some("deadbeef".to_string()),
            kind: OwnedFileKind::File,
            referent: None,
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"), None);
        let only = &records[0];
        assert_eq!(only.status, "failed", "out-of-bounds path -> failed");
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(
            integrity.status, "out_of_bounds",
            "path-safety guard must fire before any stat",
        );
    }

    // -----------------------------------------------------------------
    // Manifest health probe tests
    // -----------------------------------------------------------------

    /// Build a temporary catalog with a single component manifest under
    /// `runtime/<name>.toml`. Returns the Catalog plus the tempdir guard
    /// (dropping the guard wipes the manifests).
    fn catalog_with_component(
        name: &str,
        component_toml: &str,
    ) -> (anolisa_core::Catalog, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let runtime_dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&runtime_dir).expect("mkdir runtime");
        std::fs::write(runtime_dir.join(format!("{name}.toml")), component_toml)
            .expect("write component manifest");
        let catalog = anolisa_core::Catalog::load(anolisa_core::CatalogLayers::bundled_only(
            tmp.path().to_path_buf(),
        ))
        .expect("catalog loads");
        (catalog, tmp)
    }

    /// File-kind health check pointing at an existing file emits an `ok`
    /// entry with the path in the reason and leaves the wire status at
    /// `installed`.
    #[test]
    fn manifest_health_file_check_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe_path = layout.bin_dir.join("agentsight");
        std::fs::write(&probe_path, b"binary").expect("write probe binary");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            probe_path.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "installed");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(entry.status, "ok");
        assert!(
            entry
                .reason
                .as_deref()
                .unwrap_or("")
                .contains("file present"),
            "reason mentions presence: {:?}",
            entry.reason
        );
    }

    /// Required (default) file-kind check on a missing file escalates to
    /// `failed` and emits a `missing_file` entry with a reason.
    #[test]
    fn manifest_health_file_check_required_missing_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe_path = layout.bin_dir.join("ghost-binary");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            probe_path.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "failed", "required missing file -> failed");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(entry.status, "missing_file");
        assert!(entry.reason.is_some(), "reason must be populated");
    }

    /// Optional file-kind check on a missing file degrades (not fails).
    /// The same probe with `optional = true` must produce a degraded
    /// status — not a failure — proving the optional flag is consumed.
    #[test]
    fn manifest_health_file_check_optional_missing_degrades() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe_path = layout.bin_dir.join("ghost-binary");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
            optional = true
        "#,
            probe_path.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded", "optional missing file -> degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(entry.status, "missing_file");
    }

    /// Helper: write an executable shell script under `layout.bin_dir`.
    /// Manifest probes are required to point at executables under an
    /// ANOLISA-owned root, so tests that exercise the command path
    /// stage their probe scripts here rather than reaching for `/bin/true`.
    fn write_probe_script(layout: &FsLayout, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = layout.bin_dir.join(name);
        std::fs::write(&path, body).expect("write probe script");
        let mut perm = std::fs::metadata(&path).expect("stat").permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&path, perm).expect("chmod");
        path
    }

    /// Command-kind check that exits 0 stays `ok`. The probe is an
    /// owned executable under `{bindir}` — the only kind of command
    /// path-safety lets us run.
    #[test]
    fn manifest_health_command_check_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe = write_probe_script(&layout, "probe-ok", "#!/bin/sh\nexit 0\n");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "self-check"
            kind = "command"
            command = "{}"
        "#,
            probe.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "installed");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:self-check")
            .expect("command health entry present");
        assert_eq!(entry.status, "ok");
    }

    /// Required command-kind check that exits non-zero escalates to
    /// `failed` and surfaces the exit status in the reason.
    #[test]
    fn manifest_health_command_check_failure_escalates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe = write_probe_script(&layout, "probe-fail", "#!/bin/sh\nexit 1\n");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "self-check"
            kind = "command"
            command = "{}"
        "#,
            probe.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "failed");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:self-check")
            .expect("command health entry present");
        assert_eq!(entry.status, "command_failed");
    }

    /// File-kind check pointing outside the ANOLISA-owned roots must be
    /// refused as `out_of_bounds` and degrade the wire status — never
    /// stat the path. The component MUST NOT escalate to `failed`
    /// (a misauthored probe is a manifest bug, not an install bug) but
    /// must surface clearly in the wire output.
    #[test]
    fn manifest_health_file_check_refuses_external_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        // /etc/passwd is outside every ANOLISA root regardless of host.
        let manifest = r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "passwd"
            kind = "file"
            probe = "/etc/passwd"
        "#;
        let (catalog, _guard) = catalog_with_component("agentsight", manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(
            only.status, "degraded",
            "external probe path -> degraded, never failed",
        );
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:passwd")
            .expect("file health entry present");
        assert_eq!(entry.status, "out_of_bounds");
    }

    /// File-kind check whose `probe` resolves to a symlink must NOT
    /// follow the link — `unsupported_target` is the only safe answer.
    /// Test wires a link that stays under `bin_dir` so `validate_owned_path`
    /// passes and `symlink_metadata` is the guard that fires; the more
    /// dangerous link-to-outside case is already caught by path-safety
    /// (covered by `..._refuses_external_path`).
    #[test]
    #[cfg(unix)]
    fn manifest_health_file_check_refuses_symlink_target() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        // Real file + sibling symlink pointing at it. Both live under
        // bin_dir, so path-safety passes; the symlink is the only thing
        // the probe should refuse.
        let real = layout.bin_dir.join("probe-target");
        std::fs::write(&real, b"binary").expect("write target");
        let link = layout.bin_dir.join("probe-link");
        symlink(&real, &link).expect("symlink");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            link.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(entry.status, "unsupported_target");
    }

    /// File-kind check whose `probe` resolves to a directory (or any
    /// other non-regular file: fifo, socket, char/block device) must
    /// NOT return `ok`. Before the fix, `symlink_metadata` succeeded on
    /// a directory and the probe greenlit the install — turning a
    /// misauthored manifest (probe pointing at a parent directory
    /// instead of the binary) into a silent "everything is fine".
    /// `not_regular_file` makes the manifest bug visible while keeping
    /// the component `degraded` rather than `failed` (the install is
    /// fine; the manifest is wrong).
    #[test]
    fn manifest_health_file_check_refuses_directory_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        // Probe points at bin_dir itself — a real directory under an
        // ANOLISA-owned root. Path-safety passes; the regular-file
        // guard is the only thing left to refuse it.
        let target = layout.bin_dir.clone();
        std::fs::create_dir_all(&target).expect("mkdir target");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            target.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(
            only.status, "degraded",
            "directory target must escalate the component to degraded, not ok",
        );
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(
            entry.status, "not_regular_file",
            "directory probe must surface as not_regular_file, not ok",
        );
    }

    /// Command-kind check that names a bare or PATH-resolved executable
    /// must be refused — the probe must declare an absolute path under
    /// an ANOLISA-owned root. `true` is a builtin every shell ships;
    /// the only way it would have run before this fix was through `sh -c`.
    #[test]
    fn manifest_health_command_check_refuses_bare_executable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        let manifest = r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "bare"
            kind = "command"
            command = "true"
        "#;
        let (catalog, _guard) = catalog_with_component("agentsight", manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:bare")
            .expect("command health entry present");
        assert_eq!(entry.status, "out_of_bounds");
    }

    /// Command-kind check pointing at an absolute external executable
    /// (e.g. `/bin/true`) must be refused with `out_of_bounds`. The
    /// `validate_owned_path` guard fires on the executable, never letting
    /// us run a third-party binary on the user's behalf during status.
    #[test]
    fn manifest_health_command_check_refuses_external_absolute_executable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        let manifest = r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "host-true"
            kind = "command"
            command = "/bin/true"
        "#;
        let (catalog, _guard) = catalog_with_component("agentsight", manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:host-true")
            .expect("command health entry present");
        assert_eq!(entry.status, "out_of_bounds");
    }

    /// Command-kind check containing a shell metacharacter (pipe, redirect,
    /// `;`, …) must be refused. `status` runs probes WITHOUT a shell, so
    /// any probe that needs one is a misauthored manifest, not a runnable
    /// command.
    #[test]
    fn manifest_health_command_check_refuses_shell_metacharacters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        // Path is fine; the trailing `; rm -rf /` is what must trip the guard.
        let probe = write_probe_script(&layout, "probe-meta", "#!/bin/sh\nexit 0\n");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "metachar"
            kind = "command"
            command = "{} ; echo hax"
        "#,
            probe.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:metachar")
            .expect("command health entry present");
        assert_eq!(entry.status, "invalid_check");
    }

    /// systemd-kind check on a non-Linux / user-mode host degrades to
    /// `not_supported` rather than failing — we cannot prove the unit's
    /// state on a host without a service backend, but we don't have a
    /// positive failure either. user mode in particular short-circuits
    /// to NotSupported, which is a portable assertion across all CI
    /// platforms (linux, darwin, etc.).
    #[test]
    fn manifest_health_systemd_check_unsupported_degrades() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        let manifest = r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "service"
            kind = "systemd"
            unit = "agentsight.service"
        "#;
        let (catalog, _guard) = catalog_with_component("agentsight", manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        // user mode is the portable "no service backend" install_mode.
        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "user",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded", "unsupported -> degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:systemd:service")
            .expect("systemd health entry present");
        assert_eq!(entry.status, "not_supported");
    }

    /// Manifest health probes layer on top of integrity — a failed file
    /// integrity check stays `failed` even when every manifest check
    /// reports `ok`. Order of escalation: integrity is authoritative
    /// downward, manifest is authoritative for additional escalation.
    #[test]
    fn manifest_health_does_not_downgrade_failed_integrity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let missing_owned = layout.bin_dir.join("missing-binary");
        let probe = write_probe_script(&layout, "probe-ok-integ", "#!/bin/sh\nexit 0\n");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "self-check"
            kind = "command"
            command = "{}"
        "#,
            probe.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: missing_owned,
            owner: FileOwner::Anolisa,
            sha256: Some("deadbeef".to_string()),
            kind: OwnedFileKind::File,
            referent: None,
        }];
        state.upsert_object(comp);

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
            None,
        );
        let only = &records[0];
        assert_eq!(
            only.status, "failed",
            "integrity failure dominates over a clean manifest probe",
        );
        // Both entries must be present: integrity surfaced the missing
        // file, manifest surfaced the ok command.
        assert!(
            only.health
                .iter()
                .any(|h| h.name.starts_with("integrity:") && h.status == "missing_file"),
            "integrity entry present",
        );
        assert!(
            only.health
                .iter()
                .any(|h| h.name == "agentsight:command:self-check" && h.status == "ok"),
            "manifest entry present",
        );
    }

    // -----------------------------------------------------------------
    // Adapter summary tests
    // -----------------------------------------------------------------

    fn sample_scan_entry(component: &str, framework: &str, enabled: bool) -> ScanEntry {
        ScanEntry {
            component: component.to_string(),
            framework: framework.to_string(),
            declared: true,
            resource_root: Some(PathBuf::from(format!(
                "/usr/local/share/anolisa/adapters/{component}/{framework}"
            ))),
            driver_available: true,
            framework_detected: true,
            adapter_type: Some("plugin".to_string()),
            enabled,
            claim_status: if enabled {
                Some(ClaimStatus::Enabled)
            } else {
                None
            },
            source_status: enabled.then_some(AdapterSourceStatus::Available),
            source_reason: None,
        }
    }

    #[test]
    fn component_record_has_no_adapters_by_default() {
        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        let records = select_components(
            &state,
            &dummy_layout(),
            None,
            "system",
            Some("agentsight"),
            None,
        );
        assert!(records[0].adapters.is_empty());
    }

    #[test]
    fn adapter_summaries_filtered_to_requested_component() {
        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "tokenless",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let scan = vec![
            sample_scan_entry("tokenless", "openclaw", true),
            sample_scan_entry("agentsight", "openclaw", false),
        ];
        let records = select_components(
            &state,
            &dummy_layout(),
            None,
            "system",
            Some("tokenless"),
            Some(&scan),
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].adapters.len(), 1);
        assert_eq!(records[0].adapters[0].component, "tokenless");
        assert_eq!(records[0].adapters[0].framework, "openclaw");
        assert!(records[0].adapters[0].enabled);
        assert_eq!(
            records[0].adapters[0].claim_status,
            Some(ClaimStatus::Enabled)
        );
    }

    #[test]
    fn adapter_summaries_included_in_unfiltered_listing() {
        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "tokenless",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let scan = vec![sample_scan_entry("tokenless", "openclaw", true)];
        let records = select_components(&state, &dummy_layout(), None, "system", None, Some(&scan));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].adapters.len(), 1);
        assert_eq!(records[0].adapters[0].component, "tokenless");
    }

    #[test]
    fn synthetic_not_installed_record_has_no_adapters() {
        let state = InstalledState::default();
        let scan = vec![sample_scan_entry("ghost", "openclaw", false)];
        let records = select_components(
            &state,
            &dummy_layout(),
            None,
            "system",
            Some("ghost"),
            Some(&scan),
        );
        assert_eq!(records[0].status, "not_installed");
        assert!(records[0].adapters.is_empty());
    }

    #[test]
    fn adapter_summary_json_serialization() {
        let record = AdapterSummaryRecord {
            component: "tokenless".to_string(),
            framework: "openclaw".to_string(),
            declared: true,
            resource_present: true,
            resource_root: Some("/data/adapters/tokenless/openclaw".to_string()),
            driver_available: true,
            framework_detected: true,
            enabled: true,
            claim_status: Some(ClaimStatus::Enabled),
            source_status: Some("available".to_string()),
            source_reason: None,
        };
        let json = serde_json::to_value(&record).expect("serialize");
        assert_eq!(json["component"], "tokenless");
        assert_eq!(json["framework"], "openclaw");
        assert_eq!(json["declared"], true);
        assert_eq!(json["resource_present"], true);
        assert_eq!(json["driver_available"], true);
        assert_eq!(json["framework_detected"], true);
        assert_eq!(json["enabled"], true);
        assert_eq!(json["claim_status"], "enabled");
    }

    #[test]
    fn adapter_summary_skips_empty_adapters_in_json() {
        let record = ComponentRecord {
            name: "agentsight".to_string(),
            status: "installed".to_string(),
            scope: "system".to_string(),
            active: true,
            mutable_by_current_invocation: true,
            shadowed_by: None,
            state_path: Some("/tmp/anolisa-system-state/installed.toml".to_string()),
            version: Some("0.1.0".to_string()),
            installed_at: Some("2026-06-01T10:00:00Z".to_string()),
            last_operation_id: None,
            enabled_features: Vec::new(),
            health: Vec::new(),
            adapters: Vec::new(),
            rpm_package: None,
            rpm_evr: None,
            rpm_source_repo: None,
            provisioned_packages: Vec::new(),
        };
        let json = serde_json::to_value(&record).expect("serialize");
        assert!(
            json.get("adapters").is_none(),
            "empty adapters must be omitted from JSON"
        );
        assert!(
            json.get("rpm_package").is_none(),
            "empty rpm fields must be omitted from JSON"
        );
    }

    #[test]
    fn scope_metadata_pairs_expose_mutability_and_state_path() {
        let record = ComponentRecord {
            name: "agentsight".to_string(),
            status: "installed".to_string(),
            scope: "system".to_string(),
            active: false,
            mutable_by_current_invocation: false,
            shadowed_by: Some("user".to_string()),
            state_path: Some("/tmp/anolisa-system-state/installed.toml".to_string()),
            version: Some("0.1.0".to_string()),
            installed_at: Some("2026-06-01T10:00:00Z".to_string()),
            last_operation_id: None,
            enabled_features: Vec::new(),
            health: Vec::new(),
            adapters: Vec::new(),
            rpm_package: None,
            rpm_evr: None,
            rpm_source_repo: None,
            provisioned_packages: Vec::new(),
        };

        let pairs = scope_metadata_pairs(&record);

        assert!(pairs.contains(&("active", "false".to_string())));
        assert!(pairs.contains(&("mutable_by_current_invocation", "false".to_string())));
        assert!(pairs.contains(&("shadowed_by", "user".to_string())));
        assert!(pairs.contains(&(
            "state_path",
            "/tmp/anolisa-system-state/installed.toml".to_string()
        )));
    }

    #[test]
    fn adapter_state_label_values() {
        let base = AdapterSummaryRecord {
            component: "x".to_string(),
            framework: "y".to_string(),
            declared: true,
            resource_present: true,
            resource_root: None,
            driver_available: true,
            framework_detected: true,
            enabled: true,
            claim_status: Some(ClaimStatus::Enabled),
            source_status: Some("available".to_string()),
            source_reason: None,
        };

        assert_eq!(adapter_state_label(&base), "enabled");

        let mut cleanup = base.clone();
        cleanup.claim_status = Some(ClaimStatus::CleanupFailed);
        assert_eq!(adapter_state_label(&cleanup), "cleanup_failed");

        let mut enabled_no_claim = base.clone();
        enabled_no_claim.claim_status = None;
        assert_eq!(adapter_state_label(&enabled_no_claim), "enabled");

        let mut not_enabled = base.clone();
        not_enabled.enabled = false;
        not_enabled.source_status = None;
        assert_eq!(adapter_state_label(&not_enabled), "not enabled");

        let mut orphaned = base.clone();
        orphaned.source_status = Some("missing".to_string());
        orphaned.source_reason = Some("no visible installed component".to_string());
        assert_eq!(adapter_state_label(&orphaned), "orphaned");
    }

    // ── rpm-observed status (#958) ──────────────────────────────────

    use anolisa_core::{Ownership, RpmMetadata};
    use anolisa_platform::pkg_query::{PackageInfo, PackageQueryError, PackageVersion};

    /// An adopted `rpm-observed` component record.
    fn rpm_observed_object(name: &str, package: &str, evr: &str) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: name.to_string(),
            version: evr.to_string(),
            status: ObjectStatus::Adopted,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(Ownership::RpmObserved),
            rpm_metadata: Some(RpmMetadata {
                package_name: package.to_string(),
                evr: Some(evr.to_string()),
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            }),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-adopt-001".to_string()),
            managed: false,
            adopted: true,
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

    /// P2 gate: an rpm-observed row must keep `adopted` even when a manifest
    /// health check would fail a raw install — ANOLISA owns none of its files
    /// and never laid out the raw tree, so those checks do not apply (§8).
    #[test]
    fn rpm_observed_row_skips_manifest_health_escalation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe_path = layout.bin_dir.join("ghost-binary");
        let manifest = format!(
            r#"
            [component]
            name = "copilot-shell"
            version = "2.3.0-1.al8"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            probe_path.display()
        );
        let (catalog, _guard) = catalog_with_component("copilot-shell", &manifest);

        // Control: a raw install with the same failing check escalates.
        let mut raw_state = InstalledState::default();
        raw_state.upsert_object(component_object(
            "copilot-shell",
            "2.3.0",
            ObjectStatus::Installed,
        ));
        let raw = select_components(
            &raw_state,
            &layout,
            Some(&catalog),
            "system",
            Some("copilot-shell"),
            None,
        );
        assert_eq!(raw[0].status, "failed", "raw install must escalate");

        // rpm-observed with the same catalog stays adopted and surfaces the
        // RPM provenance fields.
        let mut obs_state = InstalledState::default();
        obs_state.upsert_object(rpm_observed_object(
            "copilot-shell",
            "copilot-shell",
            "2.3.0-1.al8",
        ));
        let obs = select_components(
            &obs_state,
            &layout,
            Some(&catalog),
            "system",
            Some("copilot-shell"),
            None,
        );
        assert_eq!(obs[0].status, "adopted", "rpm-observed must not escalate");
        assert_eq!(obs[0].rpm_package.as_deref(), Some("copilot-shell"));
        assert_eq!(obs[0].rpm_evr.as_deref(), Some("2.3.0-1.al8"));
        assert_eq!(obs[0].rpm_source_repo.as_deref(), Some("@System"));
    }

    #[test]
    fn rpm_managed_row_skips_raw_layout_health_probe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let state_path = layout.state_dir.join("installed.toml");
        let manifest = r#"
            [component]
            name = "cosh"
            version = "2.3.0-1.al8"

            [[health_checks]]
            name = "launcher"
            kind = "command"
            command = "{bindir}/cosh --version"
        "#;
        let (catalog, _guard) = catalog_with_component("cosh", manifest);
        let catalogs = ScopedCatalogs::from_entries(vec![(state_path.clone(), catalog)]);
        let query = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.3.0", "1.al8"),
            )],
            origins: Vec::new(),
        };
        let project = |object: InstalledObject| {
            let mut state = InstalledState::default();
            state.upsert_object(object);
            let root = ScopedStateRoot {
                scope: StateScope::System,
                layout: layout.clone(),
                state_path: state_path.clone(),
                writable: true,
                state,
            };
            let view = StateView {
                writable: root.clone(),
                visible_roots: vec![root],
                warnings: Vec::new(),
            };
            select_components_from_view(&view, &catalogs, Some("cosh"), None, Some(&query))
        };

        let mut rpm_managed = component_object("cosh", "2.3.0-1.al8", ObjectStatus::Installed);
        rpm_managed.install_backend = Some("rpm".to_string());
        rpm_managed.ownership = Some(Ownership::RpmManaged);
        rpm_managed.rpm_metadata = Some(rpm_meta("copilot-shell", "2.3.0-1.al8"));
        let rpm_records = project(rpm_managed);
        let rpm = &rpm_records[0];

        assert_eq!(rpm.status, "installed");
        assert!(
            !rpm.health
                .iter()
                .any(|entry| entry.name == "cosh:command:launcher"),
            "RPM rows must not run raw-layout manifest commands",
        );
        assert!(
            !rpm.health
                .iter()
                .any(|entry| entry.status == "command_error"),
            "RPM rows must not report raw-layout command failures",
        );
        assert!(
            !rpm.health.iter().any(|entry| entry.name == "rpm:drift"),
            "matching rpmdb EVR must remain installed without drift",
        );

        let mut raw_managed = component_object("cosh", "2.3.0-1.al8", ObjectStatus::Installed);
        raw_managed.install_backend = Some("raw".to_string());
        raw_managed.ownership = Some(Ownership::RawManaged);
        let raw_records = project(raw_managed);
        let raw = &raw_records[0];

        assert_eq!(raw.status, "failed");
        let launcher = raw
            .health
            .iter()
            .find(|entry| entry.name == "cosh:command:launcher")
            .expect("raw-managed launcher health entry");
        assert_eq!(launcher.status, "command_error");
    }

    /// Configurable [`PackageQuery`] for the observed-probe tests.
    #[derive(Default)]
    struct FakeQuery {
        installed: Vec<(String, PackageInfo)>,
        origins: Vec<(String, String)>,
    }

    impl PackageQuery for FakeQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            Ok(self
                .installed
                .iter()
                .find(|(n, _)| n == package)
                .map(|(_, i)| i.clone()))
        }
        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }
        fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
            Ok(self
                .origins
                .iter()
                .find(|(n, _)| n == package)
                .map(|(_, r)| r.clone()))
        }
        fn provided_capabilities_installed(
            &self,
            package: &str,
        ) -> Result<Vec<String>, PackageQueryError> {
            if self.installed.iter().any(|(n, _)| n == package) {
                Ok(vec![format!("anolisa-component({package})")])
            } else {
                Ok(Vec::new())
            }
        }
    }

    fn pkg_info(name: &str, version: &str, release: &str) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: PackageVersion {
                epoch: None,
                version: version.to_string(),
                release: Some(release.to_string()),
            },
            arch: "x86_64".to_string(),
            origin: None,
        }
    }

    #[test]
    fn status_lookup_name_uses_component_index_alias_before_state_selection() {
        let idx = ComponentIndex::from_toml_str(
            r#"
schema_version = 1

[[components]]
name = "cosh"

[[components.backends]]
kind = "rpm"
package = "copilot-shell"
legacy_adopt = true

[[components.aliases]]
kind = "rpm-package"
name = "copilot-shell"
"#,
            "components.toml",
        )
        .expect("component index");
        let q = FakeQuery::default();

        assert_eq!(
            resolve_rpm_component_name(
                "copilot-shell",
                None,
                Some(&idx),
                &q,
                ResolutionUse::StatusObserved,
            )
            .unwrap_or_else(|| "copilot-shell".to_string()),
            "cosh"
        );

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "cosh",
            "2.6.0-1.alnx4",
            ObjectStatus::Installed,
        ));
        let resolved = resolve_rpm_component_name(
            "copilot-shell",
            None,
            Some(&idx),
            &q,
            ResolutionUse::StatusObserved,
        )
        .unwrap_or_else(|| "copilot-shell".to_string());
        let records = select_components(
            &state,
            &dummy_layout(),
            None,
            "system",
            Some(&resolved),
            None,
        );

        assert_eq!(records[0].name, "cosh");
        assert_eq!(records[0].status, "installed");
    }

    #[test]
    fn observed_record_reports_installed_default_name() {
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.3.0", "1.al8"),
            )],
            origins: vec![("copilot-shell".to_string(), "@System".to_string())],
        };
        let rec = observed_record("copilot-shell", None, None, &q).expect("observed");
        assert_eq!(rec.status, "observed");
        assert_eq!(rec.rpm_package.as_deref(), Some("copilot-shell"));
        assert_eq!(rec.rpm_evr.as_deref(), Some("2.3.0-1.al8"));
        assert_eq!(rec.rpm_source_repo.as_deref(), Some("@System"));
        assert_eq!(rec.version.as_deref(), Some("2.3.0-1.al8"));
    }

    #[test]
    fn observed_record_none_when_not_installed() {
        let q = FakeQuery::default();
        assert!(observed_record("copilot-shell", None, None, &q).is_none());
    }

    #[test]
    fn observed_record_honors_package_map() {
        // package_map renames the component's RPM; the probe must query the
        // mapped name, not the default.
        let repo = RepoConfig::from_toml_str(
            "schema_version = 1\ndefault_backend = \"rpm\"\n[backends.rpm]\nbase_url = \"https://e/x\"\n[backends.rpm.package_map]\ncopilot-shell = \"site-copilot\"\n",
        )
        .expect("repo");
        let backend = repo.backends.get("rpm");
        let q = FakeQuery {
            installed: vec![(
                "site-copilot".to_string(),
                pkg_info("site-copilot", "9.9", "1"),
            )],
            origins: Vec::new(),
        };
        let rec = observed_record("copilot-shell", backend, None, &q).expect("observed");
        assert_eq!(rec.rpm_package.as_deref(), Some("site-copilot"));
        assert_eq!(rec.rpm_source_repo, None);
    }

    // ── rpm drift adjudication (#960) ───────────────────────────────

    /// Query whose `query_installed` always returns a preset anomalous error,
    /// to exercise the drift classification of the non-`Ok` branches.
    struct ErrQuery(PackageQueryError);

    impl PackageQuery for ErrQuery {
        fn query_installed(&self, _: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            Err(match &self.0 {
                PackageQueryError::UnexpectedOutput { command, detail } => {
                    PackageQueryError::UnexpectedOutput {
                        command: command.clone(),
                        detail: detail.clone(),
                    }
                }
                _ => PackageQueryError::CommandMissing {
                    command: "rpm".to_string(),
                },
            })
        }
        fn query_available(&self, _: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }
    }

    fn rpm_meta(package: &str, evr: &str) -> RpmMetadata {
        RpmMetadata {
            package_name: package.to_string(),
            evr: Some(evr.to_string()),
            arch: Some("x86_64".to_string()),
            source_repo: Some("@System".to_string()),
        }
    }

    /// rpmdb EVR matching the recorded one is not drift.
    #[test]
    fn probe_rpm_drift_none_when_evr_matches() {
        let meta = rpm_meta("copilot-shell", "2.3.0-1.al8");
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.3.0", "1.al8"),
            )],
            origins: Vec::new(),
        };
        assert!(probe_rpm_drift(&meta, &q).is_none());
    }

    /// A newer rpmdb EVR than recorded (manual `dnf update`) is drift.
    #[test]
    fn probe_rpm_drift_detects_evr_mismatch() {
        let meta = rpm_meta("copilot-shell", "2.2.0-1.al8");
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.3.0", "1.al8"),
            )],
            origins: Vec::new(),
        };
        assert!(matches!(
            probe_rpm_drift(&meta, &q),
            Some(RpmDrift::Drifted { .. })
        ));
    }

    /// The package gone from rpmdb (manual `rpm -e`) is Missing.
    #[test]
    fn probe_rpm_drift_detects_missing() {
        let meta = rpm_meta("copilot-shell", "2.2.0-1.al8");
        let q = FakeQuery::default();
        assert!(matches!(
            probe_rpm_drift(&meta, &q),
            Some(RpmDrift::Missing)
        ));
    }

    /// A same-name multi-version rpmdb is surfaced as drift, not a silent pass.
    #[test]
    fn probe_rpm_drift_multi_version_is_drifted() {
        let meta = rpm_meta("copilot-shell", "2.2.0-1.al8");
        let q = ErrQuery(PackageQueryError::UnexpectedOutput {
            command: "rpm".to_string(),
            detail: "2 installed versions".to_string(),
        });
        assert!(matches!(
            probe_rpm_drift(&meta, &q),
            Some(RpmDrift::Drifted { .. })
        ));
    }

    /// Missing rpm/dnf tooling cannot prove drift; the recorded status stands.
    #[test]
    fn probe_rpm_drift_tooling_missing_keeps_status() {
        let meta = rpm_meta("copilot-shell", "2.2.0-1.al8");
        let q = ErrQuery(PackageQueryError::CommandMissing {
            command: "rpm".to_string(),
        });
        assert!(probe_rpm_drift(&meta, &q).is_none());
    }

    /// The scoped status selector overrides an adopted rpm-observed row to
    /// `drifted` and records a `rpm:drift` health entry when rpmdb has moved on.
    #[test]
    fn select_components_from_view_overrides_rpm_status_to_drifted() {
        let mut state = InstalledState::default();
        state.upsert_object(rpm_observed_object(
            "copilot-shell",
            "copilot-shell",
            "2.2.0-1.al8",
        ));
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.3.0", "1.al8"),
            )],
            origins: Vec::new(),
        };
        let view = scoped_status_view(InstalledState::default(), state);
        let records = select_components_from_view(
            &view,
            &ScopedCatalogs::default(),
            Some("copilot-shell"),
            None,
            Some(&q),
        );

        assert_eq!(records[0].status, "drifted");
        assert!(
            records[0]
                .health
                .iter()
                .any(|h| h.name == "rpm:drift" && h.status == "drifted"),
            "a rpm:drift health entry must be recorded",
        );
    }

    /// The scoped status selector overrides to `missing` when rpmdb no longer
    /// has the package (the `rpm -e` case must not be silently reinstalled).
    #[test]
    fn select_components_from_view_overrides_rpm_status_to_missing() {
        let mut state = InstalledState::default();
        state.upsert_object(rpm_observed_object(
            "copilot-shell",
            "copilot-shell",
            "2.2.0-1.al8",
        ));
        let q = FakeQuery::default();
        let view = scoped_status_view(InstalledState::default(), state);
        let records = select_components_from_view(
            &view,
            &ScopedCatalogs::default(),
            Some("copilot-shell"),
            None,
            Some(&q),
        );

        assert_eq!(records[0].status, "missing");
        assert!(
            records[0]
                .health
                .iter()
                .any(|h| h.name == "rpm:drift" && h.status == "missing"),
        );
    }

    /// A raw component (no RPM metadata) is never touched by drift adjudication,
    /// even when the injected query would report the package absent.
    #[test]
    fn select_components_from_view_leaves_non_rpm_component_untouched() {
        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        let q = FakeQuery::default();
        let view = scoped_status_view(InstalledState::default(), state);
        let records = select_components_from_view(
            &view,
            &ScopedCatalogs::default(),
            Some("agentsight"),
            None,
            Some(&q),
        );

        assert_eq!(records[0].status, "installed", "raw row must not drift");
        assert!(
            !records[0].health.iter().any(|h| h.name == "rpm:drift"),
            "no drift entry for a non-RPM component",
        );
    }

    /// Drift never demotes a record that integrity/manifest health already
    /// escalated past a clean projection: a `failed` RPM row keeps its
    /// more-severe status even when rpmdb has moved on, and gains no drift entry.
    #[test]
    fn select_components_from_view_keeps_escalated_rpm_status() {
        let mut state = InstalledState::default();
        let mut object = rpm_observed_object("copilot-shell", "copilot-shell", "2.2.0-1.al8");
        object.status = ObjectStatus::Failed;
        state.upsert_object(object);
        // rpmdb has drifted, but the failed status must survive untouched.
        let q = FakeQuery {
            installed: vec![(
                "copilot-shell".to_string(),
                pkg_info("copilot-shell", "2.3.0", "1.al8"),
            )],
            origins: Vec::new(),
        };
        let view = scoped_status_view(InstalledState::default(), state);
        let records = select_components_from_view(
            &view,
            &ScopedCatalogs::default(),
            Some("copilot-shell"),
            None,
            Some(&q),
        );

        assert_eq!(
            records[0].status, "failed",
            "escalated status must not be demoted to drifted",
        );
        assert!(
            !records[0].health.iter().any(|h| h.name == "rpm:drift"),
            "no drift entry when the record is already escalated",
        );
    }
}
