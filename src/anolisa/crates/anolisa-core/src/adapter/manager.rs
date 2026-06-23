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
//! Resource discovery has two modes, tried in order:
//!
//! 1. **Contract-driven** — when the installed component manifest declares
//!    an `[[adapters]]` entry with a `dest` field, that template is expanded
//!    against each visible datadir root. The first root whose expanded path
//!    exists as a directory wins. When `dest` is declared but no directory
//!    exists, enable fails with an explicit error and scan shows the adapter
//!    as declared but absent — convention discovery is **not** used as a
//!    silent fallback.
//!
//! 2. **Convention** — `{datadir}/adapters/<component>/<framework>/`.
//!    Multiple datadir roots may be searched (e.g. the user datadir
//!    preferred over the system one); the first root that contains the
//!    directory wins.

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
    /// The `adapter_type` declared in the component manifest for this
    /// adapter entry, when the manifest was readable (`None` when the
    /// entry came from resource-directory discovery only).
    pub adapter_type: Option<String>,
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
    /// The `adapter_type` from the manifest entry, if present.
    adapter_type: Option<String>,
    /// Raw `dest` template from the manifest entry, before placeholder
    /// expansion. Used by `scan` and `enable` to resolve the
    /// contract-driven resource root.
    dest: Option<String>,
    /// Datadir roots from the [`VisibleRoot`] where this declaration's
    /// component contract was resolved. `dest` expansion is scoped to
    /// only these roots.
    scoped_datadir_roots: Vec<PathBuf>,
}

/// A state root paired with the datadir roots it may use for component
/// contract resolution. Contract lookup for a component found in this
/// state root searches only the paired datadir roots — not datadirs
/// from other visible roots — so a user-scope component cannot
/// silently fall back to a system-scope contract.
#[derive(Debug, Clone)]
pub struct VisibleRoot {
    /// State directory containing `installed.toml`.
    pub state_dir: PathBuf,
    /// Datadir roots searched for component contracts when a component
    /// is found in this state root. Searched in order; first match wins.
    pub contract_datadir_roots: Vec<PathBuf>,
}

/// Trusted orchestrator for adapter enable/disable/status/scan.
pub struct AdapterManager {
    layout: FsLayout,
    registry: DriverRegistry,
    state_path: PathBuf,
    /// Paired visible roots, in preference order. Each pairs a state root
    /// with its contract-visible datadir roots. Receipts are always
    /// written only to [`Self::state_path`] (the primary root's state).
    visible_roots: Vec<VisibleRoot>,
    /// All datadir roots (across all visible roots, deduped), used for
    /// resource-directory discovery (`adapters/<component>/<framework>/`).
    /// Resource discovery is scope-independent: a user-mode enable may
    /// use adapter resources from a system-installed package.
    all_datadir_roots: Vec<PathBuf>,
    user_home: Option<PathBuf>,
    /// Identity recorded as the central-log actor.
    actor: String,
}

impl AdapterManager {
    /// Build a manager for the given layout. The primary visible root
    /// pairs `layout.state_dir` with `layout.datadir`. Use
    /// [`Self::push_visible_root`] to add fallback roots (e.g. system
    /// roots when running in user mode).
    pub fn new(layout: FsLayout, user_home: Option<PathBuf>, actor: String) -> Self {
        let state_path = layout.state_dir.join("installed.toml");
        let primary = VisibleRoot {
            state_dir: layout.state_dir.clone(),
            contract_datadir_roots: vec![layout.datadir.clone()],
        };
        let all_datadir_roots = vec![layout.datadir.clone()];
        Self {
            layout,
            registry: DriverRegistry::builtin(),
            state_path,
            visible_roots: vec![primary],
            all_datadir_roots,
            user_home,
            actor,
        }
    }

    /// Add a visible root with explicit contract-scope datadir pairing.
    ///
    /// The state root is appended to the search order (lower priority
    /// than roots registered earlier). Its `contract_datadir_roots` are
    /// only used for contract resolution when a component is found in
    /// this state root — they are not mixed into other roots' contract
    /// scope.
    ///
    /// All datadir roots are also added to the global resource-discovery
    /// set (for `adapters/<component>/<framework>/` lookups), since
    /// adapter resource directories are scope-independent.
    pub fn push_visible_root(&mut self, root: VisibleRoot) {
        if self
            .visible_roots
            .iter()
            .any(|r| r.state_dir == root.state_dir)
        {
            return;
        }
        for dd in &root.contract_datadir_roots {
            if !self.all_datadir_roots.contains(dd) {
                self.all_datadir_roots.push(dd.clone());
            }
        }
        self.visible_roots.push(root);
    }

    /// Add a datadir root to the primary visible root's contract scope
    /// and to the global resource-discovery set. Use this when the
    /// system-mode packaged datadir differs from `layout.datadir`
    /// (e.g. exe-sibling `/usr/share/anolisa/` vs install prefix
    /// `/usr/local/share/anolisa/`).
    pub fn push_primary_datadir_root(&mut self, root: PathBuf) {
        if let Some(primary) = self.visible_roots.first_mut()
            && !primary.contract_datadir_roots.contains(&root)
        {
            primary.contract_datadir_roots.push(root.clone());
        }
        if !self.all_datadir_roots.contains(&root) {
            self.all_datadir_roots.push(root);
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
                    adapter_type: None,
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
                entry.adapter_type = declaration.adapter_type.clone();
                // When the contract declares a custom dest, the
                // contract-resolved path is authoritative — override the
                // convention-discovered root (which may point elsewhere).
                // If the contract dest directory does not exist, show
                // resource_root = None (declared yes / resource absent).
                if declaration.dest.is_some() {
                    entry.resource_root = declaration.dest.as_deref().and_then(|dest| {
                        self.resolve_declared_scan_root(
                            &declaration.component,
                            dest,
                            &declaration.scoped_datadir_roots,
                        )
                    });
                }
                continue;
            }

            // Not found in directory discovery — resolve from contract
            // dest, if present.
            let resource_root = declaration.dest.as_deref().and_then(|dest| {
                self.resolve_declared_scan_root(
                    &declaration.component,
                    dest,
                    &declaration.scoped_datadir_roots,
                )
            });

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
                    resource_root,
                    driver_available,
                    framework_detected,
                    adapter_type: declaration.adapter_type,
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
    /// [`AdapterError::AmbiguousFramework`], [`AdapterError::UnsupportedAdapterType`],
    /// [`AdapterError::ResourceRootNotFound`],
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

        let (manifest, scoped_datadir_roots) =
            self.load_visible_component_manifest(component, &state)?;
        let framework = self.resolve_framework(component, framework, &manifest)?;

        // Fail-closed: only `plugin` (or absent/None, defaulting to
        // plugin) is supported. Any other adapter_type must be rejected
        // before we invoke a driver.
        let adapter_type = declared_adapter_type(&manifest, &framework);
        if let Some(ref at) = adapter_type
            && at != "plugin"
        {
            return Err(AdapterError::UnsupportedAdapterType {
                component: component.to_string(),
                framework: framework.clone(),
                adapter_type: at.clone(),
            });
        }

        let declared_plugin_id = declared_plugin_id(&manifest, &framework);
        let skill_specs = declared_skills(&manifest, &framework);
        let config = declared_config(&manifest, &framework);
        let bundle_entry = declared_bundle_entry(&manifest, &framework);

        for skill in &skill_specs {
            super::claim::validate_skill_name(&skill.name).map_err(|mut err| {
                if let AdapterError::InvalidAdapterInput {
                    component: ref mut c,
                    framework: ref mut f,
                    ..
                } = err
                {
                    *c = component.to_string();
                    *f = framework.clone();
                }
                err
            })?;
        }
        for cfg in &config {
            super::claim::validate_config_key(&cfg.key).map_err(|mut err| {
                if let AdapterError::InvalidAdapterInput {
                    component: ref mut c,
                    framework: ref mut f,
                    ..
                } = err
                {
                    *c = component.to_string();
                    *f = framework.clone();
                }
                err
            })?;
        }

        let driver =
            self.registry
                .get(&framework)
                .ok_or_else(|| AdapterError::UnknownFramework {
                    framework: framework.clone(),
                })?;

        let (resource_root, effective_datadir) =
            self.resolve_resource_root(component, &framework, &manifest, &scoped_datadir_roots)?;
        let skills = resolve_skill_sources(
            skill_specs,
            &self.layout,
            &effective_datadir,
            component,
            &framework,
            &resource_root,
        )?;

        let label = format!("adapter enable {component} {framework}");
        // Two-phase ManagerOps: first build a read-only ops (no allowed
        // roots) to construct the DriverCtx needed for
        // allowed_external_roots; then rebuild with the computed roots
        // for the mutable phase.
        let probe_ops = ManagerOps::new(
            self.central_log(),
            self.actor.clone(),
            install_mode_str(self.layout.mode).to_string(),
            component.to_string(),
            label.clone(),
            vec![resource_root.clone()],
        );
        let probe_ctx = DriverCtx {
            component: component.to_string(),
            framework: framework.clone(),
            layout: &self.layout,
            resource_root: resource_root.clone(),
            user_home: self.user_home.clone(),
            declared_plugin_id: declared_plugin_id.clone(),
            declared_skills: Vec::new(),
            declared_config: Vec::new(),
            declared_bundle_entry: None,
            dry_run,
            ops: &probe_ops,
        };
        let mut allowed_roots = driver.allowed_external_roots(&probe_ctx);
        allowed_roots.push(resource_root.clone());
        // Skill sources that live outside the resource root (e.g.
        // `{datadir}/skills/<name>/`) must also be readable by the
        // Manager's controlled IO.
        for skill in &skills {
            if let Some(ref src) = skill.source {
                if !allowed_roots.iter().any(|r| src.starts_with(r)) {
                    allowed_roots.push(src.clone());
                }
            }
        }
        drop(probe_ctx);
        drop(probe_ops);

        let ops = ManagerOps::new(
            self.central_log(),
            self.actor.clone(),
            install_mode_str(self.layout.mode).to_string(),
            component.to_string(),
            label.clone(),
            allowed_roots,
        );
        let ctx = DriverCtx {
            component: component.to_string(),
            framework: framework.clone(),
            layout: &self.layout,
            resource_root: resource_root.clone(),
            user_home: self.user_home.clone(),
            declared_plugin_id,
            declared_skills: skills,
            declared_config: config,
            declared_bundle_entry: bundle_entry,
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
            .map(|(path, _)| path)
            .unwrap_or_else(|| claim.resource_root.clone());

        let label = format!("adapter disable {component} {framework}");
        let probe_ops = ManagerOps::new(
            self.central_log(),
            self.actor.clone(),
            install_mode_str(self.layout.mode).to_string(),
            component.to_string(),
            label.clone(),
            vec![resource_root.clone()],
        );
        let probe_ctx = DriverCtx {
            component: component.to_string(),
            framework: framework.clone(),
            layout: &self.layout,
            resource_root: resource_root.clone(),
            user_home: self.user_home.clone(),
            declared_plugin_id: None,
            declared_skills: Vec::new(),
            declared_config: Vec::new(),
            declared_bundle_entry: None,
            dry_run: false,
            ops: &probe_ops,
        };
        let mut allowed_roots = driver.allowed_external_roots(&probe_ctx);
        allowed_roots.push(resource_root.clone());
        drop(probe_ctx);
        drop(probe_ops);

        let ops = ManagerOps::new(
            self.central_log(),
            self.actor.clone(),
            install_mode_str(self.layout.mode).to_string(),
            component.to_string(),
            label.clone(),
            allowed_roots,
        );
        let ctx = DriverCtx {
            component: component.to_string(),
            framework: framework.clone(),
            layout: &self.layout,
            resource_root,
            user_home: self.user_home.clone(),
            declared_plugin_id: None,
            declared_skills: Vec::new(),
            declared_config: Vec::new(),
            declared_bundle_entry: None,
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
                .map(|(path, _)| path)
                .unwrap_or_else(|| claim.resource_root.clone());
            let label = format!("adapter status {} {framework}", claim.component);
            let ops = ManagerOps::new(
                self.central_log(),
                self.actor.clone(),
                install_mode_str(self.layout.mode).to_string(),
                claim.component.clone(),
                label,
                vec![resource_root.clone()],
            );
            let ctx = DriverCtx {
                component: claim.component.clone(),
                framework: framework.clone(),
                layout: &self.layout,
                resource_root,
                user_home: self.user_home.clone(),
                declared_plugin_id: None,
                declared_skills: Vec::new(),
                declared_config: Vec::new(),
                declared_bundle_entry: None,
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

    /// Load the component contract for an installed component and return
    /// the matched visible root's contract datadir roots.
    ///
    /// The component must be recorded as installed in a visible state root.
    /// Once that gate passes, the contract is resolved using only the
    /// matched visible root's paired state and datadir roots — a user-scope
    /// component never falls back to a system-scope contract.
    ///
    /// The returned datadir roots should be used to scope layout placeholder
    /// expansion for `dest` fields in the manifest.
    fn load_visible_component_manifest(
        &self,
        component: &str,
        current_state: &InstalledState,
    ) -> Result<(ComponentManifest, Vec<PathBuf>), AdapterError> {
        let vr = self
            .find_component_visible_root(component, current_state)?
            .ok_or_else(|| AdapterError::ComponentNotInstalled {
                component: component.to_string(),
            })?;

        let manifest = super::contract::resolve_component_contract(
            component,
            std::slice::from_ref(&vr.state_dir),
            &vr.contract_datadir_roots,
        )
        .map_err(|err| map_contract_error(component, err))?;

        if manifest.component.name != component {
            return Err(AdapterError::AdapterManifest {
                component: component.to_string(),
                path: PathBuf::new(),
                reason: format!("manifest declares component '{}'", manifest.component.name),
            });
        }
        Ok((manifest, vr.contract_datadir_roots.clone()))
    }

    /// First visible root whose installed state contains `component` in
    /// an adapter-visible status ([`Installed`](ObjectStatus::Installed) or
    /// [`Adopted`](ObjectStatus::Adopted)). Returns the full
    /// [`VisibleRoot`] so callers can scope contract resolution to the
    /// paired datadir roots.
    fn find_component_visible_root(
        &self,
        component: &str,
        current_state: &InstalledState,
    ) -> Result<Option<&VisibleRoot>, AdapterError> {
        for vr in &self.visible_roots {
            let visible = if vr.state_dir == self.layout.state_dir {
                current_state
                    .find_object(ObjectKind::Component, component)
                    .is_some_and(|obj| is_adapter_visible_status(obj.status))
            } else {
                let state_path = vr.state_dir.join("installed.toml");
                InstalledState::load(&state_path)?
                    .find_object(ObjectKind::Component, component)
                    .is_some_and(|obj| is_adapter_visible_status(obj.status))
            };
            if visible {
                return Ok(Some(vr));
            }
        }
        Ok(None)
    }

    /// Adapter declarations from component contracts visible to the
    /// manager. Uses the same scope-paired contract resolution as `enable`
    /// so scan and enable agree.
    ///
    /// When a component appears in multiple visible roots (e.g. user and
    /// system), only the first (highest-priority) root owns the
    /// resolution — its paired state snapshot and datadir roots are
    /// searched. A lower-priority root's contract is never used as a
    /// fallback.
    fn load_visible_adapter_declarations(
        &self,
        current_state: &InstalledState,
    ) -> (Vec<AdapterDecl>, Vec<String>) {
        let mut declarations = BTreeSet::new();
        // Map component name → the VisibleRoot where it was first seen.
        let mut component_vr: BTreeMap<String, &VisibleRoot> = BTreeMap::new();
        let mut warnings = Vec::new();

        for vr in &self.visible_roots {
            let state_path = vr.state_dir.join("installed.toml");
            let state = if vr.state_dir == self.layout.state_dir {
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
                .filter(|object| is_adapter_visible_status(object.status))
            {
                component_vr.entry(object.name.clone()).or_insert(vr);
            }
        }

        for (component, vr) in &component_vr {
            let manifest = match super::contract::resolve_component_contract(
                component,
                std::slice::from_ref(&vr.state_dir),
                &vr.contract_datadir_roots,
            ) {
                Ok(m) => m,
                Err(super::contract::ContractError::Unavailable { .. }) => {
                    let other_scope_exists = self.visible_roots.iter().any(|other| {
                        other.state_dir != vr.state_dir
                            && super::contract::resolve_component_contract(
                                component,
                                std::slice::from_ref(&other.state_dir),
                                &other.contract_datadir_roots,
                            )
                            .is_ok()
                    });
                    if other_scope_exists {
                        warnings.push(format!(
                            "installed component '{component}' has no component contract in its scope; a contract exists in another scope but was not used because the component is scoped to {}", vr.state_dir.display()
                        ));
                    } else {
                        warnings.push(format!(
                            "installed component '{component}' has no component contract; adapter declarations unavailable"
                        ));
                    }
                    continue;
                }
                Err(err) => {
                    warnings.push(format!(
                        "failed to read component contract for '{component}': {err}"
                    ));
                    continue;
                }
            };
            if manifest.component.name != component.as_str() {
                warnings.push(format!(
                    "component contract for '{component}' declares component '{}', expected '{component}'",
                    manifest.component.name,
                ));
                continue;
            }

            for adapter in &manifest.adapters {
                if let Some(framework) = adapter.framework.as_deref().map(str::trim)
                    && !framework.is_empty()
                {
                    declarations.insert(AdapterDecl {
                        component: component.clone(),
                        framework: framework.to_string(),
                        adapter_type: adapter.adapter_type.clone(),
                        dest: adapter
                            .dest
                            .as_deref()
                            .map(str::trim)
                            .filter(|d| !d.is_empty())
                            .map(str::to_string),
                        scoped_datadir_roots: vr.contract_datadir_roots.clone(),
                    });
                }
            }
        }

        (declarations.into_iter().collect(), warnings)
    }

    /// Expand a `dest` template from a component contract against a
    /// specific datadir root. The template may use layout placeholders
    /// (`{datadir}`, `{etcdir}`, …) and the extra variable `{component}`.
    ///
    /// Returns `None` when the template is absent or empty.
    fn expand_dest_template(
        &self,
        dest_template: &str,
        component: &str,
        datadir: &Path,
    ) -> Result<PathBuf, AdapterError> {
        let mut layout = self.layout.clone();
        layout.datadir = datadir.to_path_buf();
        super::expand_layout_placeholders(dest_template, &layout, &[("component", component)])
    }

    /// Resolve the adapter resource root for a component/framework using
    /// the contract `dest` field first, then the convention discovery
    /// path as fallback.
    ///
    /// `scoped_datadir_roots` are the datadir roots from the
    /// [`VisibleRoot`] that owns this component's contract. Only these
    /// roots are searched for contract-driven `dest` expansion — this
    /// prevents a user-scope component from silently discovering a
    /// system-scope resource (or vice-versa).
    ///
    /// Returns `(resource_root, effective_datadir)`. The
    /// `effective_datadir` is the datadir root whose `{datadir}`
    /// expansion produced the winning path — callers should use it for
    /// further placeholder expansion (skill sources) so `{datadir}`
    /// stays consistent across the component's scope.
    ///
    /// Contract-driven resolution (`dest` present):
    /// - Expands the `dest` template against each scoped datadir root.
    /// - Returns the first expanded path that exists as a directory.
    /// - When no expanded path exists, returns
    ///   [`AdapterError::ContractResourceRootNotFound`].
    ///
    /// Convention fallback (`dest` absent):
    /// - Searches `{datadir}/adapters/<component>/<framework>/` across
    ///   **all** datadir roots via [`Self::discover_resource_root`].
    ///   The `effective_datadir` is `self.layout.datadir` (the primary
    ///   root) since convention discovery is scope-independent.
    fn resolve_resource_root(
        &self,
        component: &str,
        framework: &str,
        manifest: &ComponentManifest,
        scoped_datadir_roots: &[PathBuf],
    ) -> Result<(PathBuf, PathBuf), AdapterError> {
        let dest_template = declared_dest(manifest, framework);
        match dest_template {
            Some(template) => {
                let mut last_expanded = None;
                for datadir in scoped_datadir_roots {
                    match self.expand_dest_template(&template, component, datadir) {
                        Ok(path) if path.is_dir() => return Ok((path, datadir.clone())),
                        Ok(path) => {
                            last_expanded = Some((path, datadir.clone()));
                        }
                        Err(_) => continue,
                    }
                }
                let path = match last_expanded {
                    Some((p, _)) => p,
                    None => {
                        // All expansions failed (unknown placeholder etc.)
                        // — try expanding with the primary layout for the
                        // error message.
                        super::expand_layout_placeholders(
                            &template,
                            &self.layout,
                            &[("component", component)],
                        )?
                    }
                };
                Err(AdapterError::ContractResourceRootNotFound {
                    component: component.to_string(),
                    framework: framework.to_string(),
                    path,
                })
            }
            None => self.discover_resource_root(component, framework).ok_or(
                AdapterError::ResourceRootNotFound {
                    component: component.to_string(),
                    framework: framework.to_string(),
                },
            ),
        }
    }

    /// Resolve the contract-declared resource root for a declared adapter
    /// during scan. Returns `Some(path)` only when the expanded `dest`
    /// directory exists on disk; returns `None` when the template cannot
    /// be expanded or the directory is absent.
    ///
    /// `scoped_datadir_roots` limits expansion to the visible root that
    /// owns the component's contract.
    fn resolve_declared_scan_root(
        &self,
        component: &str,
        dest_template: &str,
        scoped_datadir_roots: &[PathBuf],
    ) -> Option<PathBuf> {
        for datadir in scoped_datadir_roots {
            if let Ok(path) = self.expand_dest_template(dest_template, component, datadir) {
                if path.is_dir() {
                    return Some(path);
                }
            }
        }
        None
    }

    /// First datadir root that contains
    /// `adapters/<component>/<framework>/` as a directory.
    ///
    /// Returns `(resource_path, datadir_root)` so callers know which
    /// datadir root the resource came from.
    fn discover_resource_root(
        &self,
        component: &str,
        framework: &str,
    ) -> Option<(PathBuf, PathBuf)> {
        for root in &self.all_datadir_roots {
            let candidate = root.join("adapters").join(component).join(framework);
            if candidate.is_dir() {
                return Some((candidate, root.clone()));
            }
        }
        None
    }

    /// Every `(component, framework, resource_root)` discoverable under the
    /// datadir roots, deduped on `(component, framework)` and sorted.
    fn discover_all(&self) -> Vec<(String, String, PathBuf)> {
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        let mut out: Vec<(String, String, PathBuf)> = Vec::new();
        for root in &self.all_datadir_roots {
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
    /// Roots that `copy_tree` / `remove_tree` destinations must fall
    /// under. Populated from the driver's `allowed_external_roots` plus
    /// the resource root.
    allowed_roots: Vec<PathBuf>,
}

impl ManagerOps {
    fn new(
        log: CentralLog,
        actor: String,
        install_mode: String,
        component: String,
        label: String,
        allowed_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            log,
            actor,
            install_mode,
            component,
            label,
            allowed_roots,
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

    fn copy_tree(&self, src: &Path, dst: &Path) -> Result<(), AdapterError> {
        validate_ops_path(src, &self.allowed_roots)?;
        validate_ops_path(dst, &self.allowed_roots)?;
        reject_symlink(src)?;
        if !src.is_dir() {
            return Err(AdapterError::Io {
                path: src.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "source directory does not exist",
                ),
            });
        }
        std::fs::create_dir_all(dst).map_err(|source| AdapterError::Io {
            path: dst.to_path_buf(),
            source,
        })?;
        copy_dir_recursive(src, dst).map_err(|source| AdapterError::Io {
            path: dst.to_path_buf(),
            source,
        })
    }

    fn copy_file(&self, src: &Path, dst: &Path) -> Result<(), AdapterError> {
        validate_ops_path(src, &self.allowed_roots)?;
        validate_ops_path(dst, &self.allowed_roots)?;
        reject_symlink(src)?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|source| AdapterError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::copy(src, dst).map_err(|source| AdapterError::Io {
            path: dst.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    fn remove_tree(&self, path: &Path) -> Result<bool, AdapterError> {
        validate_ops_path(path, &self.allowed_roots)?;
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(path).map_err(|source| AdapterError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(true)
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
    prepend_path_with_existing(prepend, std::env::var_os("PATH"))
}

fn prepend_path_with_existing(
    prepend: &[PathBuf],
    existing: Option<std::ffi::OsString>,
) -> std::ffi::OsString {
    let mut parts: Vec<PathBuf> = prepend.to_vec();
    if let Some(existing) = existing {
        parts.extend(std::env::split_paths(&existing));
    }
    // join_paths only fails if a component contains the path separator,
    // which our dirs do not; fall back to the prepend dirs alone.
    std::env::join_paths(&parts)
        .unwrap_or_else(|_| std::env::join_paths(prepend).unwrap_or_default())
}

/// Validate that `path` is under one of `allowed_roots` and contains no
/// traversal segments. Used by `copy_tree` / `remove_tree` to enforce the
/// Manager's IO boundary before any filesystem mutation.
fn validate_ops_path(path: &Path, allowed_roots: &[PathBuf]) -> Result<(), AdapterError> {
    use super::claim::validate_external_path;

    validate_external_path(path, allowed_roots).map_err(|source| {
        AdapterError::ClaimValidation(super::claim::ClaimValidationError::ExternalPath {
            id: format!("ops:{}", path.display()),
            source,
        })
    })
}

/// Reject a path that is a symlink. Used by `copy_file` and
/// `copy_dir_recursive` to prevent following a symlink that escapes the
/// allowed roots.
fn reject_symlink(path: &Path) -> Result<(), AdapterError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(AdapterError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "symlink rejected in adapter resource tree: {}",
                    path.display()
                ),
            ),
        }),
        _ => Ok(()),
    }
}

/// Recursively copy regular files and subdirectories from `src` into
/// `dst`. Symlinks are rejected — a symlink inside the resource tree
/// could point outside the allowed roots, bypassing the boundary check
/// on the top-level `src` path.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "symlink rejected in adapter resource tree: {}",
                    entry.path().display()
                ),
            ));
        }
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ft.is_dir() {
            std::fs::create_dir_all(&dst_path)?;
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
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

/// Map a [`super::contract::ContractError`] to the existing [`AdapterError`]
/// family for backward-compatible CLI rendering.
fn map_contract_error(component: &str, err: super::contract::ContractError) -> AdapterError {
    match err {
        super::contract::ContractError::Unavailable { searched, .. } => {
            AdapterError::AdapterManifest {
                component: component.to_string(),
                path: searched.into_iter().next().unwrap_or_default(),
                reason: "component contract not found in the matched component scope".to_string(),
            }
        }
        super::contract::ContractError::ParseError { path, reason } => {
            AdapterError::AdapterManifest {
                component: component.to_string(),
                path,
                reason,
            }
        }
        super::contract::ContractError::Io { path, source } => AdapterError::AdapterManifest {
            component: component.to_string(),
            path,
            reason: source.to_string(),
        },
    }
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

/// Extract the `dest` from the first `[[adapters]]` entry whose
/// `framework` matches. Returns `None` when the field is absent or empty.
fn declared_dest(manifest: &ComponentManifest, framework: &str) -> Option<String> {
    manifest
        .adapters
        .iter()
        .find(|adapter| adapter.framework.as_deref().map(str::trim) == Some(framework))
        .and_then(|adapter| adapter.dest.as_deref())
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
}

/// Extract the `adapter_type` from the first `[[adapters]]` entry whose
/// `framework` matches. Returns `None` when the manifest omits the field
/// (which the caller treats as defaulting to `"plugin"`).
fn declared_adapter_type(manifest: &ComponentManifest, framework: &str) -> Option<String> {
    manifest
        .adapters
        .iter()
        .find(|adapter| adapter.framework.as_deref().map(str::trim) == Some(framework))
        .and_then(|adapter| adapter.adapter_type.as_deref())
        .map(str::trim)
        .filter(|at| !at.is_empty())
        .map(str::to_string)
}

/// Whether a component status makes it visible to adapter scan/enable.
/// Both fully-installed and adopted components should be adapter-visible.
fn is_adapter_visible_status(status: ObjectStatus) -> bool {
    matches!(status, ObjectStatus::Installed | ObjectStatus::Adopted)
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

/// Extract declared skills for a framework, checking the framework-specific
/// section first (e.g. `adapters.openclaw.skills`) then falling back to
/// the generic `adapters.skills`.
fn declared_skills(
    manifest: &ComponentManifest,
    framework: &str,
) -> Vec<crate::manifest::AdapterSkillSpec> {
    let adapter = manifest
        .adapters
        .iter()
        .find(|a| a.framework.as_deref().map(str::trim) == Some(framework));
    let adapter = match adapter {
        Some(a) => a,
        None => return Vec::new(),
    };
    // Framework-specific section takes precedence.
    match framework {
        "openclaw" => {
            if let Some(ref oc) = adapter.openclaw {
                if !oc.skills.is_empty() {
                    return oc.skills.clone();
                }
            }
        }
        "hermes" => {
            if let Some(ref h) = adapter.hermes {
                if !h.skills.is_empty() {
                    return h.skills.clone();
                }
            }
        }
        _ => {}
    }
    adapter.skills.clone()
}

/// Resolve skill source paths from manifest specs.
///
/// `effective_datadir` is the datadir root that was used to resolve the
/// adapter resource root — `{datadir}` in skill source templates
/// expands to this value so skill sources stay in the same scope as the
/// adapter itself (important for user-mode enabling a system-adopted
/// component).
///
/// Resolved paths are validated against an IO boundary: they must fall
/// under `resource_root` or `effective_datadir`. A manifest cannot
/// self-authorise access to arbitrary filesystem paths.
///
/// For each declared skill:
/// - If `source` is present, expand layout placeholders (with
///   `{component}` as extra var and `{datadir}` set to
///   `effective_datadir`). A relative result is resolved against
///   `resource_root`.
/// - If `source` is absent, the driver will fall back to
///   `<resource_root>/skills/<name>/`.
fn resolve_skill_sources(
    specs: Vec<crate::manifest::AdapterSkillSpec>,
    layout: &FsLayout,
    effective_datadir: &Path,
    component: &str,
    framework: &str,
    resource_root: &Path,
) -> Result<Vec<super::driver::DeclaredSkill>, AdapterError> {
    let mut scoped_layout = layout.clone();
    scoped_layout.datadir = effective_datadir.to_path_buf();
    let allowed_roots = [resource_root.to_path_buf(), effective_datadir.to_path_buf()];
    specs
        .into_iter()
        .map(|spec| {
            let source = match spec.source {
                Some(ref template) => {
                    let expanded = super::expand_layout_placeholders(
                        template,
                        &scoped_layout,
                        &[("component", component)],
                    )?;
                    let resolved = if expanded.is_relative() {
                        resource_root.join(&expanded)
                    } else {
                        expanded
                    };
                    super::claim::validate_external_path(&resolved, &allowed_roots).map_err(
                        |_| AdapterError::InvalidAdapterInput {
                            component: component.to_string(),
                            framework: framework.to_string(),
                            reason: format!(
                                "skill '{}' source '{}' resolves to '{}' which is outside the allowed roots (resource_root or datadir)",
                                spec.name,
                                template,
                                resolved.display(),
                            ),
                        },
                    )?;
                    Some(resolved)
                }
                None => None,
            };
            Ok(super::driver::DeclaredSkill {
                name: spec.name,
                source,
            })
        })
        .collect()
}

/// Extract declared config entries for a framework, checking the
/// framework-specific section first then falling back to the generic one.
fn declared_config(
    manifest: &ComponentManifest,
    framework: &str,
) -> Vec<crate::manifest::AdapterConfigSetSpec> {
    let adapter = manifest
        .adapters
        .iter()
        .find(|a| a.framework.as_deref().map(str::trim) == Some(framework));
    let adapter = match adapter {
        Some(a) => a,
        None => return Vec::new(),
    };
    // Framework-specific section takes precedence.
    if framework == "openclaw" {
        if let Some(ref oc) = adapter.openclaw {
            if !oc.config.is_empty() {
                return oc.config.clone();
            }
        }
    }
    adapter.config.clone()
}

/// Extract the bundle entry-point from the manifest, checking the
/// framework-specific section first then falling back to the generic
/// `[adapters.bundle].entry`.
fn declared_bundle_entry(manifest: &ComponentManifest, framework: &str) -> Option<String> {
    let adapter = manifest
        .adapters
        .iter()
        .find(|a| a.framework.as_deref().map(str::trim) == Some(framework))?;
    match framework {
        "openclaw" => {
            if let Some(ref oc) = adapter.openclaw {
                if let Some(ref entry) = oc.bundle.entry {
                    return Some(entry.clone());
                }
            }
        }
        "hermes" => {
            if let Some(ref h) = adapter.hermes {
                if let Some(ref entry) = h.bundle.entry {
                    return Some(entry.clone());
                }
            }
        }
        _ => {}
    }
    adapter.bundle.entry.clone()
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
        let joined = prepend_path_with_existing(
            &[PathBuf::from("/opt/a"), PathBuf::from("/opt/b")],
            Some(std::ffi::OsString::from("/usr/bin:/bin")),
        );
        let dirs: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(dirs[0], PathBuf::from("/opt/a"));
        assert_eq!(dirs[1], PathBuf::from("/opt/b"));
        assert!(dirs.contains(&PathBuf::from("/usr/bin")));
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

    // -- declared_adapter_type ------------------------------------------------

    fn manifest_with_adapter_type(adapter_type: Option<&str>) -> ComponentManifest {
        use crate::manifest::*;
        ComponentManifest {
            schema_version: CURRENT_SCHEMA_VERSION,
            component: ComponentMeta {
                name: "test-comp".to_string(),
                version: "0.1.0".to_string(),
                layer: "runtime".to_string(),
                domain: None,
                display_name: None,
                owner: None,
                license: None,
                repository: None,
            },
            contract: ContractSpec::default(),
            artifact: ArtifactSpec::default(),
            source: SourceSpec::default(),
            distribution_selectors: Vec::new(),
            build: BuildSpec::default(),
            install: InstallSpec::default(),
            backends: ManifestBackends::default(),
            env_requirements: EnvRequirements::default(),
            dependencies: DependenciesSpec::default(),
            features: Vec::new(),
            adapters: vec![AdapterSpec {
                framework: Some("openclaw".to_string()),
                adapter_type: adapter_type.map(str::to_string),
                ..Default::default()
            }],
            health_check: None,
            health_checks: Vec::new(),
        }
    }

    #[test]
    fn declared_adapter_type_returns_plugin() {
        let manifest = manifest_with_adapter_type(Some("plugin"));
        assert_eq!(
            declared_adapter_type(&manifest, "openclaw"),
            Some("plugin".to_string())
        );
    }

    #[test]
    fn declared_adapter_type_returns_none_when_absent() {
        let manifest = manifest_with_adapter_type(None);
        assert_eq!(declared_adapter_type(&manifest, "openclaw"), None);
    }

    #[test]
    fn declared_adapter_type_returns_skill_bundle() {
        let manifest = manifest_with_adapter_type(Some("skill_bundle"));
        assert_eq!(
            declared_adapter_type(&manifest, "openclaw"),
            Some("skill_bundle".to_string())
        );
    }

    #[test]
    fn declared_adapter_type_returns_none_for_wrong_framework() {
        let manifest = manifest_with_adapter_type(Some("plugin"));
        assert_eq!(declared_adapter_type(&manifest, "hermes"), None);
    }

    #[test]
    fn unsupported_adapter_type_error_contains_details() {
        let err = AdapterError::UnsupportedAdapterType {
            component: "tokenless".to_string(),
            framework: "openclaw".to_string(),
            adapter_type: "skill_bundle".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("skill_bundle"));
        assert!(msg.contains("tokenless"));
        assert!(msg.contains("openclaw"));
        assert!(msg.contains("only 'plugin' is implemented"));
    }

    #[test]
    fn unsupported_adapter_type_extension() {
        let err = AdapterError::UnsupportedAdapterType {
            component: "agentsight".to_string(),
            framework: "openclaw".to_string(),
            adapter_type: "extension".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("extension"));
        assert!(msg.contains("agentsight"));
    }

    #[test]
    fn unsupported_adapter_type_unknown_value() {
        let err = AdapterError::UnsupportedAdapterType {
            component: "agentsight".to_string(),
            framework: "openclaw".to_string(),
            adapter_type: "magic".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("magic"));
        assert!(msg.contains("only 'plugin' is implemented"));
    }

    #[test]
    fn plugin_adapter_type_passes_gate() {
        let manifest = manifest_with_adapter_type(Some("plugin"));
        let at = declared_adapter_type(&manifest, "openclaw");
        let should_reject = at.as_ref().is_some_and(|t| t != "plugin");
        assert!(!should_reject, "plugin must pass the gate");
    }

    #[test]
    fn absent_adapter_type_passes_gate() {
        let manifest = manifest_with_adapter_type(None);
        let at = declared_adapter_type(&manifest, "openclaw");
        let should_reject = at.as_ref().is_some_and(|t| t != "plugin");
        assert!(!should_reject, "absent adapter_type must pass the gate");
    }

    #[test]
    fn skill_bundle_adapter_type_fails_gate() {
        let manifest = manifest_with_adapter_type(Some("skill_bundle"));
        let at = declared_adapter_type(&manifest, "openclaw");
        let should_reject = at.as_ref().is_some_and(|t| t != "plugin");
        assert!(should_reject, "skill_bundle must be rejected");
    }

    // -- scan with Adopted + datadir-only contract ----------------------------

    /// Regression: an RPM-adopted component with no state snapshot but a
    /// datadir contract must still appear as `declared=true` in scan, and
    /// its `adapter_type` must be surfaced.
    #[test]
    fn scan_adopted_component_with_datadir_only_contract() {
        use crate::state::{InstalledObject, ObjectKind, ObjectStatus, Ownership};

        let tmp = tempfile::tempdir().expect("tempdir");
        let state_dir = tmp.path().join("state");
        let datadir = tmp.path().join("data");

        // Write installed.toml with an Adopted component, no snapshot.
        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "sec-core".to_string(),
            version: "0.1.0".to_string(),
            status: ObjectStatus::Adopted,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(Ownership::RpmObserved),
            rpm_metadata: None,
            installed_at: "2026-06-18T00:00:00Z".to_string(),
            last_operation_id: None,
            managed: false,
            adopted: true,
            subscription_scope: crate::state::SubscriptionScope::None,
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        std::fs::create_dir_all(&state_dir).expect("mkdir state");
        state
            .save(&state_dir.join("installed.toml"))
            .expect("save state");

        // Write a datadir contract (no state snapshot).
        let contract = r#"
[component]
name = "sec-core"
version = "0.1.0"
layer = "runtime"

[[adapters]]
framework = "openclaw"
adapter_type = "plugin"
plugin_id = "sec-core"
source = "adapters/openclaw"
dest = "{datadir}/adapters/{component}/openclaw/"
"#;
        let contract_dir = datadir.join("components").join("sec-core");
        std::fs::create_dir_all(&contract_dir).expect("mkdir contract");
        std::fs::write(contract_dir.join("component.toml"), contract).expect("write contract");

        // Build a manager pointing at our temp dirs.
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = state_dir.join("installed.toml");
        manager.visible_roots = vec![VisibleRoot {
            state_dir: state_dir.clone(),
            contract_datadir_roots: vec![datadir.clone()],
        }];
        manager.all_datadir_roots = vec![datadir.clone()];

        let report = manager.scan().expect("scan");

        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "sec-core" && e.framework == "openclaw")
            .expect("sec-core/openclaw should be in scan results");
        assert!(
            entry.declared,
            "adopted component with datadir contract must be declared"
        );
        assert_eq!(
            entry.adapter_type.as_deref(),
            Some("plugin"),
            "adapter_type must be surfaced from the contract"
        );
    }

    // -- user/system scope isolation ------------------------------------------

    fn valid_contract_toml(name: &str) -> String {
        format!(
            r#"
[component]
name = "{name}"
version = "0.1.0"
layer = "runtime"

[[adapters]]
framework = "openclaw"
adapter_type = "plugin"
plugin_id = "{name}"
source = "adapters/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"
"#
        )
    }

    fn seed_installed_state(state_dir: &std::path::Path, component: &str, status: ObjectStatus) {
        use crate::state::{InstalledObject, ObjectKind, Ownership, SubscriptionScope};

        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: "0.1.0".to_string(),
            status,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some(if status == ObjectStatus::Adopted {
                "rpm".to_string()
            } else {
                "raw".to_string()
            }),
            ownership: Some(if status == ObjectStatus::Adopted {
                Ownership::RpmObserved
            } else {
                Ownership::RawManaged
            }),
            rpm_metadata: None,
            installed_at: "2026-06-18T00:00:00Z".to_string(),
            last_operation_id: None,
            managed: status != ObjectStatus::Adopted,
            adopted: status == ObjectStatus::Adopted,
            subscription_scope: SubscriptionScope::None,
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        std::fs::create_dir_all(state_dir).expect("mkdir state");
        state
            .save(&state_dir.join("installed.toml"))
            .expect("save state");
    }

    fn write_contract(datadir: &std::path::Path, component: &str) {
        let dir = datadir.join("components").join(component);
        std::fs::create_dir_all(&dir).expect("mkdir contract");
        std::fs::write(dir.join("component.toml"), valid_contract_toml(component))
            .expect("write contract");
    }

    /// Contract TOML with a custom (non-convention) dest path.
    fn contract_toml_with_custom_dest(name: &str, dest: &str) -> String {
        format!(
            r#"
[component]
name = "{name}"
version = "0.1.0"
layer = "runtime"

[[adapters]]
framework = "openclaw"
adapter_type = "plugin"
plugin_id = "{name}"
source = "adapters/openclaw"
dest = "{dest}"
"#
        )
    }

    /// Contract TOML without a `dest` field on the adapter entry.
    fn contract_toml_without_dest(name: &str) -> String {
        format!(
            r#"
[component]
name = "{name}"
version = "0.1.0"
layer = "runtime"

[[adapters]]
framework = "openclaw"
adapter_type = "plugin"
plugin_id = "{name}"
source = "adapters/openclaw"
"#
        )
    }

    fn write_contract_with_content(datadir: &std::path::Path, component: &str, content: &str) {
        let dir = datadir.join("components").join(component);
        std::fs::create_dir_all(&dir).expect("mkdir contract");
        std::fs::write(dir.join("component.toml"), content).expect("write contract");
    }

    // -- contract-driven resource root discovery --------------------------------

    /// Convention path still works when manifest has no dest or no manifest.
    #[test]
    fn convention_path_works_without_dest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state_dir = tmp.path().join("state");
        let datadir = tmp.path().join("data");

        seed_installed_state(&state_dir, "tokenless", ObjectStatus::Installed);

        // Contract without dest.
        write_contract_with_content(
            &datadir,
            "tokenless",
            &contract_toml_without_dest("tokenless"),
        );

        // Convention resource directory.
        let convention = datadir.join("adapters").join("tokenless").join("openclaw");
        std::fs::create_dir_all(&convention).expect("mkdir convention");
        std::fs::write(convention.join("plugin.json"), b"{}").expect("write");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = state_dir.join("installed.toml");
        manager.visible_roots = vec![VisibleRoot {
            state_dir: state_dir.clone(),
            contract_datadir_roots: vec![datadir.clone()],
        }];
        manager.all_datadir_roots = vec![datadir.clone()];

        // scan: resource should be found at convention path.
        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "tokenless" && e.framework == "openclaw")
            .expect("tokenless/openclaw should be in scan");
        assert!(entry.declared, "must be declared");
        assert!(
            entry.resource_root.is_some(),
            "convention resource must be found"
        );
        assert_eq!(
            entry.resource_root.as_ref().unwrap(),
            &convention,
            "resource root must be the convention path"
        );
    }

    /// Convention path still works when there is no manifest at all, only
    /// resource directories (pure directory discovery).
    #[test]
    fn convention_path_works_without_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state_dir = tmp.path().join("state");
        let datadir = tmp.path().join("data");

        // No installed state, no contract — just a resource directory.
        std::fs::create_dir_all(&state_dir).expect("mkdir state");
        InstalledState::default()
            .save(&state_dir.join("installed.toml"))
            .expect("save empty state");

        let convention = datadir.join("adapters").join("tokenless").join("openclaw");
        std::fs::create_dir_all(&convention).expect("mkdir convention");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = state_dir.join("installed.toml");
        manager.visible_roots = vec![VisibleRoot {
            state_dir: state_dir.clone(),
            contract_datadir_roots: vec![datadir.clone()],
        }];
        manager.all_datadir_roots = vec![datadir.clone()];

        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "tokenless" && e.framework == "openclaw")
            .expect("tokenless/openclaw should be found by directory discovery");
        assert!(!entry.declared, "no manifest — must not be declared");
        assert!(
            entry.resource_root.is_some(),
            "convention resource must be found by directory discovery"
        );
    }

    /// Custom dest from contract is used for resource root when directory exists.
    #[test]
    fn declared_custom_dest_is_used() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state_dir = tmp.path().join("state");
        let datadir = tmp.path().join("data");

        seed_installed_state(&state_dir, "sec-core", ObjectStatus::Adopted);

        // Contract with custom dest.
        write_contract_with_content(
            &datadir,
            "sec-core",
            &contract_toml_with_custom_dest("sec-core", "{datadir}/custom/sec-core/openclaw/"),
        );

        // Resource at the custom location (not the convention path).
        let custom_root = datadir.join("custom").join("sec-core").join("openclaw");
        std::fs::create_dir_all(&custom_root).expect("mkdir custom");
        std::fs::write(custom_root.join("plugin.json"), b"{}").expect("write");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = state_dir.join("installed.toml");
        manager.visible_roots = vec![VisibleRoot {
            state_dir: state_dir.clone(),
            contract_datadir_roots: vec![datadir.clone()],
        }];
        manager.all_datadir_roots = vec![datadir.clone()];

        // scan: resource_root must use the custom dest path.
        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "sec-core" && e.framework == "openclaw")
            .expect("sec-core/openclaw should be in scan");
        assert!(entry.declared);
        assert_eq!(
            entry.resource_root.as_ref(),
            Some(&custom_root),
            "scan must use the contract-declared dest, not convention"
        );

        // resolve_resource_root (used by enable) must return the custom path.
        let state = InstalledState::load(&state_dir.join("installed.toml")).expect("load state");
        let (manifest, scoped_roots) = manager
            .load_visible_component_manifest("sec-core", &state)
            .expect("load manifest");
        let (resolved, _effective_datadir) = manager
            .resolve_resource_root("sec-core", "openclaw", &manifest, &scoped_roots)
            .expect("resolve");
        assert_eq!(
            resolved, custom_root,
            "enable resource root must be the contract dest"
        );
    }

    /// Declared dest with missing directory: scan shows absent, enable returns error.
    #[test]
    fn declared_dest_missing_directory_reports_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state_dir = tmp.path().join("state");
        let datadir = tmp.path().join("data");

        seed_installed_state(&state_dir, "sec-core", ObjectStatus::Adopted);

        // Contract with custom dest, but DO NOT create the directory.
        write_contract_with_content(
            &datadir,
            "sec-core",
            &contract_toml_with_custom_dest("sec-core", "{datadir}/custom/sec-core/openclaw/"),
        );

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = state_dir.join("installed.toml");
        manager.visible_roots = vec![VisibleRoot {
            state_dir: state_dir.clone(),
            contract_datadir_roots: vec![datadir.clone()],
        }];
        manager.all_datadir_roots = vec![datadir.clone()];

        // scan: declared yes, resource absent.
        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "sec-core" && e.framework == "openclaw")
            .expect("sec-core/openclaw should be in scan");
        assert!(entry.declared, "must be declared from contract");
        assert!(
            entry.resource_root.is_none(),
            "resource_root must be None when dest directory does not exist"
        );

        // resolve_resource_root: must return ContractResourceRootNotFound,
        // NOT silently fall back to convention.
        let state = InstalledState::load(&state_dir.join("installed.toml")).expect("load state");
        let (manifest, scoped_roots) = manager
            .load_visible_component_manifest("sec-core", &state)
            .expect("load manifest");
        let err = manager
            .resolve_resource_root("sec-core", "openclaw", &manifest, &scoped_roots)
            .expect_err("must fail when contract dest directory is absent");
        assert!(
            matches!(err, AdapterError::ContractResourceRootNotFound { .. }),
            "expected ContractResourceRootNotFound, got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("sec-core") && msg.contains("openclaw"),
            "error must mention component and framework: {msg}"
        );
    }

    /// Declared dest with missing directory must NOT fall back to convention
    /// even when convention directory exists.
    #[test]
    fn declared_dest_missing_does_not_fallback_to_convention() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state_dir = tmp.path().join("state");
        let datadir = tmp.path().join("data");

        seed_installed_state(&state_dir, "sec-core", ObjectStatus::Adopted);

        // Contract with custom dest — directory does NOT exist.
        write_contract_with_content(
            &datadir,
            "sec-core",
            &contract_toml_with_custom_dest("sec-core", "{datadir}/custom/sec-core/openclaw/"),
        );

        // Convention path DOES exist (should be ignored because contract is
        // authoritative).
        let convention = datadir.join("adapters").join("sec-core").join("openclaw");
        std::fs::create_dir_all(&convention).expect("mkdir convention");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = state_dir.join("installed.toml");
        manager.visible_roots = vec![VisibleRoot {
            state_dir: state_dir.clone(),
            contract_datadir_roots: vec![datadir.clone()],
        }];
        manager.all_datadir_roots = vec![datadir.clone()];

        // scan: declared yes, resource absent (convention exists but ignored).
        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "sec-core" && e.framework == "openclaw")
            .expect("sec-core/openclaw should be in scan");
        assert!(entry.declared);
        assert!(
            entry.resource_root.is_none(),
            "resource_root must be None — convention path must not be used when contract dest is absent"
        );

        // resolve_resource_root must error, not fall back.
        let state = InstalledState::load(&state_dir.join("installed.toml")).expect("load state");
        let (manifest, scoped_roots) = manager
            .load_visible_component_manifest("sec-core", &state)
            .expect("load manifest");
        let err = manager
            .resolve_resource_root("sec-core", "openclaw", &manifest, &scoped_roots)
            .expect_err("must not fall back to convention");
        assert!(
            matches!(err, AdapterError::ContractResourceRootNotFound { .. }),
            "expected ContractResourceRootNotFound, got: {err}"
        );
    }

    /// User-mode manager can discover contract-defined resource root from
    /// a system-installed/adopted component.
    #[test]
    fn user_mode_uses_system_contract_dest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_state = tmp.path().join("user_state");
        let user_data = tmp.path().join("user_data");
        let sys_state = tmp.path().join("sys_state");
        let sys_data = tmp.path().join("sys_data");

        // System has sec-core adopted with a custom dest.
        seed_installed_state(&sys_state, "sec-core", ObjectStatus::Adopted);
        write_contract_with_content(
            &sys_data,
            "sec-core",
            &contract_toml_with_custom_dest("sec-core", "{datadir}/custom/sec-core/openclaw/"),
        );
        // Resource in system datadir at the custom location.
        let custom_root = sys_data.join("custom").join("sec-core").join("openclaw");
        std::fs::create_dir_all(&custom_root).expect("mkdir custom");
        std::fs::write(custom_root.join("plugin.json"), b"{}").expect("write");

        // User state is empty.
        std::fs::create_dir_all(&user_state).expect("mkdir user_state");
        InstalledState::default()
            .save(&user_state.join("installed.toml"))
            .expect("save empty user state");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = user_state.join("installed.toml");
        manager.visible_roots = vec![
            VisibleRoot {
                state_dir: user_state,
                contract_datadir_roots: vec![user_data],
            },
            VisibleRoot {
                state_dir: sys_state,
                contract_datadir_roots: vec![sys_data.clone()],
            },
        ];
        manager.all_datadir_roots = vec![sys_data];

        // scan: user-mode must discover sec-core from the system root,
        // with resource_root pointing to the contract-declared custom path.
        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "sec-core" && e.framework == "openclaw")
            .expect("sec-core/openclaw should be in scan via system root");
        assert!(entry.declared);
        assert_eq!(
            entry.resource_root.as_ref(),
            Some(&custom_root),
            "user-mode scan must find contract-declared resource root from system scope"
        );
    }

    /// User-mode `resolve_skill_sources` expands `{datadir}` to the
    /// system datadir (the scope where the component contract lives),
    /// not to the user-mode layout's datadir.
    #[test]
    fn user_mode_skill_source_uses_system_datadir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_state = tmp.path().join("user_state");
        let user_data = tmp.path().join("user_data");
        let sys_state = tmp.path().join("sys_state");
        let sys_data = tmp.path().join("sys_data");

        seed_installed_state(&sys_state, "sec-core", ObjectStatus::Adopted);

        let contract = r#"
[component]
name = "sec-core"
version = "0.1.0"
layer = "runtime"

[[adapters]]
framework = "openclaw"
adapter_type = "plugin"
plugin_id = "sec-core"
dest = "{datadir}/custom/sec-core/openclaw/"

[[adapters.openclaw.skills]]
name = "code-scanner"
source = "{datadir}/skills/code-scanner/"
"#;
        write_contract_with_content(&sys_data, "sec-core", contract);

        // Resource root and skill source in system datadir.
        let custom_root = sys_data.join("custom").join("sec-core").join("openclaw");
        std::fs::create_dir_all(&custom_root).expect("mkdir custom");
        std::fs::write(custom_root.join("plugin.json"), b"{}").expect("write");
        let skill_source = sys_data.join("skills").join("code-scanner");
        std::fs::create_dir_all(&skill_source).expect("mkdir skill source");
        std::fs::write(skill_source.join("manifest.json"), b"{}").expect("write");

        // User state empty.
        std::fs::create_dir_all(&user_state).expect("mkdir user_state");
        InstalledState::default()
            .save(&user_state.join("installed.toml"))
            .expect("save empty user state");

        let user_layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(user_layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = user_state.join("installed.toml");
        manager.visible_roots = vec![
            VisibleRoot {
                state_dir: user_state,
                contract_datadir_roots: vec![user_data],
            },
            VisibleRoot {
                state_dir: sys_state,
                contract_datadir_roots: vec![sys_data.clone()],
            },
        ];
        manager.all_datadir_roots = vec![sys_data.clone()];

        // Resolve resource root — must come from system datadir.
        let state = InstalledState::load(&manager.state_path).expect("load state");
        let (manifest, scoped_roots) = manager
            .load_visible_component_manifest("sec-core", &state)
            .expect("load manifest");
        let (resource_root, effective_datadir) = manager
            .resolve_resource_root("sec-core", "openclaw", &manifest, &scoped_roots)
            .expect("resolve resource root");
        assert_eq!(resource_root, custom_root);
        assert_eq!(
            effective_datadir, sys_data,
            "effective datadir must be the system datadir"
        );

        // Resolve skill sources — {datadir} must expand to sys_data.
        let skill_specs = declared_skills(&manifest, "openclaw");
        let skills = resolve_skill_sources(
            skill_specs,
            &manager.layout,
            &effective_datadir,
            "sec-core",
            "openclaw",
            &resource_root,
        )
        .expect("resolve skills");
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "code-scanner");
        assert_eq!(
            skills[0].source.as_ref(),
            Some(&skill_source),
            "skill source must resolve to system datadir path, not user datadir"
        );
    }

    /// User component must NOT fall back to system datadir contract.
    #[test]
    fn user_component_does_not_fallback_to_system_contract() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_state = tmp.path().join("user_state");
        let user_data = tmp.path().join("user_data");
        let sys_state = tmp.path().join("sys_state");
        let sys_data = tmp.path().join("sys_data");

        // User state has tokenless installed, no contract anywhere in user scope.
        seed_installed_state(&user_state, "tokenless", ObjectStatus::Installed);
        // System datadir has a valid contract.
        write_contract(&sys_data, "tokenless");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = user_state.join("installed.toml");
        manager.visible_roots = vec![
            VisibleRoot {
                state_dir: user_state.clone(),
                contract_datadir_roots: vec![user_data.clone()],
            },
            VisibleRoot {
                state_dir: sys_state.clone(),
                contract_datadir_roots: vec![sys_data.clone()],
            },
        ];
        manager.all_datadir_roots = vec![user_data, sys_data];

        // Scan: tokenless must NOT be declared (no user contract).
        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "tokenless" && e.framework == "openclaw");
        assert!(
            entry.is_none() || !entry.unwrap().declared,
            "user component must not use system contract"
        );
        assert!(
            report.warnings.iter().any(|w| w.contains("tokenless")
                && w.contains("no component contract")
                && w.contains("another scope")),
            "scan must warn that user contract is missing and system exists, got: {:?}",
            report.warnings
        );
    }

    /// System component can use system/packaged datadir contract.
    #[test]
    fn system_component_uses_system_datadir_contract() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sys_state = tmp.path().join("sys_state");
        let sys_data = tmp.path().join("sys_data");
        let pkg_data = tmp.path().join("pkg_data");

        seed_installed_state(&sys_state, "tokenless", ObjectStatus::Installed);
        // Contract in pkg_data (simulates /usr/share vs /usr/local/share).
        write_contract(&pkg_data, "tokenless");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = sys_state.join("installed.toml");
        manager.visible_roots = vec![VisibleRoot {
            state_dir: sys_state.clone(),
            contract_datadir_roots: vec![sys_data, pkg_data.clone()],
        }];
        manager.all_datadir_roots = vec![pkg_data];

        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "tokenless" && e.framework == "openclaw")
            .expect("tokenless/openclaw should be declared");
        assert!(
            entry.declared,
            "system component must find contract in packaged datadir"
        );
    }

    /// User-mode scan includes system-installed component via system
    /// visible root (contract resolved from system datadir).
    #[test]
    fn user_scan_includes_system_component_via_system_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_state = tmp.path().join("user_state");
        let user_data = tmp.path().join("user_data");
        let sys_state = tmp.path().join("sys_state");
        let sys_data = tmp.path().join("sys_data");

        // Only system state has tokenless; contract in system datadir.
        seed_installed_state(&sys_state, "tokenless", ObjectStatus::Installed);
        write_contract(&sys_data, "tokenless");

        // User state is empty.
        std::fs::create_dir_all(&user_state).expect("mkdir user_state");
        InstalledState::default()
            .save(&user_state.join("installed.toml"))
            .expect("save empty user state");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let mut manager =
            AdapterManager::new(layout, Some(tmp.path().to_path_buf()), "test".into());
        manager.state_path = user_state.join("installed.toml");
        manager.visible_roots = vec![
            VisibleRoot {
                state_dir: user_state,
                contract_datadir_roots: vec![user_data],
            },
            VisibleRoot {
                state_dir: sys_state,
                contract_datadir_roots: vec![sys_data],
            },
        ];

        // scan must find tokenless via the system root.
        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "tokenless" && e.framework == "openclaw")
            .expect("tokenless/openclaw should be in scan");
        assert!(
            entry.declared,
            "system component must be declared via system root"
        );
    }

    // -- copy_tree / remove_tree boundary ------------------------------------

    #[test]
    fn copy_tree_rejects_source_outside_allowed_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let allowed = tmp.path().join("allowed");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(allowed.join("dst")).expect("mkdir");
        std::fs::create_dir_all(&outside).expect("mkdir");
        std::fs::write(outside.join("x.txt"), b"data").expect("write");

        let ops = ManagerOps::new(
            CentralLog::open(tmp.path().join("log.jsonl")),
            "test".into(),
            "user".into(),
            "comp".into(),
            "test".into(),
            vec![allowed.clone()],
        );
        let err = ops
            .copy_tree(&outside, &allowed.join("dst/target"))
            .expect_err("source outside allowed roots must fail");
        assert!(
            matches!(err, AdapterError::ClaimValidation(_)),
            "expected ClaimValidation, got {err:?}"
        );
    }

    #[test]
    fn copy_tree_accepts_source_inside_allowed_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let allowed = tmp.path().join("allowed");
        let src = allowed.join("src");
        let dst = allowed.join("dst");
        std::fs::create_dir_all(&src).expect("mkdir src");
        std::fs::write(src.join("f.txt"), b"ok").expect("write");

        let ops = ManagerOps::new(
            CentralLog::open(tmp.path().join("log.jsonl")),
            "test".into(),
            "user".into(),
            "comp".into(),
            "test".into(),
            vec![allowed],
        );
        ops.copy_tree(&src, &dst)
            .expect("source inside root must succeed");
        assert!(dst.join("f.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_rejects_symlink_inside_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let allowed = tmp.path().join("allowed");
        let src = allowed.join("src");
        let dst = allowed.join("dst");
        std::fs::create_dir_all(&src).expect("mkdir src");
        std::fs::write(src.join("ok.txt"), b"ok").expect("write");
        std::os::unix::fs::symlink("/etc/passwd", src.join("link")).expect("symlink");

        let ops = ManagerOps::new(
            CentralLog::open(tmp.path().join("log.jsonl")),
            "test".into(),
            "user".into(),
            "comp".into(),
            "test".into(),
            vec![allowed],
        );
        let err = ops
            .copy_tree(&src, &dst)
            .expect_err("symlink inside source must be rejected");
        assert!(
            matches!(err, AdapterError::Io { .. }),
            "expected Io error, got {err:?}"
        );
        assert!(
            err.to_string().contains("symlink rejected"),
            "error should mention symlink: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_rejects_symlink_source_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().canonicalize().expect("canonicalize");
        let allowed = base.join("allowed");
        let real_dir = allowed.join("real");
        std::fs::create_dir_all(&real_dir).expect("mkdir");
        std::fs::write(real_dir.join("f.txt"), b"data").expect("write");
        let link_dir = allowed.join("link_to_dir");
        std::os::unix::fs::symlink(&real_dir, &link_dir).expect("symlink");

        let ops = ManagerOps::new(
            CentralLog::open(base.join("log.jsonl")),
            "test".into(),
            "user".into(),
            "comp".into(),
            "test".into(),
            vec![allowed.clone()],
        );
        let err = ops
            .copy_tree(&link_dir, &allowed.join("dst"))
            .expect_err("symlink-to-dir source must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("symlink rejected"),
            "error should mention symlink: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_file_rejects_symlink_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().canonicalize().expect("canonicalize tmp");
        let allowed = base.join("allowed");
        std::fs::create_dir_all(&allowed).expect("mkdir");
        std::fs::write(allowed.join("real.txt"), b"ok").expect("write");
        std::os::unix::fs::symlink("/etc/passwd", allowed.join("link.txt")).expect("symlink");

        let ops = ManagerOps::new(
            CentralLog::open(base.join("log.jsonl")),
            "test".into(),
            "user".into(),
            "comp".into(),
            "test".into(),
            vec![allowed.clone()],
        );
        let err = ops
            .copy_file(&allowed.join("link.txt"), &allowed.join("dst.txt"))
            .expect_err("symlink source must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("symlink rejected") || msg.contains("boundary check"),
            "error should reject symlink via boundary or explicit check: {msg}"
        );

        ops.copy_file(&allowed.join("real.txt"), &allowed.join("dst.txt"))
            .expect("regular file must succeed");
        assert!(allowed.join("dst.txt").is_file());
    }

    // -- skill source allowed_roots integration ---------------------------------

    /// Skill source outside resource_root must be allowed by ManagerOps
    /// when added to allowed_roots. Verifies the P1 fix: copy_tree from
    /// `{datadir}/skills/<name>` succeeds when that path is in allowed_roots.
    #[test]
    fn copy_tree_accepts_skill_source_in_allowed_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let resource_root = tmp
            .path()
            .join("adapters")
            .join("sec-core")
            .join("openclaw");
        let skill_source = tmp.path().join("skills").join("code-scanner");
        let framework_home = tmp.path().join("home").join("skills").join("code-scanner");

        std::fs::create_dir_all(&resource_root).expect("mkdir resource_root");
        std::fs::create_dir_all(&skill_source).expect("mkdir skill_source");
        std::fs::write(skill_source.join("manifest.json"), b"{}").expect("write");
        std::fs::create_dir_all(tmp.path().join("home").join("skills")).expect("mkdir dst parent");

        let ops = ManagerOps::new(
            CentralLog::open(tmp.path().join("log.jsonl")),
            "test".into(),
            "user".into(),
            "comp".into(),
            "test".into(),
            vec![
                resource_root.clone(),
                skill_source.clone(),
                tmp.path().join("home"),
            ],
        );

        ops.copy_tree(&skill_source, &framework_home)
            .expect("skill source in allowed_roots must succeed");
        assert!(
            framework_home.join("manifest.json").is_file(),
            "skill files must be copied to framework home"
        );
    }

    /// Skill source outside resource_root and NOT in allowed_roots must
    /// be rejected.
    #[test]
    fn copy_tree_rejects_skill_source_not_in_allowed_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let resource_root = tmp
            .path()
            .join("adapters")
            .join("sec-core")
            .join("openclaw");
        let skill_source = tmp.path().join("skills").join("code-scanner");
        let framework_home = tmp.path().join("home").join("skills").join("code-scanner");

        std::fs::create_dir_all(&resource_root).expect("mkdir resource_root");
        std::fs::create_dir_all(&skill_source).expect("mkdir skill_source");
        std::fs::write(skill_source.join("manifest.json"), b"{}").expect("write");
        std::fs::create_dir_all(tmp.path().join("home").join("skills")).expect("mkdir dst parent");

        let ops = ManagerOps::new(
            CentralLog::open(tmp.path().join("log.jsonl")),
            "test".into(),
            "user".into(),
            "comp".into(),
            "test".into(),
            // Only resource_root and framework home — skill_source NOT included.
            vec![resource_root, tmp.path().join("home")],
        );

        let err = ops
            .copy_tree(&skill_source, &framework_home)
            .expect_err("skill source outside allowed_roots must be rejected");
        assert!(
            matches!(err, AdapterError::ClaimValidation(_)),
            "expected ClaimValidation, got {err:?}"
        );
    }

    // -- skill source boundary validation ---------------------------------------

    #[test]
    fn skill_source_under_datadir_is_accepted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let datadir = tmp.path().join("data");
        let resource_root = datadir.join("adapters").join("sec-core").join("openclaw");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let specs = vec![crate::manifest::AdapterSkillSpec {
            name: "code-scanner".to_string(),
            source: Some("{datadir}/skills/code-scanner/".to_string()),
        }];
        let skills = resolve_skill_sources(
            specs,
            &layout,
            &datadir,
            "sec-core",
            "openclaw",
            &resource_root,
        )
        .expect("skill under datadir must be accepted");
        assert_eq!(skills.len(), 1);
        assert_eq!(
            skills[0].source.as_ref().unwrap(),
            &datadir.join("skills").join("code-scanner"),
        );
    }

    #[test]
    fn skill_source_relative_escape_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let datadir = tmp.path().join("data");
        let resource_root = datadir.join("adapters").join("sec-core").join("openclaw");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let specs = vec![crate::manifest::AdapterSkillSpec {
            name: "code-scanner".to_string(),
            source: Some("../shared-skills/code-scanner".to_string()),
        }];
        let err = resolve_skill_sources(
            specs,
            &layout,
            &datadir,
            "sec-core",
            "openclaw",
            &resource_root,
        )
        .expect_err("relative path escaping resource_root must be rejected");
        assert!(
            matches!(err, AdapterError::InvalidAdapterInput { .. }),
            "expected InvalidAdapterInput, got {err:?}"
        );
    }

    #[test]
    fn skill_source_outside_boundary_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let datadir = tmp.path().join("data");
        let resource_root = datadir.join("adapters").join("sec-core").join("openclaw");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let specs = vec![crate::manifest::AdapterSkillSpec {
            name: "x".to_string(),
            source: Some("/etc".to_string()),
        }];
        let err = resolve_skill_sources(
            specs,
            &layout,
            &datadir,
            "sec-core",
            "openclaw",
            &resource_root,
        )
        .expect_err("source pointing to /etc must be rejected");
        assert!(
            matches!(err, AdapterError::InvalidAdapterInput { .. }),
            "expected InvalidAdapterInput, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("outside the allowed roots"),
            "error must explain boundary violation: {msg}"
        );
    }

    #[test]
    fn skill_source_none_is_accepted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let datadir = tmp.path().join("data");
        let resource_root = datadir.join("adapters").join("sec-core").join("openclaw");

        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let specs = vec![crate::manifest::AdapterSkillSpec {
            name: "code-scanner".to_string(),
            source: None,
        }];
        let skills = resolve_skill_sources(
            specs,
            &layout,
            &datadir,
            "sec-core",
            "openclaw",
            &resource_root,
        )
        .expect("no source must be accepted");
        assert_eq!(skills.len(), 1);
        assert!(skills[0].source.is_none());
    }
}
