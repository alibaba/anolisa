//! `anolisa doctor [COMPONENT]` — read-only component diagnostics.
//!
//! Doctor layers actionable findings and remediation suggestions on top of the
//! same state, health, runtime-dependency, and rpmdb probes used by the rest of
//! the CLI. It does not mutate host state; `--fix` is reserved until a repair
//! executor exists.

use std::collections::BTreeSet;

use anolisa_core::{
    Catalog, CheckEnv, CheckOutcome, CheckSpec, CheckStatus, ComponentManifest, DependencyKind,
    DependencyResolution, DependencyResolver, DependencyStatus, HealthEntry, InstalledObject,
    ObjectKind, ObjectStatus, ResolverEnv, ServiceManager, ServiceRef, ServiceScope, ServiceState,
    run_check, service_for_install_mode, user_service_for_install_mode,
};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::rpm_query::RpmPackageQuery;
use clap::Parser;
use serde::Serialize;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::tier1::status::{self, ComponentRecord, RpmDrift};
use crate::context::CliContext;
use crate::response::{CliError, render_json_with_status};

const COMMAND: &str = "doctor";

#[derive(Parser)]
pub struct DoctorArgs {
    /// Diagnose a specific component (default: all installed)
    pub component: Option<String>,
    /// Apply suggested fixes automatically.
    ///
    /// `doctor` with no `--fix` is read-only. `--fix` executes the fix
    /// plan inside a transaction. Combining `--dry-run --fix` is
    /// rejected as `INVALID_ARGUMENT`: `--dry-run` alone already shows
    /// the diagnostic plan; `--fix` is the explicit "execute" verb.
    #[arg(long)]
    pub fix: bool,
}

#[derive(Debug, Serialize)]
struct DoctorPayload {
    summary: DoctorSummary,
    components: Vec<DoctorComponent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct DoctorSummary {
    components_checked: usize,
    ok: usize,
    degraded: usize,
    failed: usize,
    findings: usize,
}

#[derive(Debug, Serialize)]
struct DoctorComponent {
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    findings: Vec<DoctorFinding>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    health_checks: Vec<DoctorHealthCheck>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<DoctorDependency>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fix_plan: Vec<FixSuggestion>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DoctorFinding {
    severity: FindingSeverity,
    code: String,
    message: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum FindingSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DoctorHealthCheck {
    name: String,
    status: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DoctorDependency {
    name: String,
    kind: DependencyKind,
    status: DoctorDependencyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DoctorDependencyStatus {
    Resolved,
    Unresolved,
    Unresolvable,
    Skipped,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct FixSuggestion {
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    reason: String,
    automatic: bool,
}

struct DoctorProbeContext<'a> {
    layout: &'a FsLayout,
    resolver_env: &'a ResolverEnv,
    rpm_query: &'a RpmPackageQuery,
    system_service: &'a dyn ServiceManager,
    user_service: &'a dyn ServiceManager,
    dry_run: bool,
}

pub fn handle(args: DoctorArgs, ctx: &CliContext) -> Result<(), CliError> {
    let command = match &args.component {
        Some(comp) => format!("doctor {comp}"),
        None => COMMAND.to_string(),
    };

    if ctx.dry_run && args.fix {
        return Err(CliError::InvalidArgument {
            command,
            reason: "--dry-run --fix is invalid; --dry-run alone prints fix plan, --fix executes"
                .to_string(),
        });
    }
    if args.fix {
        return Err(CliError::not_implemented_with_hint(
            command,
            "doctor is read-only in this release; rerun without --fix to inspect fix_plan suggestions",
        ));
    }

    let payload = diagnose(args.component.as_deref(), ctx)?;
    let has_issues = payload.summary.failed > 0 || payload.summary.degraded > 0;
    render_doctor(ctx, &payload, !has_issues)?;
    if has_issues {
        return Err(CliError::DiagnosticsFound {
            command: COMMAND.to_string(),
        });
    }
    Ok(())
}

fn diagnose(component: Option<&str>, ctx: &CliContext) -> Result<DoctorPayload, CliError> {
    let state = common::load_installed_state(ctx, COMMAND)?;
    let layout = common::resolve_layout(ctx);
    let mut warnings = Vec::new();
    let catalog = match common::load_bundled_catalog(ctx, COMMAND) {
        Ok(catalog) => Some(catalog),
        Err(err) => {
            warnings.push(format!("catalog unavailable: {}", err.reason()));
            None
        }
    };

    let rpm_query = RpmPackageQuery::system();
    let component = component.map(|name| common::lookup_component_name(name, &state, ctx, COMMAND));

    let status_catalog = if ctx.dry_run { None } else { catalog.as_ref() };
    let records = status::select_components(
        &state,
        &layout,
        status_catalog,
        ctx.install_mode.as_str(),
        component.as_deref(),
        None,
    );
    let env = anolisa_env::EnvService::detect();
    let resolver_env = resolver_env_from_facts(&env);
    let system_service = service_for_install_mode(ctx.install_mode.as_str(), &env);
    let user_service = user_service_for_install_mode(ctx.install_mode.as_str(), &env);
    let probe_ctx = DoctorProbeContext {
        layout: &layout,
        resolver_env: &resolver_env,
        rpm_query: &rpm_query,
        system_service: system_service.as_ref(),
        user_service: user_service.as_ref(),
        dry_run: ctx.dry_run,
    };

    let mut components = Vec::new();
    for mut record in records {
        let object = state.find_object(ObjectKind::Component, &record.name);
        normalize_rpm_record(&mut record, object);
        let (manifest, manifest_warning) = if object.is_some() {
            resolve_component_manifest(&layout, catalog.as_ref(), &record.name)
        } else {
            (None, None)
        };
        let component = diagnose_component(
            record,
            object,
            manifest.as_ref(),
            manifest_warning,
            &probe_ctx,
        );
        components.push(component);
    }

    let summary = summarize(&components);
    Ok(DoctorPayload {
        summary,
        components,
        warnings,
        dry_run: ctx.dry_run,
    })
}

fn diagnose_component(
    record: ComponentRecord,
    object: Option<&InstalledObject>,
    manifest: Option<&ComponentManifest>,
    manifest_warning: Option<String>,
    probe_ctx: &DoctorProbeContext<'_>,
) -> DoctorComponent {
    let mut out = DoctorComponent {
        name: record.name.clone(),
        status: "ok".to_string(),
        state_status: Some(
            object
                .map(|object| common::object_status_str(object.status).to_string())
                .unwrap_or_else(|| record.status.clone()),
        ),
        version: record.version.clone(),
        findings: Vec::new(),
        health_checks: Vec::new(),
        dependencies: Vec::new(),
        fix_plan: Vec::new(),
    };

    add_state_finding(&record, object, &mut out);
    add_health_entries(&record, &mut out);
    add_manifest_warning(manifest_warning, object, &mut out);
    add_structured_health(manifest, object, probe_ctx, &mut out);
    add_service_refs(manifest, object, probe_ctx, &mut out);
    add_runtime_dependencies(
        manifest,
        object,
        probe_ctx.resolver_env,
        probe_ctx.dry_run,
        &mut out,
    );
    add_rpm_drift(object, probe_ctx.rpm_query, &mut out);

    dedupe_fix_plan(&mut out.fix_plan);
    out.status = component_status(&out);
    out
}

fn normalize_rpm_record(record: &mut ComponentRecord, object: Option<&InstalledObject>) {
    let Some(object) = object else {
        return;
    };
    if !object.effective_ownership().is_rpm() {
        return;
    }
    // `status`' legacy manifest probes expand `{bindir}` through the raw
    // `/usr/local` layout. RPM packages own their files under distro paths, so
    // doctor must not treat those raw-layout probe results as RPM health.
    record
        .health
        .retain(|entry| !entry.name.starts_with(&format!("{}:", record.name)));
    record.status = crate::commands::common::object_status_str(object.status).to_string();
}

fn add_state_finding(
    record: &ComponentRecord,
    object: Option<&InstalledObject>,
    out: &mut DoctorComponent,
) {
    let status = object
        .map(|object| common::object_status_str(object.status))
        .unwrap_or(record.status.as_str());
    match status {
        "installed" | "adopted" | "disabled" => {}
        "not_installed" => {
            out.findings.push(finding(
                FindingSeverity::Error,
                "component_not_installed",
                format!("component '{}' is not installed", record.name),
                "state",
                None,
            ));
            out.fix_plan.push(suggestion(
                "install_component",
                Some(format!("anolisa install {}", record.name)),
                "install the component before running component-level diagnostics",
            ));
        }
        "degraded" => out.findings.push(finding(
            FindingSeverity::Warning,
            "component_degraded",
            format!(
                "component '{}' is marked degraded in ANOLISA state",
                record.name
            ),
            "state",
            None,
        )),
        "failed" => out.findings.push(finding(
            FindingSeverity::Error,
            "component_failed",
            format!(
                "component '{}' is marked failed in ANOLISA state",
                record.name
            ),
            "state",
            None,
        )),
        other => out.findings.push(finding(
            FindingSeverity::Warning,
            "component_status_attention",
            format!("component '{}' has status '{other}'", record.name),
            "state",
            None,
        )),
    }
}

fn add_health_entries(record: &ComponentRecord, out: &mut DoctorComponent) {
    for entry in &record.health {
        out.health_checks.push(health_from_entry(entry));
        let Some(severity) = severity_for_health_status(&entry.status) else {
            continue;
        };
        out.findings.push(finding(
            severity,
            format!("health_{}", sanitize_code(&entry.status)),
            format!("health check '{}' reported '{}'", entry.name, entry.status),
            "health",
            entry.reason.clone(),
        ));
        out.fix_plan.extend(suggestions_for_health(
            &record.name,
            &entry.name,
            &entry.status,
        ));
    }
}

fn add_manifest_warning(
    warning: Option<String>,
    object: Option<&InstalledObject>,
    out: &mut DoctorComponent,
) {
    let Some(warning) = warning else {
        return;
    };
    let rpm_backed = object
        .map(|object| object.effective_ownership().is_rpm())
        .unwrap_or(false);
    if rpm_backed && warning.starts_with("component contract unavailable") {
        return;
    }
    let fix = if rpm_backed {
        suggestion(
            "publish_component_contract",
            None,
            "include an ANOLISA component contract in the RPM package for full diagnostics",
        )
    } else {
        suggestion(
            "reinstall_component",
            Some(format!("anolisa install {}", out.name)),
            "restore the installed component contract snapshot",
        )
    };
    out.findings.push(finding(
        FindingSeverity::Warning,
        "manifest_unavailable",
        "component manifest could not be loaded for full diagnostics",
        "manifest",
        Some(warning),
    ));
    out.fix_plan.push(fix);
}

fn add_structured_health(
    manifest: Option<&ComponentManifest>,
    object: Option<&InstalledObject>,
    probe_ctx: &DoctorProbeContext<'_>,
    out: &mut DoctorComponent,
) {
    let Some(object) = object else {
        return;
    };
    let Some(manifest) = manifest else {
        return;
    };
    if object.effective_ownership().is_rpm() {
        out.health_checks.push(DoctorHealthCheck {
            name: "component.health_check".to_string(),
            status: "skipped".to_string(),
            source: "structured_health".to_string(),
            detail: Some(
                "RPM components are verified through rpmdb; raw-layout health probes are skipped"
                    .to_string(),
            ),
            checked_at: None,
        });
        return;
    }
    let Some(spec) = manifest.health_spec() else {
        return;
    };
    let skip_active_service_probe = object.status == ObjectStatus::Disabled;
    let outcome = run_doctor_check(&spec, Some(manifest), probe_ctx, skip_active_service_probe);
    let component = out.name.clone();
    add_check_outcome(&component, &outcome, out);
}

fn run_doctor_check(
    spec: &CheckSpec,
    manifest: Option<&ComponentManifest>,
    probe_ctx: &DoctorProbeContext<'_>,
    skip_active_service_probe: bool,
) -> CheckOutcome {
    match spec {
        CheckSpec::AllOf { checks, .. } => {
            let children: Vec<CheckOutcome> = checks
                .iter()
                .map(|child| {
                    run_doctor_check(child, manifest, probe_ctx, skip_active_service_probe)
                })
                .collect();
            CheckOutcome {
                spec_label: format!("all_of ({} checks)", checks.len()),
                status: all_of_status(&children),
                detail: None,
                children,
            }
        }
        CheckSpec::AnyOf { checks, .. } => {
            let children: Vec<CheckOutcome> = checks
                .iter()
                .map(|child| {
                    run_doctor_check(child, manifest, probe_ctx, skip_active_service_probe)
                })
                .collect();
            CheckOutcome {
                spec_label: format!("any_of ({} checks)", checks.len()),
                status: any_of_status(&children),
                detail: None,
                children,
            }
        }
        leaf if probe_ctx.dry_run => CheckOutcome {
            spec_label: doctor_check_label(leaf),
            status: CheckStatus::Skipped,
            detail: None,
            children: Vec::new(),
        },
        CheckSpec::SystemdActive { .. } if skip_active_service_probe => CheckOutcome {
            spec_label: doctor_check_label(spec),
            status: CheckStatus::Skipped,
            detail: Some("component is disabled; active service probe skipped".to_string()),
            children: Vec::new(),
        },
        CheckSpec::SystemdActive { service } => {
            let scope = systemd_active_scope(service, manifest);
            probe_systemd_active(service, scope, probe_ctx)
        }
        leaf => run_check(
            leaf,
            &CheckEnv {
                layout: probe_ctx.layout,
                dry_run: probe_ctx.dry_run,
            },
        ),
    }
}

fn all_of_status(children: &[CheckOutcome]) -> CheckStatus {
    if children
        .iter()
        .any(|child| child.status == CheckStatus::Failed)
    {
        CheckStatus::Failed
    } else if children.iter().all(|child| child.status == CheckStatus::Ok) {
        CheckStatus::Ok
    } else if children
        .iter()
        .all(|child| child.status == CheckStatus::Skipped)
    {
        CheckStatus::Skipped
    } else if children
        .iter()
        .all(|child| matches!(child.status, CheckStatus::Ok | CheckStatus::Skipped))
    {
        CheckStatus::Ok
    } else {
        CheckStatus::Unsupported
    }
}

fn any_of_status(children: &[CheckOutcome]) -> CheckStatus {
    if children.iter().any(|child| child.status == CheckStatus::Ok) {
        CheckStatus::Ok
    } else if children
        .iter()
        .all(|child| child.status == CheckStatus::Skipped)
    {
        CheckStatus::Skipped
    } else if children
        .iter()
        .any(|child| child.status == CheckStatus::Failed)
    {
        CheckStatus::Failed
    } else {
        CheckStatus::Unsupported
    }
}

fn doctor_check_label(spec: &CheckSpec) -> String {
    match spec {
        CheckSpec::BinaryVersion { binary, .. } => format!("binary_version binary={binary}"),
        CheckSpec::BinaryHelp { binary, .. } => format!("binary_help binary={binary}"),
        CheckSpec::SystemdActive { service } => format!("systemd_active service={service}"),
        CheckSpec::FileExists { path, .. } => format!("file_exists path={path}"),
        CheckSpec::PortListen { port, .. } => format!("port_listen port={port}"),
        CheckSpec::HttpGet { url, .. } => format!("http_get url={url}"),
        CheckSpec::BinaryCapabilities { binary, .. } => {
            format!("binary_capabilities binary={binary}")
        }
        CheckSpec::Command { argv, .. } => format!("command argv={}", argv.join(" ")),
        CheckSpec::AllOf { checks, .. } => format!("all_of ({} checks)", checks.len()),
        CheckSpec::AnyOf { checks, .. } => format!("any_of ({} checks)", checks.len()),
    }
}

fn probe_systemd_active(
    service: &str,
    scope: ServiceScope,
    probe_ctx: &DoctorProbeContext<'_>,
) -> CheckOutcome {
    let label = format!("systemd_active service={service}");
    if service.trim().is_empty() {
        return check_outcome(
            label,
            CheckStatus::Failed,
            Some("systemd_active check missing service name".to_string()),
        );
    }
    if probe_ctx.dry_run {
        return check_outcome(label, CheckStatus::Skipped, None);
    }
    let manager = service_manager_for_scope(scope, probe_ctx);
    if !manager.supported() {
        return check_outcome(
            label,
            CheckStatus::Unsupported,
            manager
                .unsupported_reason()
                .map(str::to_string)
                .or_else(|| Some("service manager not supported".to_string())),
        );
    }
    if !manager.handles_scope(scope) {
        return check_outcome(
            label,
            CheckStatus::Unsupported,
            Some(format!(
                "service manager '{}' does not handle {}-scope units",
                manager.manager(),
                service_scope_label(scope)
            )),
        );
    }
    match manager.probe_service(service) {
        Ok(outcome) => match outcome.state {
            ServiceState::Active => check_outcome(
                label,
                CheckStatus::Ok,
                Some(format!("unit '{service}' is active")),
            ),
            ServiceState::NotSupported => check_outcome(
                label,
                CheckStatus::Unsupported,
                Some(non_empty_or(outcome.message, "service manager unsupported")),
            ),
            ServiceState::NotInstalled => check_outcome(
                label,
                CheckStatus::Failed,
                Some(format!("unit '{service}' is not installed")),
            ),
            other => check_outcome(
                label,
                CheckStatus::Failed,
                Some(format!("unit '{service}' state '{}'", other.as_str())),
            ),
        },
        Err(err) => check_outcome(
            label,
            CheckStatus::Failed,
            Some(format!("systemd probe for unit '{service}' failed: {err}")),
        ),
    }
}

fn check_outcome(spec_label: String, status: CheckStatus, detail: Option<String>) -> CheckOutcome {
    CheckOutcome {
        spec_label,
        status,
        detail,
        children: Vec::new(),
    }
}

fn service_manager_for_scope<'a>(
    scope: ServiceScope,
    probe_ctx: &'a DoctorProbeContext<'_>,
) -> &'a dyn ServiceManager {
    match scope {
        ServiceScope::System => probe_ctx.system_service,
        ServiceScope::User => probe_ctx.user_service,
    }
}

fn service_scope_label(scope: ServiceScope) -> &'static str {
    match scope {
        ServiceScope::System => "system",
        ServiceScope::User => "user",
    }
}

fn systemd_active_scope(service: &str, manifest: Option<&ComponentManifest>) -> ServiceScope {
    manifest
        .and_then(|manifest| {
            manifest
                .install
                .services
                .iter()
                .find(|decl| service_decl_matches(&decl.unit, service))
                .map(|decl| decl.scope)
        })
        .unwrap_or(ServiceScope::System)
}

fn add_service_refs(
    manifest: Option<&ComponentManifest>,
    object: Option<&InstalledObject>,
    probe_ctx: &DoctorProbeContext<'_>,
    out: &mut DoctorComponent,
) {
    let Some(object) = object else {
        return;
    };
    if object.services.is_empty() {
        return;
    }
    let explicit_systemd = manifest
        .and_then(ComponentManifest::health_spec)
        .map(|spec| {
            let mut units = BTreeSet::new();
            collect_systemd_active_units(&spec, &mut units);
            units
        })
        .unwrap_or_default();
    let skip_active_service_probe = object.status == ObjectStatus::Disabled;

    for service in &object.services {
        if explicit_systemd.contains(&service.name) {
            continue;
        }
        add_service_ref(service, manifest, probe_ctx, skip_active_service_probe, out);
    }
}

fn add_service_ref(
    service: &ServiceRef,
    manifest: Option<&ComponentManifest>,
    probe_ctx: &DoctorProbeContext<'_>,
    skip_active_service_probe: bool,
    out: &mut DoctorComponent,
) {
    let name = format!("service_ref:{}", service.name);
    if !service_should_be_active(service, manifest) {
        out.health_checks.push(DoctorHealthCheck {
            name,
            status: "skipped".to_string(),
            source: "service_ref".to_string(),
            detail: Some("service is not declared to start during install".to_string()),
            checked_at: None,
        });
        return;
    }
    if skip_active_service_probe {
        out.health_checks.push(DoctorHealthCheck {
            name,
            status: "skipped".to_string(),
            source: "service_ref".to_string(),
            detail: Some("component is disabled; active service probe skipped".to_string()),
            checked_at: None,
        });
        return;
    }
    if probe_ctx.dry_run {
        out.health_checks.push(DoctorHealthCheck {
            name,
            status: "skipped".to_string(),
            source: "service_ref".to_string(),
            detail: Some("dry-run: service probe not executed".to_string()),
            checked_at: None,
        });
        return;
    }

    let manager = service_manager_for_scope(service.scope, probe_ctx);
    if !manager.supported() || !manager.handles_scope(service.scope) {
        let detail = manager
            .unsupported_reason()
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "service manager '{}' does not handle {}-scope units",
                    manager.manager(),
                    service_scope_label(service.scope)
                )
            });
        out.health_checks.push(DoctorHealthCheck {
            name,
            status: "skipped".to_string(),
            source: "service_ref".to_string(),
            detail: Some(detail),
            checked_at: None,
        });
        return;
    }

    match manager.probe_service(&service.name) {
        Ok(outcome) => add_service_ref_outcome(service, outcome.state, Some(outcome.message), out),
        Err(err) => {
            out.health_checks.push(DoctorHealthCheck {
                name,
                status: "probe_error".to_string(),
                source: "service_ref".to_string(),
                detail: Some(err.to_string()),
                checked_at: None,
            });
            out.findings.push(finding(
                FindingSeverity::Error,
                "service_probe_failed",
                format!("service '{}' could not be probed", service.name),
                "service_ref",
                Some(err.to_string()),
            ));
            out.fix_plan.push(suggestion(
                "inspect_logs",
                Some(format!("anolisa logs {}", out.name)),
                "inspect service-manager errors for the component",
            ));
        }
    }
}

fn add_service_ref_outcome(
    service: &ServiceRef,
    state: ServiceState,
    detail: Option<String>,
    out: &mut DoctorComponent,
) {
    let status = state.as_str().to_string();
    out.health_checks.push(DoctorHealthCheck {
        name: format!("service_ref:{}", service.name),
        status: status.clone(),
        source: "service_ref".to_string(),
        detail: detail.clone(),
        checked_at: None,
    });
    match state {
        ServiceState::Active | ServiceState::NotSupported => {}
        ServiceState::Activating | ServiceState::Deactivating => {
            out.findings.push(finding(
                FindingSeverity::Warning,
                "service_not_ready",
                format!("service '{}' is '{}'", service.name, state.as_str()),
                "service_ref",
                detail,
            ));
            out.fix_plan.push(suggestion(
                "inspect_logs",
                Some(format!("anolisa logs {}", out.name)),
                "inspect service startup progress",
            ));
        }
        ServiceState::NotInstalled => {
            out.findings.push(finding(
                FindingSeverity::Error,
                "service_unit_missing",
                format!("service unit '{}' is not installed", service.name),
                "service_ref",
                detail,
            ));
            out.fix_plan.push(suggestion(
                "reinstall_component",
                Some(format!("anolisa install {}", out.name)),
                "restore the missing service unit",
            ));
        }
        ServiceState::Inactive | ServiceState::Failed | ServiceState::Unknown => {
            out.findings.push(finding(
                FindingSeverity::Error,
                "service_not_active",
                format!("service '{}' is '{}'", service.name, state.as_str()),
                "service_ref",
                detail,
            ));
            out.fix_plan.push(suggestion(
                "restart_component",
                Some(format!("anolisa restart {}", out.name)),
                "restart the component service",
            ));
            out.fix_plan.push(suggestion(
                "inspect_logs",
                Some(format!("anolisa logs {}", out.name)),
                "inspect service logs for the component",
            ));
        }
    }
}

fn collect_systemd_active_units(spec: &CheckSpec, out: &mut BTreeSet<String>) {
    match spec {
        CheckSpec::SystemdActive { service } => {
            out.insert(service.clone());
        }
        CheckSpec::AllOf { checks, .. } | CheckSpec::AnyOf { checks, .. } => {
            for child in checks {
                collect_systemd_active_units(child, out);
            }
        }
        _ => {}
    }
}

fn service_should_be_active(service: &ServiceRef, manifest: Option<&ComponentManifest>) -> bool {
    let Some(manifest) = manifest else {
        return true;
    };
    manifest
        .install
        .services
        .iter()
        .find(|decl| service_decl_matches(&decl.unit, &service.name))
        .map(|decl| decl.start)
        .unwrap_or(true)
}

fn service_decl_matches(declared: &str, effective: &str) -> bool {
    if declared == effective {
        return true;
    }
    if let Some(prefix) = declared.strip_suffix("@.service") {
        return effective
            .strip_prefix(&format!("{prefix}@"))
            .map(|rest| rest.ends_with(".service") && rest.len() > ".service".len())
            .unwrap_or(false);
    }
    false
}

fn non_empty_or(value: String, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

fn add_check_outcome(component: &str, outcome: &CheckOutcome, out: &mut DoctorComponent) {
    let status = outcome.status.as_str().to_string();
    out.health_checks.push(DoctorHealthCheck {
        name: outcome.spec_label.clone(),
        status: status.clone(),
        source: "structured_health".to_string(),
        detail: outcome.detail.clone(),
        checked_at: None,
    });
    match outcome.status {
        CheckStatus::Ok | CheckStatus::Skipped => {}
        CheckStatus::Unsupported => {
            out.findings.push(finding(
                FindingSeverity::Warning,
                "structured_health_unsupported",
                format!(
                    "health check '{}' could not be verified",
                    outcome.spec_label
                ),
                "structured_health",
                outcome.detail.clone(),
            ));
            out.fix_plan.push(suggestion(
                "fix_manifest",
                None,
                "adjust the component health check declaration",
            ));
        }
        CheckStatus::Failed => {
            out.findings.push(finding(
                FindingSeverity::Error,
                "structured_health_failed",
                format!("health check '{}' failed", outcome.spec_label),
                "structured_health",
                outcome.detail.clone(),
            ));
            out.fix_plan
                .extend(suggestions_for_structured_health(component, outcome));
        }
    }
    for child in &outcome.children {
        add_check_outcome(component, child, out);
    }
}

fn add_runtime_dependencies(
    manifest: Option<&ComponentManifest>,
    object: Option<&InstalledObject>,
    resolver_env: &ResolverEnv,
    dry_run: bool,
    out: &mut DoctorComponent,
) {
    let Some(object) = object else {
        return;
    };
    let Some(manifest) = manifest else {
        return;
    };
    if manifest.runtime_deps.is_empty() {
        return;
    }
    if object.effective_ownership().is_rpm() {
        for dep in &manifest.runtime_deps {
            out.dependencies.push(DoctorDependency {
                name: dep.name.clone(),
                kind: dep.kind,
                status: DoctorDependencyStatus::Skipped,
                note: Some("RPM backend owns runtime dependency resolution".to_string()),
                detail: None,
            });
        }
        return;
    }
    if dry_run {
        for dep in &manifest.runtime_deps {
            out.dependencies.push(DoctorDependency {
                name: dep.name.clone(),
                kind: dep.kind,
                status: DoctorDependencyStatus::Skipped,
                note: Some("dry-run: dependency probe not executed".to_string()),
                detail: None,
            });
        }
        return;
    }

    match DependencyResolver::system().resolve(&manifest.runtime_deps, resolver_env) {
        Ok(plan) => {
            for warning in plan.warnings {
                out.findings.push(finding(
                    FindingSeverity::Warning,
                    "dependency_warning",
                    warning,
                    "dependency",
                    None,
                ));
            }
            for resolution in plan.resolutions {
                add_dependency_resolution(&resolution, out);
            }
        }
        Err(err) => {
            out.findings.push(finding(
                FindingSeverity::Error,
                "invalid_dependency_declaration",
                "runtime dependency declaration is invalid",
                "dependency",
                Some(err.to_string()),
            ));
            out.fix_plan.push(suggestion(
                "fix_manifest",
                None,
                "fix the component runtime dependency declaration",
            ));
        }
    }
}

fn add_dependency_resolution(resolution: &DependencyResolution, out: &mut DoctorComponent) {
    let (status, note) = match &resolution.status {
        DependencyStatus::Resolved => (DoctorDependencyStatus::Resolved, None),
        DependencyStatus::Unresolved { remediation } => (
            DoctorDependencyStatus::Unresolved,
            Some(remediation.clone()),
        ),
        DependencyStatus::Unresolvable { reason } => {
            (DoctorDependencyStatus::Unresolvable, Some(reason.clone()))
        }
    };
    out.dependencies.push(DoctorDependency {
        name: resolution.name.clone(),
        kind: resolution.kind,
        status,
        note: note.clone(),
        detail: resolution.detail.clone(),
    });

    match &resolution.status {
        DependencyStatus::Resolved => {}
        DependencyStatus::Unresolved { remediation } => {
            out.findings.push(finding(
                FindingSeverity::Error,
                "dependency_unresolved",
                format!(
                    "runtime dependency '{}' [{}] is missing",
                    resolution.name,
                    resolution.kind.as_str()
                ),
                "dependency",
                resolution.detail.clone(),
            ));
            out.fix_plan
                .push(suggestion_for_dependency(resolution.kind, remediation));
        }
        DependencyStatus::Unresolvable { reason } => {
            out.findings.push(finding(
                FindingSeverity::Error,
                "dependency_unresolvable",
                format!(
                    "runtime dependency '{}' [{}] cannot be satisfied automatically",
                    resolution.name,
                    resolution.kind.as_str()
                ),
                "dependency",
                Some(reason.clone()),
            ));
            out.fix_plan.push(suggestion(
                "satisfy_platform_requirement",
                None,
                reason.clone(),
            ));
        }
    }
}

fn add_rpm_drift(
    object: Option<&InstalledObject>,
    rpm_query: &RpmPackageQuery,
    out: &mut DoctorComponent,
) {
    let Some(object) = object else {
        return;
    };
    if !object.effective_ownership().is_rpm() {
        return;
    }
    let Some(meta) = object.rpm_metadata.as_ref() else {
        out.health_checks.push(DoctorHealthCheck {
            name: "rpmdb".to_string(),
            status: "unverified".to_string(),
            source: "rpm".to_string(),
            detail: Some("RPM package metadata is missing from ANOLISA state".to_string()),
            checked_at: None,
        });
        out.findings.push(finding(
            FindingSeverity::Warning,
            "rpm_metadata_missing",
            format!(
                "component '{}' is RPM-backed but has no recorded RPM package metadata",
                out.name
            ),
            "rpm",
            None,
        ));
        out.fix_plan.push(suggestion(
            "repair_state",
            Some(format!("anolisa repair {}", out.name)),
            "backfill RPM package metadata from rpmdb",
        ));
        return;
    };
    match status::probe_rpm_drift(meta, rpm_query) {
        Some(RpmDrift::Drifted { reason }) => {
            out.health_checks.push(DoctorHealthCheck {
                name: format!("rpmdb:{}", meta.package_name),
                status: "failed".to_string(),
                source: "rpm".to_string(),
                detail: Some(reason.clone()),
                checked_at: None,
            });
            out.findings.push(finding(
                FindingSeverity::Error,
                "rpm_drifted",
                format!(
                    "RPM package for component '{}' drifted from ANOLISA state",
                    out.name
                ),
                "rpm",
                Some(reason),
            ));
            out.fix_plan.push(suggestion(
                "repair_state",
                Some(format!("anolisa repair {}", out.name)),
                "refresh ANOLISA state from rpmdb",
            ));
        }
        Some(RpmDrift::Missing) => {
            let package = meta.package_name.clone();
            out.health_checks.push(DoctorHealthCheck {
                name: format!("rpmdb:{package}"),
                status: "failed".to_string(),
                source: "rpm".to_string(),
                detail: Some("recorded RPM package is absent from rpmdb".to_string()),
                checked_at: None,
            });
            out.findings.push(finding(
                FindingSeverity::Error,
                "rpm_missing",
                format!(
                    "RPM package '{package}' recorded for component '{}' is missing",
                    out.name
                ),
                "rpm",
                None,
            ));
            out.fix_plan.push(suggestion(
                "forget_state",
                Some(format!("anolisa forget {}", out.name)),
                "drop the stale ANOLISA state record for a package that is gone from rpmdb",
            ));
            out.fix_plan.push(suggestion(
                "reinstall_component",
                Some(format!("anolisa install {}", out.name)),
                "reinstall the component if it should remain managed",
            ));
        }
        None => out.health_checks.push(DoctorHealthCheck {
            name: format!("rpmdb:{}", meta.package_name),
            status: "ok".to_string(),
            source: "rpm".to_string(),
            detail: None,
            checked_at: None,
        }),
    }
}

fn resolve_component_manifest(
    layout: &FsLayout,
    catalog: Option<&Catalog>,
    component: &str,
) -> (Option<ComponentManifest>, Option<String>) {
    match common::installed_component_manifest_path(layout, component, COMMAND) {
        Ok(path) if path.is_file() => match ComponentManifest::from_file(&path) {
            Ok(manifest) => (Some(manifest), None),
            Err(err) => (
                None,
                Some(format!(
                    "failed to parse installed manifest snapshot at {}: {err}",
                    path.display()
                )),
            ),
        },
        Ok(_) => match catalog.and_then(|c| c.component(component).cloned()) {
            Some(manifest) => (Some(manifest), None),
            None => (
                None,
                Some(format!(
                    "component contract unavailable for '{component}': no installed snapshot or catalog entry found"
                )),
            ),
        },
        Err(err) => (
            catalog.and_then(|c| c.component(component).cloned()),
            Some(err.reason()),
        ),
    }
}

fn resolver_env_from_facts(facts: &anolisa_env::EnvFacts) -> ResolverEnv {
    ResolverEnv {
        kernel: facts.kernel.clone(),
        pkg_base: facts
            .os_id
            .as_deref()
            .and_then(anolisa_env::pkg_base_from_id),
        btf: facts.btf,
        cap_bpf: facts.cap_bpf,
    }
}

fn summarize(components: &[DoctorComponent]) -> DoctorSummary {
    let mut summary = DoctorSummary {
        components_checked: components.len(),
        ok: 0,
        degraded: 0,
        failed: 0,
        findings: components.iter().map(|c| c.findings.len()).sum(),
    };
    for component in components {
        match component.status.as_str() {
            "ok" => summary.ok += 1,
            "failed" | "not_installed" => summary.failed += 1,
            _ => summary.degraded += 1,
        }
    }
    summary
}

fn component_status(component: &DoctorComponent) -> String {
    if component
        .findings
        .iter()
        .any(|f| f.severity == FindingSeverity::Error)
    {
        if component
            .findings
            .iter()
            .any(|f| f.code == "component_not_installed")
        {
            "not_installed".to_string()
        } else {
            "failed".to_string()
        }
    } else if component
        .findings
        .iter()
        .any(|f| f.severity == FindingSeverity::Warning)
    {
        "degraded".to_string()
    } else {
        "ok".to_string()
    }
}

fn render_doctor(ctx: &CliContext, payload: &DoctorPayload, ok: bool) -> Result<(), CliError> {
    if ctx.json {
        return render_json_with_status(COMMAND, ok, payload);
    }
    if !ctx.quiet {
        render_human(payload, ctx.no_color);
    }
    Ok(())
}

fn render_human(payload: &DoctorPayload, no_color: bool) {
    let color = Palette::new(no_color);
    if payload.components.is_empty() {
        println!("{}", color.muted("no installed components"));
        return;
    }
    println!(
        "{} {} checked, {} ok, {} degraded, {} failed",
        color.header("Doctor:"),
        payload.summary.components_checked,
        color.ok(payload.summary.ok),
        color.warn(payload.summary.degraded),
        color.err(payload.summary.failed),
    );
    for warning in &payload.warnings {
        println!("{} {warning}", color.warn("warning:"));
    }
    for component in &payload.components {
        println!(
            "\n{} {} ({})",
            color.label(&component.name),
            color.status(&component.status),
            component.version.as_deref().unwrap_or("-"),
        );
        if component.findings.is_empty() {
            println!("  {}", color.ok("no issues found"));
            continue;
        }
        for finding in &component.findings {
            let sev = match finding.severity {
                FindingSeverity::Warning => color.warn("warning"),
                FindingSeverity::Error => color.err("error"),
            };
            println!("  {sev} [{}] {}", finding.code, finding.message);
            if let Some(detail) = &finding.detail {
                println!("    {} {detail}", color.muted("detail:"));
            }
        }
        if !component.fix_plan.is_empty() {
            println!("  {}", color.label("Recommended:"));
            for fix in &component.fix_plan {
                match &fix.command {
                    Some(command) => println!(
                        "    {} {}",
                        color.command(command),
                        color.muted(format!("({})", fix.reason))
                    ),
                    None => println!("    {} {}", fix.action, color.muted(&fix.reason)),
                }
            }
        }
    }
}

fn health_from_entry(entry: &HealthEntry) -> DoctorHealthCheck {
    DoctorHealthCheck {
        name: entry.name.clone(),
        status: entry.status.clone(),
        source: "status_health".to_string(),
        detail: entry.reason.clone(),
        checked_at: Some(entry.checked_at.clone()),
    }
}

fn severity_for_health_status(status: &str) -> Option<FindingSeverity> {
    match status {
        "ok" | "skipped" | "unverified" => None,
        "not_supported" | "unsupported" | "unsupported_kind" | "out_of_bounds"
        | "unsupported_target" | "not_regular_file" | "timeout" => Some(FindingSeverity::Warning),
        _ => Some(FindingSeverity::Error),
    }
}

fn suggestions_for_health(component: &str, check_name: &str, status: &str) -> Vec<FixSuggestion> {
    match status {
        "missing_file" | "sha256_mismatch" => vec![suggestion(
            "reinstall_component",
            Some(format!("anolisa install {component}")),
            "restore missing or modified ANOLISA-owned files",
        )],
        "command_failed" | "command_error" | "probe_error" | "timeout" => vec![suggestion(
            "inspect_logs",
            Some(format!("anolisa logs {component}")),
            "inspect runtime logs for the failing health probe",
        )],
        "out_of_bounds" | "unsupported_target" | "unsupported_kind" | "not_regular_file"
        | "invalid_check" => vec![suggestion(
            "fix_manifest",
            None,
            format!("fix the manifest health check '{check_name}'"),
        )],
        _ => vec![suggestion(
            "inspect_component",
            None,
            format!("inspect health check '{check_name}'"),
        )],
    }
}

fn suggestions_for_structured_health(
    component: &str,
    outcome: &CheckOutcome,
) -> Vec<FixSuggestion> {
    if outcome.spec_label.starts_with("file_exists")
        || outcome.spec_label.starts_with("binary_version")
        || outcome.spec_label.starts_with("binary_help")
    {
        vec![suggestion(
            "reinstall_component",
            Some(format!("anolisa install {component}")),
            "restore files required by the structured health check",
        )]
    } else {
        vec![suggestion(
            "inspect_logs",
            Some(format!("anolisa logs {component}")),
            "inspect runtime logs for the failing structured health check",
        )]
    }
}

fn suggestion_for_dependency(kind: DependencyKind, remediation: &str) -> FixSuggestion {
    let command = remediation
        .starts_with("sudo ")
        .then(|| remediation.to_string())
        .or_else(|| {
            remediation
                .starts_with("anolisa ")
                .then(|| remediation.to_string())
        });
    let action = match kind {
        DependencyKind::LanguageRuntime => "install_runtime",
        DependencyKind::SystemPackage => "install_package",
        DependencyKind::PlatformCapability => "satisfy_platform_requirement",
    };
    suggestion(action, command, remediation)
}

fn suggestion(
    action: impl Into<String>,
    command: Option<String>,
    reason: impl Into<String>,
) -> FixSuggestion {
    FixSuggestion {
        action: action.into(),
        command,
        reason: reason.into(),
        automatic: false,
    }
}

fn finding(
    severity: FindingSeverity,
    code: impl Into<String>,
    message: impl Into<String>,
    source: impl Into<String>,
    detail: Option<String>,
) -> DoctorFinding {
    DoctorFinding {
        severity,
        code: code.into(),
        message: message.into(),
        source: source.into(),
        detail,
    }
}

fn sanitize_code(status: &str) -> String {
    status
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn dedupe_fix_plan(fix_plan: &mut Vec<FixSuggestion>) {
    let mut seen = BTreeSet::new();
    fix_plan.retain(|item| seen.insert(item.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_core::{
        DependencyStatus, FakeServiceManager, HealthEntry, ObjectStatus, Ownership, ServiceOp,
    };

    fn record(name: &str, status: &str) -> ComponentRecord {
        ComponentRecord {
            name: name.to_string(),
            status: status.to_string(),
            version: Some("1.0.0".to_string()),
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

    fn object(name: &str, status: ObjectStatus, ownership: Ownership) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: name.to_string(),
            version: "1.0.0".to_string(),
            status,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(ownership),
            rpm_metadata: None,
            installed_at: "2026-06-01T00:00:00Z".to_string(),
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: anolisa_core::SubscriptionScope::None,
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
        }
    }

    fn service_ref(name: &str, scope: ServiceScope) -> ServiceRef {
        ServiceRef {
            name: name.to_string(),
            manager: scope.manager_label().to_string(),
            restartable: true,
            enabled: true,
            scope,
        }
    }

    fn empty_component(name: &str) -> DoctorComponent {
        DoctorComponent {
            name: name.to_string(),
            status: "ok".to_string(),
            state_status: None,
            version: None,
            findings: Vec::new(),
            health_checks: Vec::new(),
            dependencies: Vec::new(),
            fix_plan: Vec::new(),
        }
    }

    fn probe_context<'a>(
        layout: &'a FsLayout,
        resolver_env: &'a ResolverEnv,
        rpm_query: &'a RpmPackageQuery,
        system_service: &'a dyn ServiceManager,
        user_service: &'a dyn ServiceManager,
        dry_run: bool,
    ) -> DoctorProbeContext<'a> {
        DoctorProbeContext {
            layout,
            resolver_env,
            rpm_query,
            system_service,
            user_service,
            dry_run,
        }
    }

    #[test]
    fn missing_component_recommends_install() {
        let mut component = DoctorComponent {
            name: "ghost".to_string(),
            status: "ok".to_string(),
            state_status: None,
            version: None,
            findings: Vec::new(),
            health_checks: Vec::new(),
            dependencies: Vec::new(),
            fix_plan: Vec::new(),
        };
        add_state_finding(&record("ghost", "not_installed"), None, &mut component);

        assert_eq!(component.findings[0].code, "component_not_installed");
        assert_eq!(
            component.fix_plan[0].command.as_deref(),
            Some("anolisa install ghost")
        );
    }

    #[test]
    fn failed_health_recommends_reinstall() {
        let mut rec = record("agentsight", "installed");
        rec.health.push(HealthEntry {
            name: "integrity:/x".to_string(),
            status: "missing_file".to_string(),
            checked_at: "2026-06-01T00:00:00Z".to_string(),
            reason: Some("missing file".to_string()),
        });
        let mut component = DoctorComponent {
            name: rec.name.clone(),
            status: "ok".to_string(),
            state_status: None,
            version: None,
            findings: Vec::new(),
            health_checks: Vec::new(),
            dependencies: Vec::new(),
            fix_plan: Vec::new(),
        };
        add_health_entries(&rec, &mut component);

        assert_eq!(component.findings[0].severity, FindingSeverity::Error);
        assert_eq!(
            component.fix_plan[0].command.as_deref(),
            Some("anolisa install agentsight")
        );
    }

    #[test]
    fn unresolved_dependency_becomes_remediation() {
        let resolution = DependencyResolution {
            name: "btrfs-progs".to_string(),
            kind: DependencyKind::SystemPackage,
            status: DependencyStatus::Unresolved {
                remediation: "sudo dnf install btrfs-progs".to_string(),
            },
            detail: None,
        };
        let mut component = DoctorComponent {
            name: "ws-ckpt".to_string(),
            status: "ok".to_string(),
            state_status: None,
            version: None,
            findings: Vec::new(),
            health_checks: Vec::new(),
            dependencies: Vec::new(),
            fix_plan: Vec::new(),
        };
        add_dependency_resolution(&resolution, &mut component);

        assert_eq!(
            component.dependencies[0].status,
            DoctorDependencyStatus::Unresolved
        );
        assert_eq!(component.findings[0].code, "dependency_unresolved");
        assert_eq!(
            component.fix_plan[0].command.as_deref(),
            Some("sudo dnf install btrfs-progs")
        );
    }

    #[test]
    fn rpm_record_discards_raw_layout_manifest_health() {
        let mut rec = record("agentsight", "failed");
        rec.health.push(HealthEntry {
            name: "agentsight:command:launcher".to_string(),
            status: "command_error".to_string(),
            checked_at: "2026-06-01T00:00:00Z".to_string(),
            reason: Some("raw bindir probe failed".to_string()),
        });
        rec.health.push(HealthEntry {
            name: "persisted:last".to_string(),
            status: "ok".to_string(),
            checked_at: "2026-06-01T00:00:00Z".to_string(),
            reason: None,
        });
        let obj = object("agentsight", ObjectStatus::Installed, Ownership::RpmManaged);

        normalize_rpm_record(&mut rec, Some(&obj));

        assert_eq!(rec.status, "installed");
        assert_eq!(rec.health.len(), 1);
        assert_eq!(rec.health[0].name, "persisted:last");
    }

    #[test]
    fn rpm_missing_contract_does_not_degrade_component() {
        let obj = object("tokenless", ObjectStatus::Installed, Ownership::RpmManaged);
        let mut component = DoctorComponent {
            name: "tokenless".to_string(),
            status: "ok".to_string(),
            state_status: None,
            version: None,
            findings: Vec::new(),
            health_checks: Vec::new(),
            dependencies: Vec::new(),
            fix_plan: Vec::new(),
        };

        add_manifest_warning(
            Some(
                "component contract unavailable for 'tokenless': no installed snapshot or catalog entry found"
                    .to_string(),
            ),
            Some(&obj),
            &mut component,
        );

        assert!(component.findings.is_empty());
        assert!(component.fix_plan.is_empty());
    }

    #[test]
    fn unverified_health_is_informational_when_state_is_clean() {
        let mut rec = record("agent-memory", "degraded");
        rec.health.push(HealthEntry {
            name: "integrity:/var/lib/anolisa/component-manifests/agent-memory/component.toml"
                .to_string(),
            status: "unverified".to_string(),
            checked_at: "2026-06-01T00:00:00Z".to_string(),
            reason: None,
        });
        let obj = object(
            "agent-memory",
            ObjectStatus::Installed,
            Ownership::RawManaged,
        );
        let mut component = DoctorComponent {
            name: rec.name.clone(),
            status: "ok".to_string(),
            state_status: None,
            version: None,
            findings: Vec::new(),
            health_checks: Vec::new(),
            dependencies: Vec::new(),
            fix_plan: Vec::new(),
        };

        add_state_finding(&rec, Some(&obj), &mut component);
        add_health_entries(&rec, &mut component);

        assert!(component.findings.is_empty());
        assert!(component.fix_plan.is_empty());
        assert_eq!(component.health_checks[0].status, "unverified");
    }

    #[test]
    fn structured_systemd_active_uses_service_manager() {
        let layout = FsLayout::system(None);
        let resolver_env = ResolverEnv::default();
        let rpm_query = RpmPackageQuery::system();
        let system_service = FakeServiceManager::new();
        system_service.set_state(ServiceState::Active);
        let user_service = FakeServiceManager::with_scope(ServiceScope::User);
        let ctx = probe_context(
            &layout,
            &resolver_env,
            &rpm_query,
            &system_service,
            &user_service,
            false,
        );

        let outcome = run_doctor_check(
            &CheckSpec::SystemdActive {
                service: "agentsight.service".to_string(),
            },
            None,
            &ctx,
            false,
        );

        assert_eq!(outcome.status, CheckStatus::Ok);
        assert_eq!(
            system_service.calls(),
            vec![(ServiceOp::Probe, "agentsight.service".to_string())]
        );
    }

    #[test]
    fn structured_systemd_active_uses_manifest_service_scope() {
        let manifest = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "agent-memory"
            version = "0.1.0"

            [component.layout]
            modes = ["system"]

            [[component.services]]
            unit = "anolisa-memory@.service"
            scope = "user"

            [component.health_check]
            type = "systemd_active"
            service = "anolisa-memory@root.service"
        "#,
        )
        .expect("parse manifest");
        let layout = FsLayout::system(None);
        let resolver_env = ResolverEnv::default();
        let rpm_query = RpmPackageQuery::system();
        let system_service = FakeServiceManager::new();
        let user_service = FakeServiceManager::with_scope(ServiceScope::User);
        user_service.set_state(ServiceState::Active);
        let ctx = probe_context(
            &layout,
            &resolver_env,
            &rpm_query,
            &system_service,
            &user_service,
            false,
        );

        let outcome = run_doctor_check(
            &CheckSpec::SystemdActive {
                service: "anolisa-memory@root.service".to_string(),
            },
            Some(&manifest),
            &ctx,
            false,
        );

        assert_eq!(outcome.status, CheckStatus::Ok);
        assert!(system_service.calls().is_empty());
        assert_eq!(
            user_service.calls(),
            vec![(ServiceOp::Probe, "anolisa-memory@root.service".to_string())]
        );
    }

    #[test]
    fn structured_systemd_active_fails_when_unit_is_inactive() {
        let layout = FsLayout::system(None);
        let resolver_env = ResolverEnv::default();
        let rpm_query = RpmPackageQuery::system();
        let system_service = FakeServiceManager::new();
        let user_service = FakeServiceManager::with_scope(ServiceScope::User);
        let ctx = probe_context(
            &layout,
            &resolver_env,
            &rpm_query,
            &system_service,
            &user_service,
            false,
        );

        let outcome = run_doctor_check(
            &CheckSpec::SystemdActive {
                service: "agentsight.service".to_string(),
            },
            None,
            &ctx,
            false,
        );

        assert_eq!(outcome.status, CheckStatus::Failed);
        assert!(
            outcome
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("inactive")
        );
    }

    #[test]
    fn structured_systemd_active_is_skipped_for_disabled_component() {
        let layout = FsLayout::system(None);
        let resolver_env = ResolverEnv::default();
        let rpm_query = RpmPackageQuery::system();
        let system_service = FakeServiceManager::new();
        let user_service = FakeServiceManager::with_scope(ServiceScope::User);
        let ctx = probe_context(
            &layout,
            &resolver_env,
            &rpm_query,
            &system_service,
            &user_service,
            false,
        );

        let outcome = run_doctor_check(
            &CheckSpec::SystemdActive {
                service: "agentsight.service".to_string(),
            },
            None,
            &ctx,
            true,
        );

        assert_eq!(outcome.status, CheckStatus::Skipped);
        assert!(system_service.calls().is_empty());
        assert!(
            outcome
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("component is disabled")
        );
    }

    #[test]
    fn service_ref_inactive_unit_becomes_recommendation() {
        let layout = FsLayout::system(None);
        let resolver_env = ResolverEnv::default();
        let rpm_query = RpmPackageQuery::system();
        let system_service = FakeServiceManager::new();
        let user_service = FakeServiceManager::with_scope(ServiceScope::User);
        let ctx = probe_context(
            &layout,
            &resolver_env,
            &rpm_query,
            &system_service,
            &user_service,
            false,
        );
        let mut obj = object("agentsight", ObjectStatus::Installed, Ownership::RawManaged);
        obj.services
            .push(service_ref("agentsight.service", ServiceScope::System));
        let mut component = empty_component("agentsight");

        add_service_refs(None, Some(&obj), &ctx, &mut component);

        assert_eq!(component.health_checks[0].status, "inactive");
        assert_eq!(component.findings[0].code, "service_not_active");
        assert_eq!(
            component.fix_plan[0].command.as_deref(),
            Some("anolisa restart agentsight")
        );
    }

    #[test]
    fn service_ref_inactive_unit_is_skipped_for_disabled_component() {
        let layout = FsLayout::system(None);
        let resolver_env = ResolverEnv::default();
        let rpm_query = RpmPackageQuery::system();
        let system_service = FakeServiceManager::new();
        let user_service = FakeServiceManager::with_scope(ServiceScope::User);
        let ctx = probe_context(
            &layout,
            &resolver_env,
            &rpm_query,
            &system_service,
            &user_service,
            false,
        );
        let mut obj = object("agentsight", ObjectStatus::Disabled, Ownership::RawManaged);
        obj.services
            .push(service_ref("agentsight.service", ServiceScope::System));
        let mut component = empty_component("agentsight");

        add_service_refs(None, Some(&obj), &ctx, &mut component);

        assert_eq!(component.health_checks[0].status, "skipped");
        assert!(
            component.health_checks[0]
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("component is disabled")
        );
        assert!(component.findings.is_empty());
        assert!(component.fix_plan.is_empty());
        assert!(system_service.calls().is_empty());
    }

    #[test]
    fn service_ref_start_false_is_skipped() {
        let manifest = ComponentManifest::from_toml_str(
            r#"
            [component]
            name = "agent-memory"
            version = "0.1.0"

            [component.layout]
            modes = ["system"]

            [[component.services]]
            unit = "anolisa-memory@.service"
            scope = "user"
            enable = false
            start = false
        "#,
        )
        .expect("parse manifest");
        let layout = FsLayout::system(None);
        let resolver_env = ResolverEnv::default();
        let rpm_query = RpmPackageQuery::system();
        let system_service = FakeServiceManager::new();
        let user_service = FakeServiceManager::with_scope(ServiceScope::User);
        let ctx = probe_context(
            &layout,
            &resolver_env,
            &rpm_query,
            &system_service,
            &user_service,
            false,
        );
        let mut obj = object(
            "agent-memory",
            ObjectStatus::Installed,
            Ownership::RawManaged,
        );
        obj.services.push(service_ref(
            "anolisa-memory@root.service",
            ServiceScope::User,
        ));
        let mut component = empty_component("agent-memory");

        add_service_refs(Some(&manifest), Some(&obj), &ctx, &mut component);

        assert_eq!(component.health_checks[0].status, "skipped");
        assert!(component.findings.is_empty());
    }

    #[test]
    fn component_status_escalates_findings() {
        let mut component = DoctorComponent {
            name: "tokenless".to_string(),
            status: "ok".to_string(),
            state_status: None,
            version: None,
            findings: vec![finding(
                FindingSeverity::Warning,
                "health_unverified",
                "unverified",
                "health",
                None,
            )],
            health_checks: Vec::new(),
            dependencies: Vec::new(),
            fix_plan: Vec::new(),
        };
        assert_eq!(component_status(&component), "degraded");
        component.findings.push(finding(
            FindingSeverity::Error,
            "dependency_unresolved",
            "missing",
            "dependency",
            None,
        ));
        assert_eq!(component_status(&component), "failed");
    }
}
