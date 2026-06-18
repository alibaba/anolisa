//! Installed state tracking (`installed.toml`).
//!
//! `InstalledState` is the on-disk record of every ANOLISA-managed object
//! (component / adapter / osbase) plus the backups and
//! operations that produced them. Persistence is TOML and save is atomic
//! (`tmp` + `rename`) so a crash mid-write cannot leave a truncated state
//! file.
//!
//! See `templates/installed-state.toml` and launch spec §8.1 for the
//! field-level contract.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::adapter::claim::AdapterClaim;

/// Current `installed.toml` schema version. Bump on incompatible changes.
/// When bumped, [`InstalledState::load`] must migrate older on-disk versions
/// into the current in-memory shape before returning.
///
/// v2 added the `adapter_claims` array (adapter receipts). The field
/// default-deserializes, so v1 files load unchanged and are silently
/// upgraded to v2 on the next save.
///
/// v3 added `ownership` (provenance model) and `rpm_metadata` to
/// [`InstalledObject`]. Both fields default-deserialize (`None`), so
/// older files load unchanged and gain the new fields on next save.
pub const STATE_SCHEMA_VERSION: u32 = 3;

/// Default for `bool` fields that should serialise to `true` when absent.
fn default_true() -> bool {
    true
}

/// Install mode reported in `installed.toml`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallMode {
    /// Per-user (`file-hierarchy(7)`) install scope.
    #[default]
    User,
    /// System-wide FHS install scope.
    System,
}

/// Discriminator for objects tracked in installed state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    /// Legacy capability object. The capability concept is removed; the
    /// variant survives only so `installed.toml` files written by older
    /// releases still deserialize. New code must never create objects of
    /// this kind; queries are limited to legacy-migration paths (see
    /// [`InstalledState::prune_legacy_capabilities`]).
    Capability,
    /// Runtime/osbase component.
    Component,
    /// Agent-framework adapter object.
    Adapter,
    /// OS base-layer object.
    Osbase,
}

/// Lifecycle status for an installed object.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectStatus {
    /// Object is fully installed and active.
    Installed,
    /// Object was partially installed or has a degraded dependency.
    Partial,
    /// Object is present but intentionally inactive.
    Disabled,
    /// Last mutating operation failed or health checks found a hard error.
    Failed,
    /// Object is tracked but not fully owned by ANOLISA.
    Adopted,
}

/// Subscription scope attached to an object.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionScope {
    /// No subscription entitlement is attached.
    #[default]
    None,
    /// Registered with a subscription backend.
    Registered,
    /// Entitlement was granted for this object.
    Entitled,
    /// Object reports usage or health to a subscription backend.
    Reporting,
}

/// Provenance and lifecycle ownership of an installed object.
///
/// Determines who holds removal authority and how upgrades are executed.
/// See `raw_rpm_lifecycle_proposal.md` §5 for the full ownership table.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    /// Installed via ANOLISA raw backend; ANOLISA manages owned files and
    /// may remove them on uninstall.
    RawManaged,
    /// Installed via ANOLISA-delegated RPM backend (`dnf install`);
    /// file transactions are owned by rpm/dnf, uninstall delegates to
    /// `dnf remove`.
    RpmManaged,
    /// Pre-existing system RPM adopted/observed by ANOLISA. ANOLISA does
    /// **not** own package removal: [`owns_removal`](Ownership::owns_removal)
    /// is `false`. Per the intended lifecycle contract
    /// (`raw_rpm_lifecycle_proposal.md` §11), `uninstall` should drop only the
    /// ANOLISA state record unless an explicit `--remove-system-package`
    /// override is given. That uninstall wiring is a follow-up; this change
    /// only models the ownership, it does not implement the removal path.
    RpmObserved,
}

impl Ownership {
    /// Whether ANOLISA holds removal authority for this ownership class.
    ///
    /// `rpm-observed` objects are tracked but not owned, so default
    /// uninstall must not invoke `dnf remove`.
    pub fn owns_removal(self) -> bool {
        match self {
            Self::RawManaged | Self::RpmManaged => true,
            Self::RpmObserved => false,
        }
    }

    /// Whether the object was installed via an RPM-based backend.
    pub fn is_rpm(self) -> bool {
        matches!(self, Self::RpmManaged | Self::RpmObserved)
    }

    /// Stable provenance label for wire output (`raw-managed`, `rpm-managed`,
    /// `rpm-observed`). Centralized so every command renders the same string
    /// and a future ownership class cannot be given two different labels.
    pub fn label(self) -> &'static str {
        match self {
            Self::RawManaged => "raw-managed",
            Self::RpmManaged => "rpm-managed",
            Self::RpmObserved => "rpm-observed",
        }
    }
}

/// RPM package metadata recorded when a component is managed or observed
/// through an RPM backend. Populated from `rpmdb` queries at adopt/install
/// time; refreshed on `repair` and `update`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpmMetadata {
    /// RPM package name (e.g. `anolisa-copilot-shell`).
    pub package_name: String,
    /// Full EVR (epoch:version-release) string from rpmdb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evr: Option<String>,
    /// Package architecture (`x86_64`, `aarch64`, `noarch`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// Source repository or label that supplied the package (e.g.
    /// `@System`, `anolisa-release`, `alinux-updates`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_repo: Option<String>,
}

/// File ownership: ANOLISA-owned vs. external (third-party).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileOwner {
    /// ANOLISA may install, verify, remove, and roll back this file.
    Anolisa,
    /// ANOLISA must preserve this file and only touch it through explicit
    /// external-file backup contracts.
    External,
}

/// File installed and owned by ANOLISA.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnedFile {
    /// Absolute path recorded at install time; status probes revalidate it
    /// against owned roots before any filesystem access.
    pub path: PathBuf,
    /// Ownership contract for uninstall and integrity checks.
    pub owner: FileOwner,
    /// Recorded content digest. Older state or externally adopted files may
    /// omit it, so integrity checks surface `unverified` instead of guessing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// External (non-ANOLISA) file that an operation modified. Linked back to
/// the originating [`BackupRecord`] by `backup_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalModifiedFile {
    /// Absolute path of the third-party file touched by an operation.
    pub path: PathBuf,
    /// Ownership marker; should remain [`FileOwner::External`] so uninstall
    /// refuses deletion.
    pub owner: FileOwner,
    /// Backup record that can restore the pre-operation content.
    pub backup_id: String,
    /// Digest before modification, when the file was readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256_before: Option<String>,
    /// Digest after modification, when ANOLISA can verify its own write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256_after: Option<String>,
}

/// Service unit installed or managed by an object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceRef {
    /// Native unit name such as `agentsight.service`.
    pub name: String,
    /// Service manager namespace (`systemd`, `launchd`, `none`, ...).
    pub manager: String,
    /// Whether `anolisa restart` may target this unit.
    #[serde(default)]
    pub restartable: bool,
    /// Desired enabled-on-boot state when a manager supports it.
    #[serde(default)]
    pub enabled: bool,
}

/// Last-known health probe result for an object.
///
/// `reason` is an optional human-readable detail that callers (status
/// renderer, JSON wire) surface alongside the status label. Manifest-driven
/// probes (file existence, command exit, systemd unit state) populate it
/// with a short pointer at why the check landed where it did so a user can
/// triage without re-running the probe by hand. Older state files written
/// before this field existed deserialize with `reason = None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthEntry {
    /// Probe name or manifest health-check identifier.
    pub name: String,
    /// Status label rendered by `anolisa status`.
    pub status: String,
    /// RFC3339 UTC timestamp when the probe last ran.
    pub checked_at: String,
    /// Optional explanation for non-obvious status outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A single installed object (component, adapter, or osbase).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledObject {
    /// Object vocabulary used by commands and state lookup.
    pub kind: ObjectKind,
    /// Stable object name from the manifest/catalog.
    pub name: String,
    /// Version installed or adopted into state.
    pub version: String,
    /// Lifecycle state used by list/status filters.
    pub status: ObjectStatus,
    /// Digest of the manifest used for install. Optional for older state and
    /// adopted objects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<String>,
    /// Distribution entry URL or backend-specific source that supplied bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution_source: Option<String>,
    /// Backend that resolved and installed this object.
    ///
    /// Install refuses a later attempt through a different backend so a
    /// component's provenance stays deterministic across updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_backend: Option<String>,
    /// Provenance/ownership class for lifecycle decisions (removal,
    /// upgrade delegation). `None` on state files written before v3;
    /// callers fall back to inspecting `managed` / `adopted` /
    /// `install_backend` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<Ownership>,
    /// RPM metadata populated when [`ownership`](Self::ownership) is
    /// [`RpmManaged`](Ownership::RpmManaged) or
    /// [`RpmObserved`](Ownership::RpmObserved). `None` for raw installs
    /// and pre-v3 state files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm_metadata: Option<RpmMetadata>,
    /// RFC3339 UTC timestamp when this object entered state.
    pub installed_at: String,
    /// Last operation that changed this object, shared with central log rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_operation_id: Option<String>,
    /// False for externally adopted objects that ANOLISA should not mutate as
    /// normal owned installs.
    #[serde(default = "default_true")]
    pub managed: bool,
    /// Explicit adoption marker kept separate from `managed` for UI/audit
    /// vocabulary.
    #[serde(default)]
    pub adopted: bool,
    /// Subscription entitlement attached to this object.
    #[serde(default)]
    pub subscription_scope: SubscriptionScope,
    /// Enabled feature names, omitted from TOML when empty to preserve compact
    /// state files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_features: Vec<String>,
    /// Legacy capability-to-component linkage; retained so old state files
    /// still deserialize. Component objects leave it empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_refs: Vec<String>,
    /// ANOLISA-owned files that status/uninstall may verify or remove.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<OwnedFile>,
    /// Third-party files touched under explicit backup contracts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_modified_files: Vec<ExternalModifiedFile>,
    /// Service units associated with this object.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ServiceRef>,
    /// Cached health results from the last status/probe pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub health: Vec<HealthEntry>,
}

impl InstalledObject {
    /// Effective ownership, resolving `None` (pre-v3 state) by inspecting
    /// legacy fields `managed`, `adopted`, and `install_backend`.
    pub fn effective_ownership(&self) -> Ownership {
        if let Some(o) = self.ownership {
            return o;
        }
        // Legacy heuristic, reached only when `ownership` is absent — i.e.
        // pre-v3 files, since every v3 write sets `ownership` explicitly.
        // A pre-v3 adopted RPM was recorded either via `adopted = true` or
        // via `managed = false` (the "external, do not mutate" marker), so
        // both imply rpm-observed when the backend is RPM. This cannot
        // misclassify future writes: they never reach the heuristic.
        // (adopted || !managed) + RPM → rpm-observed; managed + RPM →
        // rpm-managed; otherwise raw-managed.
        if (self.adopted || !self.managed) && self.install_backend.as_deref() == Some("rpm") {
            return Ownership::RpmObserved;
        }
        if self.install_backend.as_deref() == Some("rpm") {
            return Ownership::RpmManaged;
        }
        Ownership::RawManaged
    }

    /// Whether this object represents a pre-existing system RPM that
    /// ANOLISA only observes without claiming removal authority.
    pub fn is_rpm_observed(&self) -> bool {
        self.effective_ownership() == Ownership::RpmObserved
    }

    /// Whether default uninstall may remove this object's backing files
    /// or packages.
    pub fn owns_removal(&self) -> bool {
        self.effective_ownership().owns_removal()
    }
}

/// Backup metadata recorded when an operation touched an external file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupRecord {
    /// Stable backup identifier used by state and logs.
    pub id: String,
    /// Operation that created this backup.
    pub operation_id: String,
    /// Original file path before the mutating operation.
    pub original_path: PathBuf,
    /// Backup copy path under the ANOLISA backup root.
    pub backup_path: PathBuf,
    /// Strategy hint for future repair tooling.
    pub restore_strategy: String,
}

/// Operation record for an `installed.toml` audit trail entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationRecord {
    /// Operation id shared with central logs and transaction journals.
    pub id: String,
    /// User-facing command or operation verb.
    pub command: String,
    /// Terminal status label (`started`, `ok`, `failed`, ...).
    pub status: String,
    /// RFC3339 UTC start timestamp.
    pub started_at: String,
    /// RFC3339 UTC finish timestamp; absent while an operation is in flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

/// On-disk record of installed objects, backups, and operation history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledState {
    /// On-disk schema version for migration decisions.
    pub schema_version: u32,
    /// RFC3339 UTC timestamp refreshed on every save.
    pub updated_at: String,
    /// Install scope used to interpret paths in this state file.
    pub install_mode: InstallMode,
    /// Prefix recorded for diagnostics and future migrations.
    pub prefix: PathBuf,
    /// ANOLISA version that last wrote the state file.
    pub anolisa_version: String,
    /// Installed/adopted objects tracked by name and kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<InstalledObject>,
    /// Backup metadata created by lifecycle transactions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backups: Vec<BackupRecord>,
    /// Lightweight operation history mirrored by central logs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<OperationRecord>,
    /// Adapter receipts written by `anolisa adapter enable`. Per-user
    /// state: each records a framework driver's takeover of framework-side
    /// state for one component. Empty on fresh and pre-v2 state files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adapter_claims: Vec<AdapterClaim>,
}

impl Default for InstalledState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            updated_at: now_iso8601(),
            install_mode: InstallMode::User,
            prefix: PathBuf::new(),
            anolisa_version: env!("CARGO_PKG_VERSION").to_string(),
            objects: Vec::new(),
            backups: Vec::new(),
            operations: Vec::new(),
            adapter_claims: Vec::new(),
        }
    }
}

/// Errors raised while loading or persisting [`InstalledState`].
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// Filesystem error while reading or writing state.
    #[error("io error while accessing {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: io::Error,
    },
    /// TOML parse error while loading state.
    #[error("failed to parse installed state at {path}: {source}")]
    Parse {
        /// State path being parsed.
        path: PathBuf,
        /// TOML parser error.
        #[source]
        source: toml::de::Error,
    },
    /// TOML serialization error while saving state.
    #[error("failed to serialize installed state: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl InstalledState {
    /// Load state from `path`. Returns a fresh default if the file does
    /// not exist (first-run case).
    pub fn load(path: &Path) -> Result<Self, StateError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path).map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&content).map_err(|source| StateError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Atomically write state to `path` (unique `tmp` sibling + `rename`).
    /// Refreshes `updated_at` to the current UTC time before serialising.
    ///
    /// Security-critical: the tmp sibling is opened with `O_CREAT|O_EXCL`
    /// (plus `O_NOFOLLOW` on Unix) so a pre-placed symlink at the tmp
    /// path fails the open instead of letting us write through it to a
    /// path outside the state directory. The tmp name itself is salted
    /// with the writer's pid, a process-wide monotonic counter and a
    /// nanosecond timestamp so two concurrent saves cannot collide on
    /// the same path. Mirrors `transaction::write_atomic`.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        // Keep save() non-mutating for callers while refreshing persisted
        // updated_at and schema_version. Installed state is small, so this
        // clone is acceptable.
        //
        // Legacy objects deliberately keep `ownership = None` rather than
        // being back-filled from `effective_ownership()`: that result is a
        // guess derived from filesystem-side fields, and persisting it would
        // make a wrong guess permanent and indistinguishable from a verified
        // value. Authoritative ownership is written only when known — by
        // install (raw-managed) or by adopt/repair after an rpmdb query.
        // Re-running the heuristic on load is a few field comparisons, not I/O.
        let mut snapshot = self.clone();
        snapshot.schema_version = STATE_SCHEMA_VERSION;
        snapshot.updated_at = now_iso8601();

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|source| StateError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let content = toml::to_string_pretty(&snapshot)?;

        write_atomic(path, content.as_bytes()).map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Insert or replace an object, deduped by `(kind, name)`.
    pub fn upsert_object(&mut self, obj: InstalledObject) {
        if let Some(slot) = self
            .objects
            .iter_mut()
            .find(|o| o.kind == obj.kind && o.name == obj.name)
        {
            *slot = obj;
        } else {
            self.objects.push(obj);
        }
    }

    /// Remove an object by `(kind, name)`, returning the removed value.
    pub fn remove_object(&mut self, kind: ObjectKind, name: &str) -> Option<InstalledObject> {
        let idx = self
            .objects
            .iter()
            .position(|o| o.kind == kind && o.name == name)?;
        Some(self.objects.remove(idx))
    }

    /// Find an object by `(kind, name)`.
    pub fn find_object(&self, kind: ObjectKind, name: &str) -> Option<&InstalledObject> {
        self.objects
            .iter()
            .find(|o| o.kind == kind && o.name == name)
    }

    /// Mutable variant of [`Self::find_object`].
    pub fn find_object_mut(
        &mut self,
        kind: ObjectKind,
        name: &str,
    ) -> Option<&mut InstalledObject> {
        self.objects
            .iter_mut()
            .find(|o| o.kind == kind && o.name == name)
    }

    /// Drop legacy `kind = "capability"` objects left by releases that
    /// predate the capability concept's removal, returning the pruned
    /// names so callers can audit the migration in the central log.
    ///
    /// Only state-writing paths (install / uninstall) may call this:
    /// read-only commands must not rewrite `installed.toml`.
    pub fn prune_legacy_capabilities(&mut self) -> Vec<String> {
        let mut pruned = Vec::new();
        self.objects.retain(|obj| {
            if obj.kind == ObjectKind::Capability {
                pruned.push(obj.name.clone());
                false
            } else {
                true
            }
        });
        pruned
    }

    /// Append a backup record.
    pub fn append_backup(&mut self, b: BackupRecord) {
        self.backups.push(b);
    }

    /// Append an operation record.
    pub fn append_operation(&mut self, op: OperationRecord) {
        self.operations.push(op);
    }

    /// Find an adapter receipt by `(component, framework)`.
    pub fn find_adapter_claim(&self, component: &str, framework: &str) -> Option<&AdapterClaim> {
        self.adapter_claims
            .iter()
            .find(|c| c.component == component && c.framework == framework)
    }

    /// Insert or replace an adapter receipt, deduped by
    /// `(component, framework)`.
    pub fn upsert_adapter_claim(&mut self, claim: AdapterClaim) {
        if let Some(slot) = self
            .adapter_claims
            .iter_mut()
            .find(|c| c.component == claim.component && c.framework == claim.framework)
        {
            *slot = claim;
        } else {
            self.adapter_claims.push(claim);
        }
    }

    /// Remove an adapter receipt by `(component, framework)`, returning the
    /// removed value.
    pub fn remove_adapter_claim(
        &mut self,
        component: &str,
        framework: &str,
    ) -> Option<AdapterClaim> {
        let idx = self
            .adapter_claims
            .iter()
            .position(|c| c.component == component && c.framework == framework)?;
        Some(self.adapter_claims.remove(idx))
    }

    /// All adapter receipts for a component, across frameworks.
    pub fn adapter_claims_for_component(&self, component: &str) -> Vec<&AdapterClaim> {
        self.adapter_claims
            .iter()
            .filter(|c| c.component == component)
            .collect()
    }
}

fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Monotonic, process-wide counter mixed into [`tmp_path_for`] so that
/// concurrent writers on the same `path` don't pick the same tmp name.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique tmp sibling path for `path`.
///
/// Pattern: `.{file_name}.{pid}.{counter}.{nanos}.tmp`. Combined with
/// `O_CREAT|O_EXCL` in [`open_excl_nofollow`], a stale tmp (or a hostile
/// plant) at the *exact* generated path is a hard error, not a silent
/// overwrite. Mirrors the pattern in `transaction::tmp_path_for`.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "installed.toml".to_string());
    let pid = std::process::id();
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    tmp.set_file_name(format!(".{file_name}.{pid}.{counter}.{nanos}.tmp"));
    tmp
}

/// Open `tmp` for writing with `O_CREAT|O_EXCL` (+ `O_NOFOLLOW` on Unix).
/// Mirrors `transaction::open_excl_nofollow`.
fn open_excl_nofollow(tmp: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(nix::libc::O_NOFOLLOW);
    }
    opts.open(tmp)
}

/// `tmp` + `rename` write so a crash mid-write cannot leave a truncated
/// file. Mirrors `transaction::write_atomic`.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path_for(path);
    let mut f = open_excl_nofollow(&tmp)?;
    if let Err(err) = f.write_all(bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    let _ = f.sync_all();
    drop(f);
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_object(kind: ObjectKind, name: &str, version: &str) -> InstalledObject {
        InstalledObject {
            kind,
            name: name.to_string(),
            version: version.to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: Some("sha256:abc".to_string()),
            distribution_source: Some("builtin".to_string()),
            install_backend: Some("raw".to_string()),
            ownership: Some(Ownership::RawManaged),
            rpm_metadata: None,
            installed_at: now_iso8601(),
            last_operation_id: Some("op-1".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: SubscriptionScope::None,
            enabled_features: vec!["alpha".to_string()],
            component_refs: vec!["agentsight".to_string()],
            files: vec![OwnedFile {
                path: PathBuf::from("/tmp/anolisa/bin/foo"),
                owner: FileOwner::Anolisa,
                sha256: Some("deadbeef".to_string()),
            }],
            external_modified_files: Vec::new(),
            services: vec![ServiceRef {
                name: "foo.service".to_string(),
                manager: "systemd".to_string(),
                restartable: true,
                enabled: true,
            }],
            health: vec![HealthEntry {
                name: "binary".to_string(),
                status: "ok".to_string(),
                checked_at: now_iso8601(),
                reason: None,
            }],
        }
    }

    fn sample_backup(id: &str, op: &str) -> BackupRecord {
        BackupRecord {
            id: id.to_string(),
            operation_id: op.to_string(),
            original_path: PathBuf::from("/etc/openclaw/config.toml"),
            backup_path: PathBuf::from("/var/lib/anolisa/backups/op-1/openclaw/config.toml"),
            restore_strategy: "replace-file".to_string(),
        }
    }

    fn sample_operation(id: &str) -> OperationRecord {
        OperationRecord {
            id: id.to_string(),
            command: "enable agent-observability".to_string(),
            status: "ok".to_string(),
            started_at: now_iso8601(),
            finished_at: Some(now_iso8601()),
        }
    }

    #[test]
    fn default_state_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let state = InstalledState::default();
        state.save(&path).expect("save default");

        let loaded = InstalledState::load(&path).expect("load default");
        assert_eq!(loaded.schema_version, STATE_SCHEMA_VERSION);
        assert_eq!(loaded.install_mode, InstallMode::User);
        assert_eq!(loaded.anolisa_version, env!("CARGO_PKG_VERSION"));
        assert!(loaded.objects.is_empty());
        assert!(loaded.backups.is_empty());
        assert!(loaded.operations.is_empty());
    }

    #[test]
    fn parse_template_round_trip() {
        let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("templates")
            .join("installed-state.toml");
        let content = fs::read_to_string(&template_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", template_path.display()));
        let state: InstalledState =
            toml::from_str(&content).expect("template parses into InstalledState");

        assert_eq!(state.schema_version, 1);
        assert_eq!(state.install_mode, InstallMode::User);
        assert!(!state.objects.is_empty(), "expected at least one object");
        assert!(!state.backups.is_empty(), "expected at least one backup");
        assert!(
            !state.operations.is_empty(),
            "expected at least one operation"
        );

        let comp = state
            .objects
            .iter()
            .find(|o| o.kind == ObjectKind::Component)
            .expect("template has component object");
        assert_eq!(comp.name, "agentsight");
        assert!(!comp.external_modified_files.is_empty());
        assert_eq!(
            comp.external_modified_files[0].backup_id,
            state.backups[0].id
        );
    }

    /// State files written before the capability concept was removed still
    /// carry `kind = "capability"` objects; loading must not reject them.
    #[test]
    fn legacy_capability_object_still_deserializes() {
        let toml_text = r#"
            schema_version = 1
            updated_at = "2026-06-01T10:00:00Z"
            install_mode = "user"
            prefix = "~/.local"
            anolisa_version = "0.1.0"

            [[objects]]
            kind = "capability"
            name = "agent-observability"
            version = "0.1.0"
            status = "installed"
            installed_at = "2026-06-01T10:00:00Z"
        "#;
        let state: InstalledState = toml::from_str(toml_text).expect("legacy state parses");
        assert_eq!(state.objects[0].kind, ObjectKind::Capability);
    }

    #[test]
    fn prune_legacy_capabilities_drops_only_capability_objects() {
        let mut state = InstalledState::default();
        state.upsert_object(sample_object(
            ObjectKind::Capability,
            "agent-observability",
            "0.1.0",
        ));
        state.upsert_object(sample_object(ObjectKind::Component, "agentsight", "0.2.0"));

        let pruned = state.prune_legacy_capabilities();

        assert_eq!(pruned, vec!["agent-observability".to_string()]);
        assert_eq!(state.objects.len(), 1);
        assert_eq!(state.objects[0].kind, ObjectKind::Component);

        // Idempotent on a clean state.
        assert!(state.prune_legacy_capabilities().is_empty());
    }

    #[test]
    fn upsert_then_find_object() {
        let mut state = InstalledState::default();
        let first = sample_object(ObjectKind::Component, "agentsight", "0.1.0");
        state.upsert_object(first);

        let found = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("present after upsert");
        assert_eq!(found.version, "0.1.0");

        let second = sample_object(ObjectKind::Component, "agentsight", "0.2.0");
        state.upsert_object(second);
        assert_eq!(state.objects.len(), 1, "upsert dedupes by (kind, name)");
        assert_eq!(
            state
                .find_object(ObjectKind::Component, "agentsight")
                .expect("present")
                .version,
            "0.2.0"
        );
    }

    #[test]
    fn remove_object_returns_removed() {
        let mut state = InstalledState::default();
        state.upsert_object(sample_object(ObjectKind::Component, "agentsight", "0.1.0"));

        let removed = state.remove_object(ObjectKind::Component, "agentsight");
        assert!(removed.is_some());
        assert_eq!(removed.expect("just checked").name, "agentsight");

        assert!(
            state
                .remove_object(ObjectKind::Component, "agentsight")
                .is_none()
        );
    }

    #[test]
    fn append_backup_and_operation() {
        let mut state = InstalledState::default();
        assert_eq!(state.backups.len(), 0);
        assert_eq!(state.operations.len(), 0);

        state.append_backup(sample_backup("backup-op-1", "op-1"));
        state.append_operation(sample_operation("op-1"));
        state.append_operation(sample_operation("op-2"));

        assert_eq!(state.backups.len(), 1);
        assert_eq!(state.operations.len(), 2);
    }

    #[test]
    fn external_modified_files_links_backup_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let mut state = InstalledState::default();
        let mut obj = sample_object(ObjectKind::Adapter, "openclaw", "0.1.0");
        obj.external_modified_files.push(ExternalModifiedFile {
            path: PathBuf::from("/etc/openclaw/config.toml"),
            owner: FileOwner::External,
            backup_id: "backup-op-1".to_string(),
            sha256_before: Some("before".to_string()),
            sha256_after: Some("after".to_string()),
        });
        state.upsert_object(obj);
        state.append_backup(sample_backup("backup-op-1", "op-1"));
        state.append_operation(sample_operation("op-1"));

        state.save(&path).expect("save");
        let loaded = InstalledState::load(&path).expect("load");

        let adapter = loaded
            .find_object(ObjectKind::Adapter, "openclaw")
            .expect("adapter present");
        assert_eq!(adapter.external_modified_files.len(), 1);
        assert_eq!(
            adapter.external_modified_files[0].backup_id,
            loaded.backups[0].id
        );
    }

    #[test]
    fn tmp_path_for_is_unique_across_calls() {
        let p = Path::new("/var/lib/anolisa/installed.toml");
        let a = tmp_path_for(p);
        let b = tmp_path_for(p);
        let an = a.file_name().expect("a name").to_string_lossy();
        let bn = b.file_name().expect("b name").to_string_lossy();
        assert!(an.starts_with(".installed.toml."));
        assert!(an.ends_with(".tmp"));
        assert_ne!(an, bn, "two tmp paths for the same target must differ");
    }

    #[test]
    fn open_excl_nofollow_refuses_existing_regular_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plant = dir.path().join(".already-here.tmp");
        fs::write(&plant, b"stale").expect("seed stale");
        let err = open_excl_nofollow(&plant).expect_err("must refuse existing");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[cfg(unix)]
    #[test]
    fn open_excl_nofollow_refuses_existing_symlink() {
        // Direct test of the primitive: a symlink planted at the tmp path
        // must error out instead of letting save() write through to the
        // victim outside the state dir.
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let victim = outside.path().join("victim");
        fs::write(&victim, b"do not touch").expect("seed victim");

        let plant = dir.path().join(".target.tmp");
        std::os::unix::fs::symlink(&victim, &plant).expect("plant symlink");

        let err = open_excl_nofollow(&plant).expect_err("must refuse symlink");
        let kind = err.kind();
        assert!(
            kind == io::ErrorKind::AlreadyExists || err.raw_os_error() == Some(nix::libc::ELOOP),
            "expected EEXIST or ELOOP, got {err:?}"
        );
        let bytes = fs::read(&victim).expect("victim still readable");
        assert_eq!(
            bytes, b"do not touch",
            "symlinked tmp must never be written through"
        );
    }

    #[test]
    fn back_to_back_save_calls_both_succeed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let mut state = InstalledState::default();
        state.upsert_object(sample_object(ObjectKind::Component, "agentsight", "0.1.0"));
        state.save(&path).expect("first save");
        state.upsert_object(sample_object(ObjectKind::Component, "tokenless", "0.1.0"));
        state.save(&path).expect("second save");

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "save must not leak tmp siblings: {leftovers:?}"
        );

        let loaded = InstalledState::load(&path).expect("load");
        assert_eq!(loaded.objects.len(), 2);
    }

    #[test]
    fn save_failure_preserves_prior_installed_toml() {
        // If save fails after the file already exists, the prior bytes
        // must remain intact (the rename never executed).
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let mut state = InstalledState::default();
        state.upsert_object(sample_object(ObjectKind::Component, "agentsight", "0.1.0"));
        state.save(&path).expect("seed save");
        let prior = fs::read(&path).expect("read prior");

        // Replace the parent directory with a regular file so create_dir_all
        // and write would both fail.
        let cleanly_isolated = dir.path().join("inner");
        fs::write(&cleanly_isolated, b"blocker").expect("seed blocker");
        let blocked_path = cleanly_isolated.join("installed.toml");

        let mut blocked_state = state.clone();
        blocked_state.upsert_object(sample_object(ObjectKind::Component, "tokenless", "0.1.0"));
        let err = blocked_state.save(&blocked_path).expect_err("must fail");
        match err {
            StateError::Io { .. } => {}
            other => panic!("expected Io, got {other:?}"),
        }

        // Independent valid path is unchanged byte-for-byte.
        let after = fs::read(&path).expect("read after");
        assert_eq!(after, prior, "prior installed.toml must be untouched");
    }

    #[cfg(unix)]
    #[test]
    fn save_replaces_symlinked_target_without_writing_through_to_victim() {
        // If the *final* installed.toml is a symlink to a victim outside the
        // state dir, rename(2) replaces the symlink itself rather than
        // writing through it.
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let victim = outside.path().join("victim");
        fs::write(&victim, b"do not touch").expect("seed victim");

        let path = dir.path().join("installed.toml");
        std::os::unix::fs::symlink(&victim, &path).expect("plant symlink at target");

        let state = InstalledState::default();
        state.save(&path).expect("save over symlink");

        let meta = fs::symlink_metadata(&path).expect("stat target");
        assert!(meta.file_type().is_file(), "target must be regular file");
        let after = fs::read(&victim).expect("read victim");
        assert_eq!(
            after, b"do not touch",
            "rename must replace the symlink, not write through it"
        );
    }

    #[test]
    fn serialize_skips_optional_none() {
        let mut state = InstalledState::default();
        let mut obj = sample_object(ObjectKind::Component, "agentsight", "0.1.0");
        obj.manifest_digest = None;
        obj.distribution_source = None;
        obj.install_backend = None;
        obj.last_operation_id = None;
        obj.ownership = None;
        obj.rpm_metadata = None;
        state.upsert_object(obj);

        let rendered = toml::to_string_pretty(&state).expect("serialize");
        assert!(
            !rendered.contains("manifest_digest"),
            "None manifest_digest must be skipped, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("distribution_source"),
            "None distribution_source must be skipped"
        );
        assert!(
            !rendered.contains("install_backend"),
            "None install_backend must be skipped"
        );
        assert!(
            !rendered.contains("last_operation_id"),
            "None last_operation_id must be skipped"
        );
        assert!(
            !rendered.contains("ownership"),
            "None ownership must be skipped"
        );
        assert!(
            !rendered.contains("rpm_metadata"),
            "None rpm_metadata must be skipped"
        );
    }

    // ── Ownership model tests ───────────────────────────────────────────

    #[test]
    fn ownership_owns_removal() {
        assert!(Ownership::RawManaged.owns_removal());
        assert!(Ownership::RpmManaged.owns_removal());
        assert!(!Ownership::RpmObserved.owns_removal());
    }

    #[test]
    fn ownership_is_rpm() {
        assert!(!Ownership::RawManaged.is_rpm());
        assert!(Ownership::RpmManaged.is_rpm());
        assert!(Ownership::RpmObserved.is_rpm());
    }

    #[test]
    fn effective_ownership_uses_explicit_field() {
        let mut obj = sample_object(ObjectKind::Component, "test", "1.0.0");
        obj.ownership = Some(Ownership::RpmObserved);
        assert_eq!(obj.effective_ownership(), Ownership::RpmObserved);
        assert!(obj.is_rpm_observed());
        assert!(!obj.owns_removal());
    }

    #[test]
    fn effective_ownership_legacy_raw_managed() {
        let mut obj = sample_object(ObjectKind::Component, "test", "1.0.0");
        obj.ownership = None;
        obj.managed = true;
        obj.adopted = false;
        obj.install_backend = Some("raw".to_string());
        assert_eq!(obj.effective_ownership(), Ownership::RawManaged);
        assert!(obj.owns_removal());
    }

    #[test]
    fn effective_ownership_legacy_rpm_managed() {
        let mut obj = sample_object(ObjectKind::Component, "test", "1.0.0");
        obj.ownership = None;
        obj.managed = true;
        obj.adopted = false;
        obj.install_backend = Some("rpm".to_string());
        assert_eq!(obj.effective_ownership(), Ownership::RpmManaged);
        assert!(obj.owns_removal());
    }

    #[test]
    fn effective_ownership_legacy_rpm_observed() {
        let mut obj = sample_object(ObjectKind::Component, "test", "1.0.0");
        obj.ownership = None;
        obj.managed = false;
        obj.adopted = true;
        obj.install_backend = Some("rpm".to_string());
        assert_eq!(obj.effective_ownership(), Ownership::RpmObserved);
        assert!(obj.is_rpm_observed());
        assert!(!obj.owns_removal());
    }

    #[test]
    fn rpm_observed_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let mut state = InstalledState::default();
        let mut obj = sample_object(ObjectKind::Component, "copilot-shell", "1.2.3");
        obj.ownership = Some(Ownership::RpmObserved);
        obj.status = ObjectStatus::Adopted;
        obj.managed = false;
        obj.adopted = true;
        obj.install_backend = Some("rpm".to_string());
        obj.rpm_metadata = Some(RpmMetadata {
            package_name: "anolisa-copilot-shell".to_string(),
            evr: Some("0:1.2.3-1.al8".to_string()),
            arch: Some("x86_64".to_string()),
            source_repo: Some("@System".to_string()),
        });
        obj.files = Vec::new();
        state.upsert_object(obj);

        state.save(&path).expect("save");
        let loaded = InstalledState::load(&path).expect("load");

        let comp = loaded
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("present");
        assert_eq!(comp.ownership, Some(Ownership::RpmObserved));
        assert!(comp.is_rpm_observed());
        assert!(!comp.owns_removal());

        let rpm = comp.rpm_metadata.as_ref().expect("rpm_metadata present");
        assert_eq!(rpm.package_name, "anolisa-copilot-shell");
        assert_eq!(rpm.evr.as_deref(), Some("0:1.2.3-1.al8"));
        assert_eq!(rpm.arch.as_deref(), Some("x86_64"));
        assert_eq!(rpm.source_repo.as_deref(), Some("@System"));
    }

    #[test]
    fn rpm_managed_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let mut state = InstalledState::default();
        let mut obj = sample_object(ObjectKind::Component, "copilot-shell", "1.2.3");
        obj.ownership = Some(Ownership::RpmManaged);
        obj.install_backend = Some("rpm".to_string());
        obj.rpm_metadata = Some(RpmMetadata {
            package_name: "anolisa-copilot-shell".to_string(),
            evr: Some("0:1.2.3-1.al8".to_string()),
            arch: Some("x86_64".to_string()),
            source_repo: Some("anolisa-release".to_string()),
        });
        state.upsert_object(obj);

        state.save(&path).expect("save");
        let loaded = InstalledState::load(&path).expect("load");

        let comp = loaded
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("present");
        assert_eq!(comp.ownership, Some(Ownership::RpmManaged));
        assert!(!comp.is_rpm_observed());
        assert!(comp.owns_removal());
    }

    /// Pre-v3 state files omit `ownership` and `rpm_metadata`; loading
    /// must not reject them (backward compatibility).
    #[test]
    fn pre_v3_state_without_ownership_deserializes() {
        let toml_text = r#"
            schema_version = 2
            updated_at = "2026-06-01T10:00:00Z"
            install_mode = "system"
            prefix = "/"
            anolisa_version = "0.2.0"

            [[objects]]
            kind = "component"
            name = "copilot-shell"
            version = "1.0.0"
            status = "adopted"
            install_backend = "rpm"
            installed_at = "2026-06-01T10:00:00Z"
            managed = false
            adopted = true
        "#;
        let state: InstalledState = toml::from_str(toml_text).expect("pre-v3 state parses");
        let obj = state
            .find_object(ObjectKind::Component, "copilot-shell")
            .expect("present");
        assert_eq!(obj.ownership, None);
        assert_eq!(obj.rpm_metadata, None);
        // Legacy fallback resolves to rpm-observed.
        assert_eq!(obj.effective_ownership(), Ownership::RpmObserved);
        assert!(obj.is_rpm_observed());
    }

    /// Loading an older state file and saving it must stamp the current
    /// `schema_version`, silently upgrading the on-disk version while
    /// preserving the object payload.
    #[test]
    fn save_upgrades_schema_version_from_older_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let v2_text = r#"
            schema_version = 2
            updated_at = "2026-06-01T10:00:00Z"
            install_mode = "system"
            prefix = "/"
            anolisa_version = "0.2.0"

            [[objects]]
            kind = "component"
            name = "copilot-shell"
            version = "1.0.0"
            status = "installed"
            install_backend = "raw"
            installed_at = "2026-06-01T10:00:00Z"
            managed = true
            adopted = false
        "#;
        fs::write(&path, v2_text).expect("seed v2 file");

        let state = InstalledState::load(&path).expect("load v2");
        assert_eq!(state.schema_version, 2, "loaded value reflects the file");

        state.save(&path).expect("save");

        let upgraded = InstalledState::load(&path).expect("reload");
        assert_eq!(
            upgraded.schema_version, STATE_SCHEMA_VERSION,
            "save must stamp the current schema version"
        );
        assert!(
            upgraded
                .find_object(ObjectKind::Component, "copilot-shell")
                .is_some(),
            "object payload survives the upgrade"
        );
    }

    #[test]
    fn ownership_serde_snake_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");

        let mut state = InstalledState::default();
        let mut obj = sample_object(ObjectKind::Component, "test", "1.0.0");
        obj.ownership = Some(Ownership::RpmObserved);
        state.upsert_object(obj);
        state.save(&path).expect("save");

        let content = fs::read_to_string(&path).expect("read");
        assert!(
            content.contains("rpm_observed"),
            "ownership must serialize as snake_case, got:\n{content}"
        );
    }
}
