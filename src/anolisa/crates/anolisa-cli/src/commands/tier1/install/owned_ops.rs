//! [`OwnedOps`] ports over the raw backend.
//!
//! [`RawReplayOps`] serves replay plans — re-placing an owned installation at
//! its recorded version (backup, download+verify, remove, place,
//! capabilities, restart, record). [`RawTeardownOps`] serves uninstall plans
//! (X1: hooks, stop services, remove files, drop record). Both adapt the raw
//! backend's existing primitives — the download cache, the install runner,
//! capability/service application, and the v5 state store — to the owned
//! executor's step vocabulary, and hold the working state their steps share.
//!
//! Only each plan family's subset is wired. Steps a plan never contains
//! return an honest error instead of a silent no-op, so a future plan that
//! reaches them fails loudly at the exact step rather than committing a
//! record that lies.

use std::fs;
use std::path::{Path, PathBuf};

use anolisa_core::central_log::CentralLog;
use anolisa_core::domain::{
    Installation, InstallationScope, LifecycleStatus, OwnedArtifact, ProviderBinding,
};
use anolisa_core::install_runner::{InstallRunner, InstalledFile, PreparedFileSet};
use anolisa_core::lifecycle::prepare_backup;
use anolisa_core::owned_executor::{OwnedOpError, OwnedOps, StepSuccess};
use anolisa_core::path_safety::validate_owned_path;
use anolisa_core::planner::{HookKind, RecordWrite};
use anolisa_core::state::{FileOwner, ObjectKind, OwnedFile, OwnedFileKind, ServiceRef};
use anolisa_core::state_store::StateStore;
use anolisa_core::transaction::restore_backup_file;
use anolisa_core::{
    ResolvedLifecycleHooks, ServiceActivation, ServiceRequest, ServiceRunOutcome, ServiceScope,
    apply_capabilities, apply_services, capability_for_install_mode, deactivate_services,
    run_hooks, service_for_install_mode, user_service_for_install_mode,
};
use anolisa_platform::fs_layout::FsLayout;

use crate::context::CliContext;
use crate::response::CliError;

use super::io_util::{
    rollback_activated_services, rollback_installed_files, rollback_installed_manifest,
    service_cleanup_suffix, write_installed_component_manifest,
};
use super::provision::{retained_packages_note, run_provision};
use super::raw::{InstallHooks, prepare_raw_execution, resolve_install_hooks};
use super::render::artifact_type_wire;
use super::types::{PreparedInstall, RawResolution};

struct ReplayBackup {
    source: PathBuf,
    dest: PathBuf,
    sha256: Option<String>,
}

/// Raw-backend [`OwnedOps`] for one replay operation.
pub(crate) struct RawReplayOps<'a> {
    ctx: &'a CliContext,
    layout: &'a FsLayout,
    component: String,
    scope: InstallationScope,
    now: String,
    operation_id: String,
    env: anolisa_env::EnvFacts,
    log: CentralLog,
    /// Resolution to download from; consumed by [`OwnedOps::download_verify`].
    resolution: Option<RawResolution>,
    /// The record's artifact at plan time: what to back up, remove, and
    /// preserve (provisioned packages, service enablement) across the replay.
    prior: OwnedArtifact,
    /// Prepared artifact + resolved contract, set by `download_verify`.
    prepared: Option<PreparedInstall>,
    prepared_files: Option<PreparedFileSet>,
    /// Files this run placed, set by `place_files`.
    placed: Vec<InstalledFile>,
    /// Manifest snapshot this run wrote, set by `place_files`.
    manifest_path: Option<PathBuf>,
    backups: Vec<ReplayBackup>,
    backup_root: PathBuf,
    /// Activation result, read back by the record commit.
    service_run: Option<ServiceRunOutcome>,
    /// Probe the new artifact's runtime dependencies during download-verify.
    runtime_preflight: bool,
    store: &'a mut StateStore,
    state_path: &'a Path,
}

impl<'a> RawReplayOps<'a> {
    /// Bind the port to one operation. `prior` is the record's artifact as
    /// re-validated under the install lock — its file list drives backup and
    /// removal, so it must never come from a pre-lock snapshot.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        ctx: &'a CliContext,
        layout: &'a FsLayout,
        component: String,
        scope: InstallationScope,
        now: String,
        operation_id: String,
        resolution: RawResolution,
        prior: OwnedArtifact,
        store: &'a mut StateStore,
        state_path: &'a Path,
    ) -> Self {
        let backup_root = layout.backup_dir.join(&operation_id);
        Self {
            ctx,
            layout,
            component,
            scope,
            now,
            env: anolisa_env::EnvService::detect(),
            log: CentralLog::open(layout.central_log.clone()),
            resolution: Some(resolution),
            prior,
            prepared: None,
            prepared_files: None,
            placed: Vec::new(),
            manifest_path: None,
            backups: Vec::new(),
            backup_root,
            service_run: None,
            runtime_preflight: false,
            store,
            state_path,
            operation_id,
        }
    }

    /// Probe the new artifact's runtime dependencies during download-verify.
    ///
    /// Update plans opt in: a newer artifact may declare dependencies the
    /// installed version did not, and replacing files on a host that misses
    /// them strands the component exactly like a fresh install would. Replay
    /// plans skip it — the recorded version's dependencies were satisfied
    /// when it was installed.
    pub(crate) fn with_runtime_preflight(mut self) -> Self {
        self.runtime_preflight = true;
        self
    }

    /// Remove the per-operation backup scratch. Call after the plan
    /// committed; a failed plan keeps its backups on disk for forensics.
    pub(crate) fn discard_backups(&self) {
        let _ = fs::remove_dir_all(&self.backup_root);
    }

    fn prepared(&self) -> Result<&PreparedInstall, OwnedOpError> {
        self.prepared.as_ref().ok_or_else(|| {
            OwnedOpError("internal: step ran before the download-verify step".to_string())
        })
    }

    /// Service rows for the record. Restart does not change enablement, so
    /// each unit keeps the enabled flag the prior record gave it.
    fn service_refs(&self, services: &[ServiceRequest]) -> Vec<ServiceRef> {
        services
            .iter()
            .map(|svc| ServiceRef {
                name: svc.unit.clone(),
                manager: svc.scope.manager_label().to_string(),
                restartable: true,
                enabled: self
                    .prior
                    .services
                    .iter()
                    .any(|prior| prior.name == svc.unit && prior.enabled),
                scope: svc.scope,
            })
            .collect()
    }

    fn not_wired(what: &str) -> Result<StepSuccess, OwnedOpError> {
        Err(OwnedOpError(format!(
            "{what} is not wired for owned replay yet"
        )))
    }
}

impl OwnedOps for RawReplayOps<'_> {
    fn download_verify(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let resolution = self.resolution.take().ok_or_else(|| {
            OwnedOpError("internal: download-verify ran twice in one plan".to_string())
        })?;
        let prepared = prepare_raw_execution(self.ctx, self.layout, resolution)
            .map_err(|err| OwnedOpError(err.to_string()))?;
        let prepared_files = InstallRunner::new(self.layout)
            .prepare_replacement_files(
                artifact_type_wire(&prepared.resolution.entry.artifact_type),
                &prepared.artifact_path,
                &prepared.files,
            )
            .map_err(|err| OwnedOpError(format!("failed to inspect verified payload: {err}")))?;
        let mut warnings = Vec::new();
        if self.runtime_preflight {
            let manifest = anolisa_core::ComponentManifest::from_toml_str(&prepared.manifest_toml)
                .map_err(|err| {
                    OwnedOpError(format!(
                        "failed to parse component manifest for preflight: {err}"
                    ))
                })?;
            warnings = super::provision::run_runtime_preflight(&manifest, &self.env, "update")
                .map_err(|err| OwnedOpError(err.reason()))?;
        }
        self.prepared = Some(prepared);
        self.prepared_files = Some(prepared_files);
        Ok(StepSuccess::with_warnings(warnings))
    }

    fn provision_runtime_deps(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("runtime-dependency provisioning")
    }

    fn run_hook(&mut self, kind: HookKind) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired(&format!("the {kind:?} hook phase"))
    }

    fn backup_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        for (idx, file) in self.prior.files.iter().enumerate() {
            validate_owned_path(self.layout, &file.path).map_err(|err| {
                OwnedOpError(format!(
                    "recorded owned file {} is outside ANOLISA-owned roots: {err}",
                    file.path.display()
                ))
            })?;
            let backup_path = self.backup_root.join(format!("{idx}.bak"));
            match prepare_backup(&file.path, &backup_path) {
                Ok(Some(artifact)) => self.backups.push(ReplayBackup {
                    source: backup_path,
                    dest: file.path.clone(),
                    sha256: artifact.into_sha256(),
                }),
                // Already gone from disk — nothing to preserve; placement
                // recreates it.
                Ok(None) => {}
                Err(err) => {
                    return Err(OwnedOpError(format!(
                        "failed to back up {}: {err}",
                        file.path.display()
                    )));
                }
            }
        }
        Ok(StepSuccess::clean())
    }

    fn place_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let prepared_files = self.prepared_files.take().ok_or_else(|| {
            OwnedOpError("internal: placement ran before payload preparation".to_string())
        })?;
        let runner = InstallRunner::new(self.layout);
        let outcome = runner
            .install_prepared(prepared_files)
            .map_err(|err| OwnedOpError(format!("placing files failed: {err}")))?;
        let prepared = self.prepared()?;
        match write_installed_component_manifest(
            self.layout,
            &self.component,
            &prepared.manifest_toml,
        ) {
            Ok(path) => {
                self.placed = outcome.files;
                self.manifest_path = Some(path);
                Ok(StepSuccess::clean())
            }
            Err(err) => {
                // The step did not complete: clean up its own partial work so
                // the executor never registers an undo for half-placed files.
                rollback_installed_files(&outcome.files);
                Err(OwnedOpError(err.to_string()))
            }
        }
    }

    fn set_capabilities(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let prepared = self.prepared()?;
        let manager = capability_for_install_mode(self.ctx.install_mode.as_str(), &self.env);
        let outcome = apply_capabilities(
            manager.as_ref(),
            &prepared.capabilities,
            Some(&self.log),
            &self.component,
            &self.operation_id,
            "cli",
            self.ctx.install_mode.as_str(),
        );
        if let Some(reason) = outcome.aborted {
            return Err(OwnedOpError(format!(
                "required capability application failed: {reason}"
            )));
        }
        Ok(StepSuccess::with_warnings(outcome.warnings))
    }

    fn enable_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("service enablement")
    }

    fn restart_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let prepared = self.prepared()?;
        let services = prepared.services.clone();
        let mode = self.ctx.install_mode.as_str();
        // Scope-matched backend, as install/update pick it: an all-user-scope
        // contract restarts through `systemctl --user`.
        let manager: Box<dyn anolisa_core::ServiceManager> =
            if !services.is_empty() && services.iter().all(|s| s.scope == ServiceScope::User) {
                user_service_for_install_mode(mode, &self.env)
            } else {
                service_for_install_mode(mode, &self.env)
            };
        let run = apply_services(
            manager.as_ref(),
            &services,
            ServiceActivation::Restart,
            Some(&self.log),
            &self.component,
            &self.operation_id,
            "cli",
            mode,
        );
        let warnings = run.warnings.clone();
        self.service_run = Some(run);
        Ok(StepSuccess::with_warnings(warnings))
    }

    fn stop_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("service stop")
    }

    fn remove_owned_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        for file in &self.prior.files {
            match fs::remove_file(&file.path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(OwnedOpError(format!(
                        "failed to remove {}: {err}",
                        file.path.display()
                    )));
                }
            }
        }
        Ok(StepSuccess::clean())
    }

    fn write_record(&mut self, write: RecordWrite) -> Result<StepSuccess, OwnedOpError> {
        if write != RecordWrite::Owned {
            return Err(OwnedOpError(format!(
                "raw replay cannot write a {} record",
                write.label()
            )));
        }
        let prepared = self.prepared()?;
        let manifest_path = self.manifest_path.clone().ok_or_else(|| {
            OwnedOpError("internal: record commit ran before files were placed".to_string())
        })?;
        let artifact = OwnedArtifact {
            version: prepared.resolution.entry.version.clone(),
            distribution_source: Some(prepared.resolution.artifact_url.clone()),
            raw_package: Some(prepared.resolution.package.clone()),
            // Digest verification of the embedded manifest is future work;
            // recording an unverified digest would overstate what ran.
            manifest_digest: None,
            files: owned_file_rows(&self.placed, &manifest_path, &prepared.manifest_toml),
            services: self.service_refs(&prepared.services),
            // A clean replay leaves no externally-modified files behind.
            external_modified_files: Vec::new(),
            // Provisioning did not run; the packages the original install
            // provisioned are still this installation's responsibility.
            provisioned_packages: self.prior.provisioned_packages.clone(),
        };

        match self.store.find_mut(ObjectKind::Component, &self.component) {
            Some(existing) => {
                existing.binding = ProviderBinding::Owned { artifact };
                existing.status = LifecycleStatus::Installed;
                existing.last_operation_id = Some(self.operation_id.clone());
                existing.health = Vec::new();
            }
            None => {
                self.store.upsert(Installation {
                    kind: ObjectKind::Component,
                    name: self.component.clone(),
                    scope: self.scope,
                    binding: ProviderBinding::Owned { artifact },
                    status: LifecycleStatus::Installed,
                    installed_at: self.now.clone(),
                    last_operation_id: Some(self.operation_id.clone()),
                    subscription_scope: Default::default(),
                    enabled_features: Vec::new(),
                    health: Vec::new(),
                });
            }
        }
        self.store
            .save(self.state_path)
            .map_err(|err| OwnedOpError(format!("failed to save state: {err}")))?;
        Ok(StepSuccess::clean())
    }

    fn drop_record(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("record removal")
    }

    fn undo_place_files(&mut self) -> Vec<String> {
        rollback_installed_files(&self.placed);
        self.placed.clear();
        if let Some(path) = self.manifest_path.take() {
            rollback_installed_manifest(&path);
        }
        Vec::new()
    }

    fn undo_enable_services(&mut self) -> Vec<String> {
        // Replay plans restart services, they never enable them, so the
        // executor never registers this compensation against this port.
        Vec::new()
    }

    fn restore_backup(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        for backup in &self.backups {
            if let Err(err) =
                restore_backup_file(&backup.source, &backup.dest, backup.sha256.as_deref())
            {
                warnings.push(format!(
                    "failed to restore {} from its backup: {err}",
                    backup.dest.display()
                ));
            }
        }
        warnings
    }
}

/// Raw-backend [`OwnedOps`] for one uninstall (X1) operation: contract
/// hooks, service stop/disable, owned-file removal, and the record drop.
pub(crate) struct RawTeardownOps<'a> {
    layout: &'a FsLayout,
    component: String,
    install_mode: String,
    operation_id: String,
    env: anolisa_env::EnvFacts,
    log: CentralLog,
    /// The record's artifact as re-validated under the install lock: its
    /// file list drives removal and its service list drives the stop.
    prior: OwnedArtifact,
    /// Pre/post-uninstall hooks resolved from the installed manifest
    /// snapshot (best-effort: a missing snapshot means no hooks).
    hooks: ResolvedLifecycleHooks,
    store: &'a mut StateStore,
    state_path: &'a Path,
}

impl<'a> RawTeardownOps<'a> {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        ctx: &CliContext,
        layout: &'a FsLayout,
        component: String,
        operation_id: String,
        prior: OwnedArtifact,
        hooks: ResolvedLifecycleHooks,
        store: &'a mut StateStore,
        state_path: &'a Path,
    ) -> Self {
        Self {
            layout,
            component,
            install_mode: ctx.install_mode.as_str().to_string(),
            operation_id,
            env: anolisa_env::EnvService::detect(),
            log: CentralLog::open(layout.central_log.clone()),
            prior,
            hooks,
            store,
            state_path,
        }
    }

    /// Turn a hook-run result into the step result: a strict failure is an
    /// `Err`, everything else succeeds with the collected warnings.
    fn hook_step(&self, run: anolisa_core::HookRunResult) -> Result<StepSuccess, OwnedOpError> {
        if let Some(failure) = run.hard_failure {
            return Err(OwnedOpError(failure.summary()));
        }
        Ok(StepSuccess::with_warnings(run.warnings))
    }

    fn not_wired(what: &str) -> Result<StepSuccess, OwnedOpError> {
        Err(OwnedOpError(format!(
            "{what} is not wired for owned teardown yet"
        )))
    }
}

impl OwnedOps for RawTeardownOps<'_> {
    fn download_verify(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("artifact download")
    }

    fn provision_runtime_deps(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("runtime-dependency provisioning")
    }

    fn run_hook(&mut self, kind: HookKind) -> Result<StepSuccess, OwnedOpError> {
        let specs = match kind {
            HookKind::PreUninstall => &self.hooks.pre_uninstall,
            HookKind::PostUninstall => &self.hooks.post_uninstall,
            other => return Self::not_wired(&format!("the {other:?} hook phase")),
        };
        let run = run_hooks(
            specs,
            self.layout,
            Some(&self.log),
            &self.operation_id,
            "cli",
            &self.install_mode,
        );
        self.hook_step(run)
    }

    fn backup_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("file backup")
    }

    fn place_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("file placement")
    }

    fn set_capabilities(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("capability application")
    }

    fn enable_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("service enablement")
    }

    fn restart_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("service restart")
    }

    /// Stop AND disable every recorded unit before the files go, so a
    /// running daemon shuts down cleanly and no orphan `enabled` symlink
    /// survives the uninstall. Best-effort by convention: trouble surfaces
    /// as warnings, never as a failed uninstall.
    fn stop_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let mut sys_units: Vec<(String, String)> = Vec::new();
        let mut user_units: Vec<(String, String)> = Vec::new();
        for service in &self.prior.services {
            let row = (self.component.clone(), service.name.clone());
            match service.scope {
                ServiceScope::User => user_units.push(row),
                ServiceScope::System => sys_units.push(row),
            }
        }
        let mut warnings = Vec::new();
        for (units, manager) in [
            (
                sys_units,
                service_for_install_mode(&self.install_mode, &self.env),
            ),
            (
                user_units,
                user_service_for_install_mode(&self.install_mode, &self.env),
            ),
        ] {
            if units.is_empty() {
                continue;
            }
            let outcome = deactivate_services(
                manager.as_ref(),
                &units,
                Some(&self.log),
                &self.operation_id,
                "cli",
                &self.install_mode,
            );
            warnings.extend(outcome.warnings);
        }
        Ok(StepSuccess::with_warnings(warnings))
    }

    /// Remove the recorded owned files. A path outside the ANOLISA-owned
    /// roots is skipped with a warning instead of failing the teardown: a
    /// forged or stale record must not turn uninstall into an
    /// arbitrary-delete primitive, and skipping keeps the operation moving
    /// on the legitimate files.
    fn remove_owned_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let mut warnings = Vec::new();
        for file in &self.prior.files {
            if let Err(boundary) = validate_owned_path(self.layout, &file.path) {
                warnings.push(format!(
                    "skipped {}: outside ANOLISA-owned roots ({boundary})",
                    file.path.display()
                ));
                continue;
            }
            match fs::remove_file(&file.path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(OwnedOpError(format!(
                        "failed to remove {}: {err}",
                        file.path.display()
                    )));
                }
            }
        }
        Ok(StepSuccess::with_warnings(warnings))
    }

    fn write_record(&mut self, _write: RecordWrite) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("record write")
    }

    fn drop_record(&mut self) -> Result<StepSuccess, OwnedOpError> {
        self.store.remove(ObjectKind::Component, &self.component);
        self.store
            .save(self.state_path)
            .map_err(|err| OwnedOpError(format!("failed to save state: {err}")))?;
        // The manifest snapshot directory travels with the record; its file
        // was already removed with the owned files, the empty directory goes
        // here. Best-effort: the record is already dropped and committed.
        let mut warnings = Vec::new();
        if let Ok(dir) = crate::commands::common::installed_component_manifest_dir(
            self.layout,
            &self.component,
            "uninstall",
        ) && let Err(err) = fs::remove_dir_all(&dir)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            warnings.push(format!(
                "failed to remove component manifest snapshot at {}: {err}",
                dir.display()
            ));
        }
        Ok(StepSuccess::with_warnings(warnings))
    }

    fn undo_place_files(&mut self) -> Vec<String> {
        // X1 plans never place files, so this compensation is never
        // registered against this port.
        Vec::new()
    }

    fn undo_enable_services(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn restore_backup(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// [`OwnedOps`] for the quarantine-restore exit (repair R6): a quarantined
/// legacy record whose owned files verified intact is rebuilt into an active
/// owned record. The only wired step is the record write — the files are
/// already on disk and already verified, so nothing else may touch the host.
pub(crate) struct QuarantineRestoreOps<'a> {
    component: String,
    scope: InstallationScope,
    operation_id: String,
    store: &'a mut StateStore,
    state_path: &'a Path,
}

impl<'a> QuarantineRestoreOps<'a> {
    pub(crate) fn new(
        component: String,
        scope: InstallationScope,
        operation_id: String,
        store: &'a mut StateStore,
        state_path: &'a Path,
    ) -> Self {
        Self {
            component,
            scope,
            operation_id,
            store,
            state_path,
        }
    }

    fn not_wired(what: &str) -> Result<StepSuccess, OwnedOpError> {
        Err(OwnedOpError(format!(
            "{what} is not wired for quarantine restore"
        )))
    }
}

impl OwnedOps for QuarantineRestoreOps<'_> {
    fn download_verify(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("artifact download")
    }

    fn provision_runtime_deps(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("runtime-dependency provisioning")
    }

    fn run_hook(&mut self, kind: HookKind) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired(&format!("the {kind:?} hook phase"))
    }

    fn backup_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("file backup")
    }

    fn place_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("file placement")
    }

    fn set_capabilities(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("capability application")
    }

    fn enable_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("service enablement")
    }

    fn restart_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("service restart")
    }

    fn stop_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("service stop")
    }

    fn remove_owned_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("owned-file removal")
    }

    fn write_record(&mut self, write: RecordWrite) -> Result<StepSuccess, OwnedOpError> {
        if write != RecordWrite::Owned {
            return Err(OwnedOpError(format!(
                "quarantine restore cannot write a {} record",
                write.label()
            )));
        }
        let quarantined = self
            .store
            .quarantined
            .iter()
            .find(|q| q.record.kind == ObjectKind::Component && q.record.name == self.component)
            .ok_or_else(|| {
                OwnedOpError(format!(
                    "no quarantined record for '{}' remains to restore",
                    self.component
                ))
            })?;
        let mut installation =
            anolisa_core::state_migration::owned_installation(&quarantined.record, self.scope);
        // The restore is itself the verdict that this installation is
        // healthy again — the legacy status must not survive it.
        installation.status = LifecycleStatus::Installed;
        installation.last_operation_id = Some(self.operation_id.clone());
        // Upsert consumes the same-identity quarantined record.
        self.store.upsert(installation);
        self.store
            .save(self.state_path)
            .map_err(|err| OwnedOpError(format!("failed to save state: {err}")))?;
        Ok(StepSuccess::clean())
    }

    fn drop_record(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("record removal")
    }

    fn undo_place_files(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn undo_enable_services(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn restore_backup(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// A fresh install's artifact and contract, downloaded, verified, and
/// validated before any lock or journal exists.
pub(crate) struct ValidatedInstall {
    prepared: PreparedInstall,
    prepared_files: PreparedFileSet,
    manifest: anolisa_core::ComponentManifest,
    hooks: InstallHooks,
}

impl ValidatedInstall {
    /// Package name the resolution settled on.
    pub(crate) fn package(&self) -> &str {
        &self.prepared.resolution.package
    }

    /// Version the resolution settled on.
    pub(crate) fn version(&self) -> &str {
        &self.prepared.resolution.entry.version
    }

    /// Warnings accumulated during resolution.
    pub(crate) fn warnings(&self) -> &[String] {
        &self.prepared.resolution.warnings
    }
}

/// Download+verify the resolved artifact and validate its install contract —
/// install modes, component conflicts, hook declarations — with typed
/// errors. Runs before the install lock: everything here is side-effect free
/// outside the download cache, and a contract refusal must surface as
/// INVALID_ARGUMENT rather than a failed transaction.
pub(crate) fn validate_owned_install(
    ctx: &CliContext,
    layout: &FsLayout,
    store: &StateStore,
    resolution: RawResolution,
    command: &str,
) -> Result<ValidatedInstall, CliError> {
    let component = resolution.component.clone();
    let prepared = prepare_raw_execution(ctx, layout, resolution)?;
    let manifest = anolisa_core::ComponentManifest::from_toml_str(&prepared.manifest_toml)
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to parse component manifest: {err}"),
        })?;
    if let Some(reason) = component_conflict(&manifest, store) {
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason,
        });
    }
    let hooks = resolve_install_hooks(&manifest, layout, &component)?;
    let prepared_files = InstallRunner::new(layout)
        .prepare_files(
            artifact_type_wire(&prepared.resolution.entry.artifact_type),
            &prepared.artifact_path,
            &prepared.files,
        )
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("failed to inspect verified install payload: {err}"),
        })?;
    Ok(ValidatedInstall {
        prepared,
        prepared_files,
        manifest,
        hooks,
    })
}

/// Component-level mutual exclusion (the raw equivalent of RPM's
/// `Conflicts:` tag): the refusal message when the contract conflicts with
/// an installed component.
fn component_conflict(
    manifest: &anolisa_core::ComponentManifest,
    store: &StateStore,
) -> Option<String> {
    for conflict in &manifest.component.conflicts {
        if let Some(installed) = store.find(ObjectKind::Component, conflict) {
            return Some(format!(
                "component '{}' conflicts with installed component '{}' (v{}) — uninstall '{}' first, then retry",
                manifest.component.name,
                conflict,
                installed_version_label(installed),
                conflict,
            ));
        }
    }
    None
}

/// [`OwnedOps`] for one fresh owned install (I1): download+verify (with
/// conflict and hook-contract validation), runtime-dependency provisioning,
/// pre/post-install hooks, file placement, capabilities, service activation,
/// and the record write.
///
/// The contract's `post_enable` hooks run inside the service-activation step
/// (the planner's step vocabulary has no post-enable phase): a strict
/// failure deactivates the units this step just brought up and fails the
/// step, so the executor compensates the earlier steps normally.
pub(crate) struct RawInstallOps<'a> {
    ctx: &'a CliContext,
    layout: &'a FsLayout,
    component: String,
    scope: InstallationScope,
    now: String,
    operation_id: String,
    env: anolisa_env::EnvFacts,
    log: CentralLog,
    /// Prepared artifact + resolved contract, validated pre-lock.
    prepared: Option<PreparedInstall>,
    prepared_files: Option<PreparedFileSet>,
    /// Parsed contract manifest, validated pre-lock.
    manifest: Option<anolisa_core::ComponentManifest>,
    /// Install hooks resolved from the contract, validated pre-lock.
    hooks: Option<InstallHooks>,
    /// System packages auto-installed by `provision_runtime_deps`.
    /// Intentionally never rolled back.
    provisioned_packages: Vec<String>,
    /// Files this run placed, set by `place_files`.
    placed: Vec<InstalledFile>,
    /// Manifest snapshot this run wrote, set by `place_files`.
    manifest_path: Option<PathBuf>,
    /// Activation result, read back by the record commit and the undo.
    service_run: Option<ServiceRunOutcome>,
    store: &'a mut StateStore,
    state_path: &'a Path,
}

impl<'a> RawInstallOps<'a> {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new(
        ctx: &'a CliContext,
        layout: &'a FsLayout,
        component: String,
        scope: InstallationScope,
        now: String,
        operation_id: String,
        validated: ValidatedInstall,
        store: &'a mut StateStore,
        state_path: &'a Path,
    ) -> Self {
        let ValidatedInstall {
            prepared,
            prepared_files,
            manifest,
            hooks,
        } = validated;
        Self {
            ctx,
            layout,
            component,
            scope,
            now,
            env: anolisa_env::EnvService::detect(),
            log: CentralLog::open(layout.central_log.clone()),
            prepared: Some(prepared),
            prepared_files: Some(prepared_files),
            manifest: Some(manifest),
            hooks: Some(hooks),
            provisioned_packages: Vec::new(),
            placed: Vec::new(),
            manifest_path: None,
            service_run: None,
            store,
            state_path,
            operation_id,
        }
    }

    /// The note appended to failure reports when system packages were
    /// provisioned but the install did not complete (they are retained).
    pub(crate) fn retained_packages_note(&self) -> String {
        retained_packages_note(&self.provisioned_packages)
    }

    fn prepared(&self) -> Result<&PreparedInstall, OwnedOpError> {
        self.prepared.as_ref().ok_or_else(|| {
            OwnedOpError("internal: step ran before the download-verify step".to_string())
        })
    }

    fn hooks(&self) -> Result<&InstallHooks, OwnedOpError> {
        self.hooks.as_ref().ok_or_else(|| {
            OwnedOpError("internal: hook step ran before the download-verify step".to_string())
        })
    }

    /// Scope-matched service manager, as install/update pick it: an
    /// all-user-scope contract drives `systemctl --user`.
    fn service_manager(
        &self,
        services: &[ServiceRequest],
    ) -> Box<dyn anolisa_core::ServiceManager> {
        let mode = self.ctx.install_mode.as_str();
        if !services.is_empty() && services.iter().all(|s| s.scope == ServiceScope::User) {
            user_service_for_install_mode(mode, &self.env)
        } else {
            service_for_install_mode(mode, &self.env)
        }
    }

    fn not_wired(what: &str) -> Result<StepSuccess, OwnedOpError> {
        Err(OwnedOpError(format!(
            "{what} is not wired for fresh install yet"
        )))
    }
}

impl OwnedOps for RawInstallOps<'_> {
    fn download_verify(&mut self) -> Result<StepSuccess, OwnedOpError> {
        // Fetch, digest check, and contract validation already ran pre-lock
        // (`validate_owned_install`). Only the conflict check repeats here,
        // against the store re-read under the lock, closing the window where
        // a conflicting component landed between validation and locking.
        let manifest = self.manifest.as_ref().ok_or_else(|| {
            OwnedOpError("internal: install ops built without a validated contract".to_string())
        })?;
        if let Some(reason) = component_conflict(manifest, self.store) {
            return Err(OwnedOpError(reason));
        }
        Ok(StepSuccess::clean())
    }

    fn provision_runtime_deps(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let manifest = self.manifest.as_ref().ok_or_else(|| {
            OwnedOpError("internal: provisioning ran before the download-verify step".to_string())
        })?;
        let mut warnings = Vec::new();
        self.provisioned_packages =
            run_provision(manifest, &self.env, self.ctx, super::COMMAND, &mut warnings)
                .map_err(|err| OwnedOpError(err.reason()))?;
        Ok(StepSuccess::with_warnings(warnings))
    }

    fn run_hook(&mut self, kind: HookKind) -> Result<StepSuccess, OwnedOpError> {
        let hooks = self.hooks()?;
        let specs = match kind {
            HookKind::PreInstall => hooks.pre_install.clone(),
            HookKind::PostInstall => hooks.post_install.clone(),
            other => return Self::not_wired(&format!("the {other:?} hook phase")),
        };
        let run = run_hooks(
            &specs,
            self.layout,
            Some(&self.log),
            &self.operation_id,
            "cli",
            self.ctx.install_mode.as_str(),
        );
        if let Some(failure) = run.hard_failure {
            return Err(OwnedOpError(failure.summary()));
        }
        Ok(StepSuccess::with_warnings(run.warnings))
    }

    fn backup_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("file backup")
    }

    fn place_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let prepared_files = self.prepared_files.take().ok_or_else(|| {
            OwnedOpError("internal: placement ran before payload preparation".to_string())
        })?;
        let runner = InstallRunner::new(self.layout);
        let outcome = runner
            .install_prepared(prepared_files)
            .map_err(|err| OwnedOpError(format!("placing files failed: {err}")))?;
        let prepared = self.prepared()?;
        match write_installed_component_manifest(
            self.layout,
            &self.component,
            &prepared.manifest_toml,
        ) {
            Ok(path) => {
                self.placed = outcome.files;
                self.manifest_path = Some(path);
                Ok(StepSuccess::clean())
            }
            Err(err) => {
                // The step did not complete: clean up its own partial work so
                // the executor never registers an undo for half-placed files.
                rollback_installed_files(&outcome.files);
                Err(OwnedOpError(err.to_string()))
            }
        }
    }

    fn set_capabilities(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let prepared = self.prepared()?;
        let manager = capability_for_install_mode(self.ctx.install_mode.as_str(), &self.env);
        let outcome = apply_capabilities(
            manager.as_ref(),
            &prepared.capabilities,
            Some(&self.log),
            &self.component,
            &self.operation_id,
            "cli",
            self.ctx.install_mode.as_str(),
        );
        if let Some(reason) = outcome.aborted {
            return Err(OwnedOpError(format!(
                "required capability application failed: {reason}"
            )));
        }
        Ok(StepSuccess::with_warnings(outcome.warnings))
    }

    fn enable_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        let prepared = self.prepared()?;
        let services = prepared.services.clone();
        let post_enable_specs = self.hooks()?.post_enable.clone();
        let manager = self.service_manager(&services);
        let mode = self.ctx.install_mode.as_str();
        // Activation is best-effort by convention: a failed enable/start is
        // a warning, not an abort — the files are installed and an operator
        // can fix a unit out of band.
        let run = apply_services(
            manager.as_ref(),
            &services,
            ServiceActivation::Start,
            Some(&self.log),
            &self.component,
            &self.operation_id,
            "cli",
            mode,
        );
        let mut warnings = run.warnings.clone();
        // The contract's post_enable hooks belong to this step: services are
        // up, the record is not yet written. A strict failure deactivates
        // the units this step just activated (its own partial work), then
        // fails the step so the executor unwinds the earlier steps.
        let post_enable = run_hooks(
            &post_enable_specs,
            self.layout,
            Some(&self.log),
            &self.operation_id,
            "cli",
            mode,
        );
        warnings.extend(post_enable.warnings);
        if let Some(failure) = post_enable.hard_failure {
            let cleanup = rollback_activated_services(
                manager.as_ref(),
                &run,
                Some(&self.log),
                &self.component,
                &self.operation_id,
                mode,
            );
            return Err(OwnedOpError(format!(
                "post_enable hook failed; stopped/disabled activated services{}: {}",
                service_cleanup_suffix(&cleanup),
                failure.summary()
            )));
        }
        self.service_run = Some(run);
        Ok(StepSuccess::with_warnings(warnings))
    }

    fn restart_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("service restart")
    }

    fn stop_services(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("service stop")
    }

    fn remove_owned_files(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("owned-file removal")
    }

    fn write_record(&mut self, write: RecordWrite) -> Result<StepSuccess, OwnedOpError> {
        if write != RecordWrite::Owned {
            return Err(OwnedOpError(format!(
                "fresh install cannot write a {} record",
                write.label()
            )));
        }
        let prepared = self.prepared()?;
        let manifest_path = self.manifest_path.clone().ok_or_else(|| {
            OwnedOpError("internal: record commit ran before files were placed".to_string())
        })?;
        let enabled_units: Vec<String> = self
            .service_run
            .as_ref()
            .map(|run| run.enabled_units.clone())
            .unwrap_or_default();
        let artifact = OwnedArtifact {
            version: prepared.resolution.entry.version.clone(),
            distribution_source: Some(prepared.resolution.artifact_url.clone()),
            raw_package: Some(prepared.resolution.package.clone()),
            // Digest verification of the embedded manifest is future work;
            // recording an unverified digest would overstate what ran.
            manifest_digest: None,
            files: owned_file_rows(&self.placed, &manifest_path, &prepared.manifest_toml),
            services: prepared
                .services
                .iter()
                .map(|svc| ServiceRef {
                    name: svc.unit.clone(),
                    manager: svc.scope.manager_label().to_string(),
                    restartable: true,
                    // Reflect what the executor actually enabled this run.
                    enabled: enabled_units.contains(&svc.unit),
                    scope: svc.scope,
                })
                .collect(),
            external_modified_files: Vec::new(),
            provisioned_packages: self.provisioned_packages.clone(),
        };
        // I1 only fires on an absent record, so this is a fresh insert;
        // upsert also consumes a same-name quarantined record, which the
        // planner has already refused (I10) before this step could run.
        self.store.upsert(Installation {
            kind: ObjectKind::Component,
            name: self.component.clone(),
            scope: self.scope,
            binding: ProviderBinding::Owned { artifact },
            status: LifecycleStatus::Installed,
            installed_at: self.now.clone(),
            last_operation_id: Some(self.operation_id.clone()),
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            health: Vec::new(),
        });
        self.store
            .save(self.state_path)
            .map_err(|err| OwnedOpError(format!("failed to save state: {err}")))?;
        Ok(StepSuccess::clean())
    }

    fn drop_record(&mut self) -> Result<StepSuccess, OwnedOpError> {
        Self::not_wired("record removal")
    }

    fn undo_place_files(&mut self) -> Vec<String> {
        rollback_installed_files(&self.placed);
        self.placed.clear();
        if let Some(path) = self.manifest_path.take() {
            rollback_installed_manifest(&path);
        }
        Vec::new()
    }

    fn undo_enable_services(&mut self) -> Vec<String> {
        let Some(run) = self.service_run.take() else {
            return Vec::new();
        };
        let services = self
            .prepared
            .as_ref()
            .map(|p| p.services.clone())
            .unwrap_or_default();
        let manager = self.service_manager(&services);
        rollback_activated_services(
            manager.as_ref(),
            &run,
            Some(&self.log),
            &self.component,
            &self.operation_id,
            self.ctx.install_mode.as_str(),
        )
    }

    fn restore_backup(&mut self) -> Vec<String> {
        // I1 plans never back up files, so this compensation is never
        // registered against this port.
        Vec::new()
    }
}

/// Version label for an installed record in a conflict report: the owned
/// artifact's version, or a delegated record's last observation.
pub(crate) fn installed_version_label(installation: &Installation) -> String {
    match &installation.binding {
        ProviderBinding::Owned { artifact } => artifact.version.clone(),
        ProviderBinding::Delegated { last_observed, .. } => last_observed
            .as_ref()
            .map(|o| o.version.clone())
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

/// Owned-file rows for a record commit: what this run placed, plus the
/// manifest snapshot it wrote.
fn owned_file_rows(
    placed: &[InstalledFile],
    manifest_path: &Path,
    manifest_toml: &str,
) -> Vec<OwnedFile> {
    let mut files: Vec<OwnedFile> = placed
        .iter()
        .map(|f| OwnedFile {
            path: f.path.clone(),
            owner: FileOwner::Anolisa,
            sha256: if f.referent.is_some() {
                None
            } else {
                Some(f.sha256.clone())
            },
            kind: if f.referent.is_some() {
                OwnedFileKind::Symlink
            } else {
                OwnedFileKind::File
            },
            referent: f.referent.clone(),
        })
        .collect();
    files.push(OwnedFile {
        path: manifest_path.to_path_buf(),
        owner: FileOwner::Anolisa,
        sha256: Some(sha256_hex(manifest_toml.as_bytes())),
        kind: OwnedFileKind::File,
        referent: None,
    });
    files
}

/// Lowercase-hex sha256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(bytes);
    hash.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}
