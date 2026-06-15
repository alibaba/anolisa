//! Adapter manager: the trusted orchestrator that owns the
//! dangerous-resource boundary.
//!
//! The Manager is the only thing that takes the install lock, reads and
//! writes adapter receipts in `installed.toml`, re-validates every
//! [`ClaimResource`](super::claim::ClaimResource) against a driver's static
//! external roots, runs framework CLIs through a single controlled
//! [`AdapterOps`] implementation, and records to the central log. Drivers
//! own framework *semantics*; the Manager owns *trust and IO*. A driver
//! never spawns a process, deletes a path, or persists state on its own.
//!
//! Resource discovery follows the layout convention
//! `{datadir}/adapters/<component>/<framework>/`. Multiple datadir roots
//! may be searched (e.g. the user datadir preferred over the system one);
//! the first root that contains the directory wins.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anolisa_platform::fs_layout::{FsLayout, InstallMode};

use super::AdapterError;
use super::claim::{AdapterClaim, ClaimStatus};
use super::driver::{
    AdapterOps, AdapterStatusReport, CliOutput, DisableReport, DriverCtx, DriverPlan,
    FrameworkCommand, HostEnv,
};
use super::registry::DriverRegistry;
use crate::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use crate::lock::InstallLock;
use crate::manifest::ComponentManifest;
use crate::state::{InstalledState, ObjectKind, ObjectStatus};

/// Per-CLI-call producer name recorded in the central log.
const LOG_SOURCE: &str = "anolisa-cli";

/// Cap on captured stdout/stderr per framework CLI invocation (bytes).
/// Output beyond this is drained (so the child never blocks on a full
/// pipe) but discarded before logging.
const OUTPUT_CAP: usize = 64 * 1024;
/// State subdirectory where install stores component manifests used for
/// future adapter enable checks.
const INSTALLED_COMPONENT_MANIFESTS_SUBDIR: &str = "component-manifests";
/// Filename used for each saved installed component contract.
const INSTALLED_COMPONENT_MANIFEST_FILE: &str = "component.toml";

/// Outcome of [`AdapterManager::enable`].
#[derive(Debug, Clone)]
pub enum EnableOutcome {
    /// `--dry-run`: what enable *would* do, no state mutated.
    Planned(DriverPlan),
    /// Enable ran; the persisted receipt.
    Enabled(Box<AdapterClaim>),
}

/// Outcome of [`AdapterManager::disable`].
#[derive(Debug, Clone)]
pub struct DisableOutcome {
    /// Component the disable targeted.
    pub component: String,
    /// Resolved framework, when one was determined (`None` only for the
    /// "component has no enabled adapters" no-op).
    pub framework: Option<String>,
    /// The driver's cleanup report.
    pub report: DisableReport,
    /// True when the receipt was removed; false when it was kept and
    /// marked `cleanup_failed` for retry.
    pub claim_removed: bool,
}

/// One row of [`AdapterManager::scan`].
#[derive(Debug, Clone)]
pub struct ScanEntry {
    /// Component the adapter belongs to.
    pub component: String,
    /// Framework the adapter targets.
    pub framework: String,
    /// Whether the installed component manifest declares this adapter.
    pub declared: bool,
    /// Resource directory, when present under a visible datadir root.
    pub resource_root: Option<PathBuf>,
    /// Whether a built-in driver exists for `framework`.
    pub driver_available: bool,
    /// Whether the framework was detected on the host (best-effort;
    /// `false` when no driver is available to probe).
    pub framework_detected: bool,
    /// Whether a receipt for `(component, framework)` exists in state.
    pub enabled: bool,
    /// Lifecycle status of the receipt, when one exists.
    pub claim_status: Option<ClaimStatus>,
}

/// Full result of [`AdapterManager::scan`].
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    /// Adapter entries from manifest declarations and/or resource
    /// directories, sorted by `(component, framework)`.
    pub entries: Vec<ScanEntry>,
    /// Non-fatal manifest/state issues encountered while scanning fallback
    /// roots.
    pub warnings: Vec<String>,
}

/// One row of [`AdapterManager::status`].
#[derive(Debug, Clone)]
pub struct StatusEntry {
    /// Component the receipt belongs to.
    pub component: String,
    /// Framework the receipt targets.
    pub framework: String,
    /// The driver's status report for this receipt.
    pub report: AdapterStatusReport,
}

/// Full result of [`AdapterManager::status`].
#[derive(Debug, Clone, Default)]
pub struct StatusReport {
    /// Per-receipt status entries.
    pub entries: Vec<StatusEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AdapterDecl {
    component: String,
    framework: String,
}

/// Trusted orchestrator for adapter enable/disable/status/scan.
pub struct AdapterManager {
    layout: FsLayout,
    registry: DriverRegistry,
    state_path: PathBuf,
    /// State roots searched for installed component records/manifests, in
    /// preference order. Receipts are always written only to
    /// [`Self::state_path`].
    state_roots: Vec<PathBuf>,
    /// Datadir roots searched for `adapters/<component>/<framework>/`, in
    /// preference order (first match wins).
    datadir_roots: Vec<PathBuf>,
    user_home: Option<PathBuf>,
    /// Identity recorded as the central-log actor.
    actor: String,
}

impl AdapterManager {
    /// Build a manager for the given layout. The state file is
    /// `{state_dir}/installed.toml`; the primary datadir root is
    /// `layout.datadir`, and the primary component-manifest root is
    /// `layout.state_dir`. Use [`Self::push_datadir_root`] and
    /// [`Self::push_state_root`] to add fallbacks (e.g. system roots when
    /// running in user mode).
    pub fn new(layout: FsLayout, user_home: Option<PathBuf>, actor: String) -> Self {
        let state_path = layout.state_dir.join("installed.toml");
        let state_roots = vec![layout.state_dir.clone()];
        let datadir_roots = vec![layout.datadir.clone()];
        Self {
            layout,
            registry: DriverRegistry::builtin(),
            state_path,
            state_roots,
            datadir_roots,
            user_home,
            actor,
        }
    }

    /// Append an additional datadir root to search after the ones already
    /// registered. Ignored if already present.
    pub fn push_datadir_root(&mut self, root: PathBuf) {
        if !self.datadir_roots.contains(&root) {
            self.datadir_roots.push(root);
        }
    }

    /// Append an additional installed-state root to search for component
    /// installation records and saved component manifests. Receipts still
    /// belong to the manager's primary state root.
    pub fn push_state_root(&mut self, root: PathBuf) {
        if !self.state_roots.contains(&root) {
            self.state_roots.push(root);
        }
    }

    /// Built-in driver registry, for callers that want to introspect
    /// supported frameworks.
    pub fn registry(&self) -> &DriverRegistry {
        &self.registry
    }

    // -- scan ---------------------------------------------------------------

    /// Discover adapter declarations from visible installed component
    /// manifests, merge them with resource directories under the datadir
    /// roots, then annotate each row with driver availability, framework
    /// detection, and receipt state. Read-only.
    ///
    /// # Errors
    ///
    /// [`AdapterError::State`] if the state file cannot be read.
    pub fn scan(&self) -> Result<ScanReport, AdapterError> {
        let state = InstalledState::load(&self.state_path)?;
        let mut entries: BTreeMap<(String, String), ScanEntry> = BTreeMap::new();
        for (component, framework, resource_root) in self.discover_all() {
            let driver = self.registry.get(&framework);
            let driver_available = driver.is_some();
            let framework_detected = driver
                .map(|d| {
                    d.detect(&HostEnv {
                        user_home: self.user_home.clone(),
                    })
                    .detected
                })
                .unwrap_or(false);
            let claim = state.find_adapter_claim(&component, &framework);
            entries.insert(
                (component.clone(), framework.clone()),
                ScanEntry {
                    component,
                    framework,
                    declared: false,
                    resource_root: Some(resource_root),
                    driver_available,
                    framework_detected,
                    enabled: claim.is_some(),
                    claim_status: claim.map(|c| c.status),
                },
            );
        }

        let (declarations, warnings) = self.load_visible_adapter_declarations(&state);
        for declaration in declarations {
            let key = (declaration.component.clone(), declaration.framework.clone());
            if let Some(entry) = entries.get_mut(&key) {
                entry.declared = true;
                continue;
            }
            let driver = self.registry.get(&declaration.framework);
            let driver_available = driver.is_some();
            let framework_detected = driver
                .map(|d| {
                    d.detect(&HostEnv {
                        user_home: self.user_home.clone(),
                    })
                    .detected
                })
                .unwrap_or(false);
            let claim = state.find_adapter_claim(&declaration.component, &declaration.framework);
            entries.insert(
                key,
                ScanEntry {
                    component: declaration.component,
                    framework: declaration.framework,
                    declared: true,
                    resource_root: None,
                    driver_available,
                    framework_detected,
                    enabled: claim.is_some(),
                    claim_status: claim.map(|c| c.status),
                },
            );
        }

        Ok(ScanReport {
            entries: entries.into_values().collect(),
            warnings,
        })
    }

    // -- enable -------------------------------------------------------------

    /// Enable `component`'s adapter for `framework` (resolved automatically
    /// when `None` and exactly one framework is present). When `dry_run`,
    /// returns the plan without mutating any state.
    ///
    /// Takes the install lock for the whole operation.
    ///
    /// # Errors
    ///
    /// [`AdapterError::ComponentNotInstalled`], [`AdapterError::AdapterNotDeclared`],
    /// [`AdapterError::AdapterManifest`], [`AdapterError::UnknownFramework`],
    /// [`AdapterError::AmbiguousFramework`], [`AdapterError::ResourceRootNotFound`],
    /// [`AdapterError::FrameworkNotDetected`], [`AdapterError::BundleInvalid`],
    /// [`AdapterError::FrameworkCli`], [`AdapterError::ClaimValidation`], or
    /// state/lock/log errors.
    pub fn enable(
        &self,
        component: &str,
        framework: Option<&str>,
        dry_run: bool,
    ) -> Result<EnableOutcome, AdapterError> {
        let _lock = InstallLock::acquire(&self.layout.lock_file)?;
        let mut state = InstalledState::load(&self.state_path)?;

        let manifest = self.load_visible_component_manifest(component, &state)?;
        let framework = self.resolve_framework(component, framework, &manifest)?;
        let declared_plugin_id = declared_plugin_id(&manifest, &framework);
        let driver =
            self.registry
                .get(&framework)
                .ok_or_else(|| AdapterError::UnknownFramework {
                    framework: framework.clone(),
                })?;

        let resource_root = self.discover_resource_root(component, &framework).ok_or(
            AdapterError::ResourceRootNotFound {
                component: component.to_string(),
                framework: framework.clone(),
            },
        )?;

        let label = format!("adapter enable {component} {framework}");
        let ops = ManagerOps::new(
            self.central_log(),
            self.actor.clone(),
            install_mode_str(self.layout.mode).to_string(),
            component.to_string(),
            label.clone(),
        );
        let ctx = DriverCtx {
            component: component.to_string(),
            framework: framework.clone(),
            layout: &self.layout,
            resource_root: resource_root.clone(),
            user_home: self.user_home.clone(),
            declared_plugin_id,
            dry_run,
            ops: &ops,
        };

        let bundle = driver.read_bundle(&ctx)?;

        if dry_run {
            let plan = driver.plan_enable(&bundle, &ctx)?;
            return Ok(EnableOutcome::Planned(plan));
        }

        // enable mutates framework state, so the framework must be usable.
        let detect = driver.detect(&HostEnv {
            user_home: self.user_home.clone(),
        });
        if !detect.detected {
            return Err(AdapterError::FrameworkNotDetected {
                framework: framework.clone(),
                reason: detect.reason,
            });
        }

        let claim = driver.prepare_enable(&bundle, &ctx)?;
        // Defense in depth: the driver must not emit a claim that points
        // outside its own declared roots. Reject before persisting.
        claim.validate(&self.layout, &driver.allowed_external_roots(&ctx))?;

        state.upsert_adapter_claim(claim.clone());
        state.save(&self.state_path)?;
        if let Err(err) = driver.apply_enable(&claim, &ctx) {
            let mut failed_claim = claim.clone();
            failed_claim.status = ClaimStatus::CleanupFailed;
            state.upsert_adapter_claim(failed_claim);
            if let Err(save_err) = state.save(&self.state_path) {
                self.log_operation(
                    &label,
                    component,
                    LogStatus::Partial,
                    "adapter enable failed; receipt status update failed",
                    Some(format!(
                        "enable error: {err}; failed to mark receipt cleanup_failed: {save_err}"
                    )),
                );
            } else {
                self.log_operation(
                    &label,
                    component,
                    LogStatus::Failed,
                    "adapter enable failed; receipt kept for cleanup retry",
                    Some(err.to_string()),
                );
            }
            return Err(err);
        }
        self.log_operation(&label, component, LogStatus::Ok, "adapter enabled", None);

        Ok(EnableOutcome::Enabled(Box::new(claim)))
    }

    // -- disable ------------------------------------------------------------

    /// Disable `component`'s adapter for `framework` (resolved from existing
    /// receipts when `None`). Idempotent: disabling something with no
    /// receipt is a successful no-op.
    ///
    /// Takes the install lock for the whole operation.
    ///
    /// # Errors
    ///
    /// [`AdapterError::AmbiguousFramework`] when `framework` is omitted and
    /// the component has receipts for more than one; [`AdapterError::UnknownFramework`],
    /// [`AdapterError::ClaimValidation`], [`AdapterError::FrameworkCli`], or
    /// state/lock/log errors.
    pub fn disable(
        &self,
        component: &str,
        framework: Option<&str>,
    ) -> Result<DisableOutcome, AdapterError> {
        let _lock = InstallLock::acquire(&self.layout.lock_file)?;
        let mut state = InstalledState::load(&self.state_path)?;

        let framework = match framework {
            Some(f) => f.to_string(),
            None => {
                let claimed: Vec<String> = state
                    .adapter_claims_for_component(component)
                    .iter()
                    .map(|c| c.framework.clone())
                    .collect();
                match claimed.len() {
                    0 => {
                        return Ok(DisableOutcome {
                            component: component.to_string(),
                            framework: None,
                            report: DisableReport {
                                cleanup_complete: true,
                                messages: vec![format!(
                                    "component '{component}' has no enabled adapters"
                                )],
                            },
                            claim_removed: false,
                        });
                    }
                    1 => claimed[0].clone(),
                    _ => {
                        return Err(AdapterError::AmbiguousFramework {
                            component: component.to_string(),
                            frameworks: claimed,
                        });
                    }
                }
            }
        };

        let claim = match state.find_adapter_claim(component, &framework) {
            Some(c) => c.clone(),
            None => {
                // Idempotent: nothing to disable.
                return Ok(DisableOutcome {
                    component: component.to_string(),
                    framework: Some(framework.clone()),
                    report: DisableReport {
                        cleanup_complete: true,
                        messages: vec![format!(
                            "no receipt for '{component}/{framework}'; nothing to disable"
                        )],
                    },
                    claim_removed: false,
                });
            }
        };

        let driver =
            self.registry
                .get(&framework)
                .ok_or_else(|| AdapterError::UnknownFramework {
                    framework: framework.clone(),
                })?;

        // resource_root may be gone after an uninstall of the bundle; that
        // is fine — disable must not depend on it. Fall back to the
        // receipt's recorded root for context only.
        let resource_root = self
            .discover_resource_root(component, &framework)
            .unwrap_or_else(|| claim.resource_root.clone());

        let label = format!("adapter disable {component} {framework}");
        let ops = ManagerOps::new(
            self.central_log(),
            self.actor.clone(),
            install_mode_str(self.layout.mode).to_string(),
            component.to_string(),
            label.clone(),
        );
        let ctx = DriverCtx {
            component: component.to_string(),
            framework: framework.clone(),
            layout: &self.layout,
            resource_root,
            user_home: self.user_home.clone(),
            declared_plugin_id: None,
            dry_run: false,
            ops: &ops,
        };

        // Re-validate the receipt before acting on it (forged-state guard).
        claim.validate(&self.layout, &driver.allowed_external_roots(&ctx))?;

        let report = driver.disable(&claim, &ctx)?;
        let claim_removed = report.cleanup_complete;
        if claim_removed {
            state.remove_adapter_claim(component, &framework);
            self.log_operation(&label, component, LogStatus::Ok, "adapter disabled", None);
        } else {
            // Keep the receipt so cleanup can be retried; mark it failed.
            let mut kept = claim;
            kept.status = ClaimStatus::CleanupFailed;
            state.upsert_adapter_claim(kept);
            self.log_operation(
                &label,
                component,
                LogStatus::Failed,
                "adapter cleanup incomplete; receipt kept",
                Some(report.messages.join("; ")),
            );
        }
        state.save(&self.state_path)?;

        Ok(DisableOutcome {
            component: component.to_string(),
            framework: Some(framework),
            report,
            claim_removed,
        })
    }

    // -- status -------------------------------------------------------------

    /// Report status for every receipt, or only those of `component` when
    /// given. Read-only; never mutates state.
    ///
    /// # Errors
    ///
    /// [`AdapterError::ClaimValidation`] if a stored receipt fails
    /// re-validation, or state errors. A missing driver or undetectable
    /// framework is reported in the per-entry conditions, not as an error.
    pub fn status(&self, component: Option<&str>) -> Result<StatusReport, AdapterError> {
        let state = InstalledState::load(&self.state_path)?;
        let mut entries = Vec::new();

        for claim in &state.adapter_claims {
            if let Some(c) = component
                && claim.component != c
            {
                continue;
            }
            let framework = claim.framework.clone();
            let driver = match self.registry.get(&framework) {
                Some(d) => d,
                None => {
                    // No driver: cannot verify. Surface an unverified report
                    // rather than skipping the receipt silently.
                    entries.push(StatusEntry {
                        component: claim.component.clone(),
                        framework,
                        report: unverified_report("no built-in driver for framework"),
                    });
                    continue;
                }
            };

            let resource_root = self
                .discover_resource_root(&claim.component, &framework)
                .unwrap_or_else(|| claim.resource_root.clone());
            let label = format!("adapter status {} {framework}", claim.component);
            let ops = ManagerOps::new(
                self.central_log(),
                self.actor.clone(),
                install_mode_str(self.layout.mode).to_string(),
                claim.component.clone(),
                label,
            );
            let ctx = DriverCtx {
                component: claim.component.clone(),
                framework: framework.clone(),
                layout: &self.layout,
                resource_root,
                user_home: self.user_home.clone(),
                declared_plugin_id: None,
                dry_run: false,
                ops: &ops,
            };

            claim.validate(&self.layout, &driver.allowed_external_roots(&ctx))?;
            let report = driver.status(claim, &ctx)?;
            entries.push(StatusEntry {
                component: claim.component.clone(),
                framework,
                report,
            });
        }

        Ok(StatusReport { entries })
    }

    // -- discovery helpers --------------------------------------------------

    /// Resolve the framework for an operation from the installed manifest:
    /// use the explicit one when declared, else the single declared
    /// framework, else error.
    fn resolve_framework(
        &self,
        component: &str,
        framework: Option<&str>,
        manifest: &ComponentManifest,
    ) -> Result<String, AdapterError> {
        let declared = declared_frameworks(manifest);
        if let Some(f) = framework {
            if declared.iter().any(|decl| decl == f) {
                return Ok(f.to_string());
            }
            return Err(AdapterError::AdapterNotDeclared {
                component: component.to_string(),
                framework: f.to_string(),
            });
        }
        match declared.len() {
            0 => Err(AdapterError::AdapterNotDeclared {
                component: component.to_string(),
                framework: "<any>".to_string(),
            }),
            1 => Ok(declared[0].clone()),
            _ => Err(AdapterError::AmbiguousFramework {
                component: component.to_string(),
                frameworks: declared,
            }),
        }
    }

    /// Load the installed component manifest from the first visible state
    /// root that records `component` as installed.
    fn load_visible_component_manifest(
        &self,
        component: &str,
        current_state: &InstalledState,
    ) -> Result<ComponentManifest, AdapterError> {
        let Some(state_dir) = self.find_component_state_dir(component, current_state)? else {
            return Err(AdapterError::ComponentNotInstalled {
                component: component.to_string(),
            });
        };
        let path = installed_component_manifest_path(&state_dir, component);
        let manifest =
            ComponentManifest::from_file(&path).map_err(|err| AdapterError::AdapterManifest {
                component: component.to_string(),
                path: path.clone(),
                reason: err.to_string(),
            })?;
        if manifest.component.name != component {
            return Err(AdapterError::AdapterManifest {
                component: component.to_string(),
                path,
                reason: format!("manifest declares component '{}'", manifest.component.name),
            });
        }
        Ok(manifest)
    }

    /// First visible state root whose installed state contains `component`.
    fn find_component_state_dir(
        &self,
        component: &str,
        current_state: &InstalledState,
    ) -> Result<Option<PathBuf>, AdapterError> {
        for state_dir in &self.state_roots {
            let installed = if state_dir == &self.layout.state_dir {
                current_state
                    .find_object(ObjectKind::Component, component)
                    .is_some()
            } else {
                let state_path = state_dir.join("installed.toml");
                InstalledState::load(&state_path)?
                    .find_object(ObjectKind::Component, component)
                    .is_some()
            };
            if installed {
                return Ok(Some(state_dir.clone()));
            }
        }
        Ok(None)
    }

    /// Adapter declarations from installed component manifests in visible
    /// state roots. Earlier roots shadow later roots for the same component,
    /// matching enable's user-before-system resolution.
    fn load_visible_adapter_declarations(
        &self,
        current_state: &InstalledState,
    ) -> (Vec<AdapterDecl>, Vec<String>) {
        let mut declarations = BTreeSet::new();
        let mut seen_components = BTreeSet::new();
        let mut warnings = Vec::new();

        for state_dir in &self.state_roots {
            let state_path = state_dir.join("installed.toml");
            let state = if state_dir == &self.layout.state_dir {
                current_state.clone()
            } else {
                match InstalledState::load(&state_path) {
                    Ok(state) => state,
                    Err(err) => {
                        warnings.push(format!(
                            "failed to load installed state at {}: {err}",
                            state_path.display()
                        ));
                        continue;
                    }
                }
            };

            for object in state
                .objects
                .iter()
                .filter(|object| object.kind == ObjectKind::Component)
                .filter(|object| object.status == ObjectStatus::Installed)
            {
                if !seen_components.insert(object.name.clone()) {
                    continue;
                }

                let path = installed_component_manifest_path(state_dir, &object.name);
                if !path.exists() {
                    warnings.push(format!(
                        "installed component '{}' has no local component manifest at {}",
                        object.name,
                        path.display()
                    ));
                    continue;
                }
                let manifest = match ComponentManifest::from_file(&path) {
                    Ok(manifest) => manifest,
                    Err(err) => {
                        warnings.push(format!(
                            "failed to read installed component manifest for '{}' at {}: {err}",
                            object.name,
                            path.display()
                        ));
                        continue;
                    }
                };
                if manifest.component.name != object.name {
                    warnings.push(format!(
                        "installed component manifest at {} declares component '{}', expected '{}'",
                        path.display(),
                        manifest.component.name,
                        object.name
                    ));
                    continue;
                }

                for framework in declared_frameworks(&manifest) {
                    declarations.insert(AdapterDecl {
                        component: object.name.clone(),
                        framework,
                    });
                }
            }
        }

        (declarations.into_iter().collect(), warnings)
    }

    /// First datadir root that contains
    /// `adapters/<component>/<framework>/` as a directory.
    fn discover_resource_root(&self, component: &str, framework: &str) -> Option<PathBuf> {
        for root in &self.datadir_roots {
            let candidate = root.join("adapters").join(component).join(framework);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        None
    }

    /// Every `(component, framework, resource_root)` discoverable under the
    /// datadir roots, deduped on `(component, framework)` and sorted.
    fn discover_all(&self) -> Vec<(String, String, PathBuf)> {
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        let mut out: Vec<(String, String, PathBuf)> = Vec::new();
        for root in &self.datadir_roots {
            let adapters = root.join("adapters");
            let Ok(components) = adapters.read_dir() else {
                continue;
            };
            for comp_entry in components.flatten() {
                if !comp_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let component = comp_entry.file_name().to_string_lossy().into_owned();
                let Ok(frameworks) = comp_entry.path().read_dir() else {
                    continue;
                };
                for fw_entry in frameworks.flatten() {
                    if !fw_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        continue;
                    }
                    let framework = fw_entry.file_name().to_string_lossy().into_owned();
                    if seen.insert((component.clone(), framework.clone())) {
                        out.push((component.clone(), framework, fw_entry.path()));
                    }
                }
            }
        }
        out.sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));
        out
    }

    // -- logging ------------------------------------------------------------

    fn central_log(&self) -> CentralLog {
        CentralLog::open(self.layout.central_log.clone())
    }

    /// Append one operation-summary record. Logging failures are
    /// swallowed: an audit-log hiccup must not fail an otherwise-successful
    /// adapter operation.
    fn log_operation(
        &self,
        command: &str,
        component: &str,
        status: LogStatus,
        message: &str,
        detail: Option<String>,
    ) {
        let severity = match status {
            LogStatus::Ok => Severity::Info,
            LogStatus::Partial => Severity::Warn,
            LogStatus::Failed | LogStatus::RolledBack => Severity::Error,
        };
        let now = now_iso8601();
        let record = LogRecord {
            kind: LogKind::Operation,
            operation_id: None,
            command: command.to_string(),
            source: LOG_SOURCE.to_string(),
            component: Some(component.to_string()),
            severity,
            message: message.to_string(),
            actor: self.actor.clone(),
            install_mode: Some(install_mode_str(self.layout.mode).to_string()),
            started_at: now.clone(),
            finished_at: Some(now),
            status: Some(status),
            objects: vec![component.to_string()],
            backup_ids: Vec::new(),
            warnings: detail.into_iter().collect(),
            details: serde_json::Value::Null,
        };
        let _ = self.central_log().append(&record);
    }
}

// ---------------------------------------------------------------------------
// Controlled IO
// ---------------------------------------------------------------------------

/// The Manager's [`AdapterOps`] implementation: spawns framework CLIs with
/// a timeout, captures and truncates their output, and records each
/// invocation in the central log. The argv is executed directly (no
/// shell), so receipt-derived data can never inject extra commands.
struct ManagerOps {
    log: CentralLog,
    actor: String,
    install_mode: String,
    component: String,
    /// Human-readable operation label for the log `command` field.
    label: String,
}

impl ManagerOps {
    fn new(
        log: CentralLog,
        actor: String,
        install_mode: String,
        component: String,
        label: String,
    ) -> Self {
        Self {
            log,
            actor,
            install_mode,
            component,
            label,
        }
    }

    /// Record one framework CLI invocation. Best-effort; a log failure
    /// never propagates.
    fn record(&self, cmd: &FrameworkCommand, output: &CliOutput) {
        let severity = if output.success() {
            Severity::Debug
        } else {
            Severity::Warn
        };
        let argv = std::iter::once(cmd.program.clone())
            .chain(cmd.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        let now = now_iso8601();
        let record = LogRecord {
            kind: LogKind::Operation,
            operation_id: None,
            command: self.label.clone(),
            source: LOG_SOURCE.to_string(),
            component: Some(self.component.clone()),
            severity,
            message: format!("framework cli: {argv}"),
            actor: self.actor.clone(),
            install_mode: Some(self.install_mode.clone()),
            started_at: now.clone(),
            finished_at: Some(now),
            status: Some(if output.success() {
                LogStatus::Ok
            } else {
                LogStatus::Failed
            }),
            objects: vec![self.component.clone()],
            backup_ids: Vec::new(),
            warnings: Vec::new(),
            details: serde_json::json!({
                "exit": output.status,
                "timed_out": output.timed_out,
            }),
        };
        let _ = self.log.append(&record);
    }
}

impl AdapterOps for ManagerOps {
    fn run_framework_cli(&self, cmd: FrameworkCommand) -> Result<CliOutput, AdapterError> {
        let output = run_capture(&cmd)?;
        self.record(&cmd, &output);
        Ok(output)
    }
}

/// Spawn `cmd` as a direct argv (no shell), enforce its timeout, and return
/// truncated output. The child's stdout/stderr are drained on separate
/// threads so a full pipe can never deadlock the wait loop.
fn run_capture(cmd: &FrameworkCommand) -> Result<CliOutput, AdapterError> {
    let mut command = Command::new(&cmd.program);
    command
        .args(&cmd.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for key in &cmd.env_remove {
        command.env_remove(key);
    }
    for (key, value) in &cmd.env_set {
        command.env(key, value);
    }
    if !cmd.path_prepend.is_empty() {
        command.env("PATH", prepend_path(&cmd.path_prepend));
    }

    let mut child = crate::process::spawn_retry_etxtbsy(&mut command).map_err(|source| {
        AdapterError::FrameworkCli {
            program: cmd.program.clone(),
            reason: format!("failed to spawn: {source}"),
        }
    })?;

    let stdout_handle = child.stdout.take().map(|r| spawn_drain(r, OUTPUT_CAP));
    let stderr_handle = child.stderr.take().map(|r| spawn_drain(r, OUTPUT_CAP));

    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() >= cmd.timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(source) => {
                return Err(AdapterError::FrameworkCli {
                    program: cmd.program.clone(),
                    reason: format!("failed to wait: {source}"),
                });
            }
        }
    };

    let stdout = collect_drain(stdout_handle);
    let stderr = collect_drain(stderr_handle);

    Ok(CliOutput {
        status: status.and_then(|s| s.code()),
        timed_out,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Build a `PATH` value with `prepend` dirs in front of the current one.
fn prepend_path(prepend: &[PathBuf]) -> std::ffi::OsString {
    let mut parts: Vec<PathBuf> = prepend.to_vec();
    if let Some(existing) = std::env::var_os("PATH") {
        parts.extend(std::env::split_paths(&existing));
    }
    // join_paths only fails if a component contains the path separator,
    // which our dirs do not; fall back to the prepend dirs alone.
    std::env::join_paths(&parts)
        .unwrap_or_else(|_| std::env::join_paths(prepend).unwrap_or_default())
}

/// Drain a child pipe to EOF on its own thread, keeping at most `cap`
/// bytes. Reading to EOF (even past the cap) keeps the child from blocking
/// on a full pipe.
fn spawn_drain<R: Read + Send + 'static>(mut reader: R, cap: usize) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if kept.len() < cap {
                        let take = (cap - kept.len()).min(n);
                        kept.extend_from_slice(&chunk[..take]);
                    }
                }
                Err(_) => break,
            }
        }
        kept
    })
}

/// Join a drain thread, returning its captured bytes (empty on panic or
/// absent pipe).
fn collect_drain(handle: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

/// ISO 8601 UTC timestamp, second precision.
fn now_iso8601() -> String {
    use chrono::{SecondsFormat, Utc};
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Stable string for the central log's `install_mode` field.
fn install_mode_str(mode: InstallMode) -> &'static str {
    match mode {
        InstallMode::System => "system",
        InstallMode::User => "user",
    }
}

fn installed_component_manifest_path(state_dir: &Path, component: &str) -> PathBuf {
    state_dir
        .join(INSTALLED_COMPONENT_MANIFESTS_SUBDIR)
        .join(component)
        .join(INSTALLED_COMPONENT_MANIFEST_FILE)
}

fn declared_frameworks(manifest: &ComponentManifest) -> Vec<String> {
    let mut set = BTreeSet::new();
    for adapter in &manifest.adapters {
        if let Some(framework) = adapter.framework.as_deref().map(str::trim)
            && !framework.is_empty()
        {
            set.insert(framework.to_string());
        }
    }
    set.into_iter().collect()
}

fn declared_plugin_id(manifest: &ComponentManifest, framework: &str) -> Option<String> {
    manifest
        .adapters
        .iter()
        .find(|adapter| adapter.framework.as_deref().map(str::trim) == Some(framework))
        .and_then(|adapter| adapter.plugin_id.as_deref())
        .map(str::trim)
        .filter(|plugin_id| !plugin_id.is_empty())
        .map(str::to_string)
}

/// A status report for a receipt that cannot be verified at all (e.g. no
/// driver). Reports `Unknown` rather than faking a healthy/absent verdict.
fn unverified_report(reason: &str) -> AdapterStatusReport {
    use super::driver::{AdapterCondition, AdapterConditionKind, AdapterSummary, ConditionStatus};
    AdapterStatusReport {
        summary: AdapterSummary::Unknown,
        conditions: vec![AdapterCondition {
            kind: AdapterConditionKind::VerificationSupported,
            status: ConditionStatus::False,
            reason: Some(reason.to_string()),
            resource: None,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepend_path_puts_dirs_in_front() {
        // SAFETY: single-threaded test mutating PATH; restored after.
        let saved = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", "/usr/bin:/bin");
        }
        let joined = prepend_path(&[PathBuf::from("/opt/a"), PathBuf::from("/opt/b")]);
        let dirs: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(dirs[0], PathBuf::from("/opt/a"));
        assert_eq!(dirs[1], PathBuf::from("/opt/b"));
        assert!(dirs.contains(&PathBuf::from("/usr/bin")));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    #[test]
    fn run_capture_captures_stdout_and_exit() {
        let cmd = FrameworkCommand {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "printf hello; exit 0".to_string()],
            env_set: Vec::new(),
            env_remove: Vec::new(),
            path_prepend: Vec::new(),
            timeout: Duration::from_secs(5),
        };
        let out = run_capture(&cmd).expect("run");
        assert!(out.success());
        assert_eq!(out.stdout, "hello");
        assert!(!out.timed_out);
    }

    #[test]
    fn run_capture_reports_nonzero_exit() {
        let cmd = FrameworkCommand {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 3".to_string()],
            env_set: Vec::new(),
            env_remove: Vec::new(),
            path_prepend: Vec::new(),
            timeout: Duration::from_secs(5),
        };
        let out = run_capture(&cmd).expect("run");
        assert_eq!(out.status, Some(3));
        assert!(!out.success());
    }

    #[test]
    fn run_capture_times_out_and_kills() {
        let cmd = FrameworkCommand {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "sleep 30".to_string()],
            env_set: Vec::new(),
            env_remove: Vec::new(),
            path_prepend: Vec::new(),
            timeout: Duration::from_millis(150),
        };
        let out = run_capture(&cmd).expect("run");
        assert!(out.timed_out, "expected timeout");
        assert!(!out.success());
    }

    #[test]
    fn spawn_failure_is_framework_cli_error() {
        let cmd = FrameworkCommand {
            program: "/no/such/binary/xyz".to_string(),
            args: Vec::new(),
            env_set: Vec::new(),
            env_remove: Vec::new(),
            path_prepend: Vec::new(),
            timeout: Duration::from_secs(5),
        };
        let err = run_capture(&cmd).expect_err("spawn must fail");
        assert!(matches!(err, AdapterError::FrameworkCli { .. }));
    }
}
