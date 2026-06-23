//! Framework driver trait and the value types it exchanges with the
//! [`AdapterManager`](super::manager).
//!
//! A driver understands one framework's enable/disable/status semantics.
//! It never spawns processes or deletes paths directly: dangerous IO goes
//! through the controlled [`AdapterOps`] handle the Manager injects via
//! [`DriverCtx`], so timeout/truncation/logging and path-boundary policy
//! stay centralized. The split is deliberate — **driver owns framework
//! semantics, Manager owns dangerous-resource boundaries**.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use anolisa_platform::fs_layout::FsLayout;

use super::AdapterError;
use super::claim::AdapterClaim;

/// Read-only host facts a driver may inspect during [`FrameworkDriver::detect`].
#[derive(Debug, Clone, Default)]
pub struct HostEnv {
    /// Current caller's home directory, when resolvable. Frameworks whose
    /// state lives under `$HOME` use this to locate it.
    pub user_home: Option<PathBuf>,
}

/// A skill declared in the component manifest, with an optional resolved
/// source path for the skill bundle.
#[derive(Debug, Clone)]
pub struct DeclaredSkill {
    /// Skill name.
    pub name: String,
    /// Resolved source directory. When `None`, the driver falls back to
    /// `<resource_root>/skills/<name>/`.
    pub source: Option<PathBuf>,
}

/// Detection outcome for a framework.
#[derive(Debug, Clone, Serialize)]
pub struct DetectResult {
    /// Whether the framework appears usable on this host.
    pub detected: bool,
    /// Human-readable explanation (binary path found, home dir present, …).
    pub reason: String,
}

/// Everything a driver needs for one operation, constructed by the
/// Manager. The driver never locates state/log/lock paths itself.
pub struct DriverCtx<'a> {
    /// Component being enabled/disabled/queried.
    pub component: String,
    /// Framework name (matches the driver's [`FrameworkDriver::name`]).
    pub framework: String,
    /// Resolved filesystem layout for the active install mode.
    pub layout: &'a FsLayout,
    /// Resource directory under `{datadir}/adapters/<component>/<framework>/`.
    pub resource_root: PathBuf,
    /// Caller's home directory, when resolvable.
    pub user_home: Option<PathBuf>,
    /// Plugin id declared in the component's adapter manifest, if any.
    /// A driver may fall back to it when the bundle does not name one.
    pub declared_plugin_id: Option<String>,
    /// Skills declared in the component's adapter manifest. Each entry
    /// carries an optional resolved `source` path; when absent, the driver
    /// falls back to `<resource_root>/skills/<name>/`.
    pub declared_skills: Vec<DeclaredSkill>,
    /// Post-install config key/value pairs declared in the component's
    /// adapter manifest. The driver applies these via the framework CLI.
    pub declared_config: Vec<crate::manifest::AdapterConfigSetSpec>,
    /// Bundle entry-point filename from the manifest (framework-specific
    /// section preferred over generic `[adapters.bundle]`). The driver
    /// should use this to locate the framework-native manifest inside
    /// the resource root instead of hardcoding a filename.
    pub declared_bundle_entry: Option<String>,
    /// True when the caller passed `--dry-run`; drivers must not mutate
    /// framework state in this mode (the Manager also guards this).
    pub dry_run: bool,
    /// Controlled IO helpers. The only sanctioned way to run a framework
    /// CLI.
    pub ops: &'a dyn AdapterOps,
}

/// Parsed framework-native bundle from the resource directory.
#[derive(Debug, Clone)]
pub struct AdapterBundle {
    /// Resource root the bundle was read from.
    pub resource_root: PathBuf,
    /// Digest of the resource tree, for drift/upgrade detection. `None`
    /// when the driver declined to compute one.
    pub digest: Option<String>,
    /// Framework-native plugin id resolved from the bundle (or the
    /// manifest-declared fallback).
    pub plugin_id: Option<String>,
}

/// What [`FrameworkDriver::apply_enable`] would do, for `--dry-run` and
/// human confirmation. Carries no executable data.
#[derive(Debug, Clone, Serialize)]
pub struct DriverPlan {
    /// Framework name.
    pub framework: String,
    /// Component name.
    pub component: String,
    /// Ordered human-readable descriptions of the enable steps.
    pub actions: Vec<String>,
    /// Display form of the framework CLI registration command, when one is
    /// run. Display-only — never parsed back into an argv.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub register_command: Option<String>,
}

/// Outcome of [`FrameworkDriver::disable`].
#[derive(Debug, Clone, Serialize)]
pub struct DisableReport {
    /// True when every claimed resource was successfully released. When
    /// false, the Manager keeps the receipt and marks it `cleanup_failed`.
    pub cleanup_complete: bool,
    /// Human-readable notes (e.g. "openclaw CLI not found; assuming
    /// registry absent").
    pub messages: Vec<String>,
}

// ---------------------------------------------------------------------------
// Status report
// ---------------------------------------------------------------------------

/// Full status of one receipt: a human summary plus machine-readable
/// conditions. JSON output must keep `conditions` so distinct failures are
/// not flattened into one label.
#[derive(Debug, Clone, Serialize)]
pub struct AdapterStatusReport {
    /// One-line health summary.
    pub summary: AdapterSummary,
    /// Individual signals behind the summary.
    pub conditions: Vec<AdapterCondition>,
}

/// Human-facing one-line adapter health summary.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterSummary {
    /// Every critical signal is healthy.
    Healthy,
    /// At least one critical signal is missing or has drifted.
    Degraded,
    /// A prior cleanup did not complete.
    CleanupFailed,
    /// State cannot currently be verified reliably.
    Unknown,
}

/// One machine-readable signal contributing to a status summary.
#[derive(Debug, Clone, Serialize)]
pub struct AdapterCondition {
    /// Which signal this is.
    pub kind: AdapterConditionKind,
    /// Tri-state result.
    pub status: ConditionStatus,
    /// Optional human explanation (e.g. "plugin not in `plugins list`").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional pointer to the claim resource this condition is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<ClaimResourceRef>,
}

/// Reference to a [`ClaimResource`](super::claim::ClaimResource) by id.
#[derive(Debug, Clone, Serialize)]
pub struct ClaimResourceRef {
    /// Resource id.
    pub id: String,
}

/// The kinds of signal a condition can report.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterConditionKind {
    /// The framework itself is detectable on the host.
    FrameworkDetected,
    /// The installed resource bundle still matches the enable-time digest.
    ResourceBundleMatches,
    /// The plugin is still present in the framework registry.
    PluginRegistered,
    /// A marketplace source is still registered (future drivers).
    MarketplaceRegistered,
    /// A claimed directory tree still exists unmodified (future drivers).
    TreePresent,
    /// Injected JSON keys still equal ANOLISA's last-applied values
    /// (future drivers).
    JsonKeysPresent,
    /// A claimed symlink still points at the expected target (future
    /// drivers).
    SymlinkPresent,
    /// A prior cleanup completed.
    CleanupComplete,
    /// Whether this driver supports reliable read-only verification at
    /// all. `False` means the related conditions are `Unknown` rather than
    /// faked healthy.
    VerificationSupported,
}

/// Tri-state condition result. `Unknown` is distinct from `False`: it
/// means "could not verify", never "verified absent".
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConditionStatus {
    /// Signal verified present/healthy.
    True,
    /// Signal verified absent/unhealthy.
    False,
    /// Signal could not be verified.
    Unknown,
}

// ---------------------------------------------------------------------------
// Controlled IO helpers
// ---------------------------------------------------------------------------

/// A framework CLI invocation, built by a driver from a static command
/// template plus validated arguments. Executed by [`AdapterOps`] as an
/// argv array — never through a shell.
#[derive(Debug, Clone)]
pub struct FrameworkCommand {
    /// Executable to run (resolved via `PATH` with [`Self::path_prepend`]).
    pub program: String,
    /// Argument vector. Each element is passed verbatim; no shell parsing.
    pub args: Vec<String>,
    /// Environment variables to set on the child.
    pub env_set: Vec<(String, String)>,
    /// Environment variables to remove from the child.
    pub env_remove: Vec<String>,
    /// Directories prepended to `PATH` before spawning.
    pub path_prepend: Vec<PathBuf>,
    /// Hard timeout; the child is killed if it exceeds this.
    pub timeout: Duration,
}

/// Captured output of a [`FrameworkCommand`]. stdout/stderr are truncated
/// to a bounded size before being returned and logged.
#[derive(Debug, Clone)]
pub struct CliOutput {
    /// Exit code, or `None` when the child was killed (e.g. on timeout).
    pub status: Option<i32>,
    /// True when the child was killed because it exceeded the timeout.
    pub timed_out: bool,
    /// Truncated stdout (UTF-8 lossy).
    pub stdout: String,
    /// Truncated stderr (UTF-8 lossy).
    pub stderr: String,
}

impl CliOutput {
    /// True iff the process exited zero and was not killed by timeout.
    pub fn success(&self) -> bool {
        self.status == Some(0) && !self.timed_out
    }
}

/// The closed set of dangerous IO a driver may perform, mediated by the
/// Manager.
///
/// MVP exposes only [`run_framework_cli`](AdapterOps::run_framework_cli);
/// the file/JSON/symlink helpers from the design land with the drivers
/// that need them (Cosh, Qoder, Hermes), so the boundary grows one
/// reviewed method at a time rather than shipping unused surface.
pub trait AdapterOps {
    /// Spawn a framework CLI with a timeout, capture and truncate its
    /// output, and record the invocation in the central log. The argv is
    /// executed directly (no shell), so receipt-derived data cannot inject
    /// extra commands.
    ///
    /// # Errors
    ///
    /// [`AdapterError::FrameworkCli`] when the process cannot be spawned.
    /// A non-zero exit or a timeout is reported through [`CliOutput`], not
    /// as an error, so the driver decides how to interpret it.
    fn run_framework_cli(&self, cmd: FrameworkCommand) -> Result<CliOutput, AdapterError>;

    /// Recursively copy a directory tree from `src` to `dst`. The Manager
    /// validates that `dst` is under an allowed external root before
    /// executing. `src` must be under the resource root or an allowed
    /// root. Creates `dst` and any missing parents.
    ///
    /// # Errors
    ///
    /// [`AdapterError::Io`] on filesystem failure;
    /// [`AdapterError::ClaimValidation`] if `dst` fails boundary check.
    fn copy_tree(&self, src: &Path, dst: &Path) -> Result<(), AdapterError>;

    /// Copy a single file from `src` to `dst`. Both paths are validated
    /// against the allowed roots. Creates parent directories of `dst`.
    ///
    /// # Errors
    ///
    /// [`AdapterError::Io`] on filesystem failure;
    /// [`AdapterError::ClaimValidation`] if either path fails boundary check.
    fn copy_file(&self, src: &Path, dst: &Path) -> Result<(), AdapterError>;

    /// Remove a directory tree rooted at `path`. The Manager validates
    /// that `path` is under an allowed external root before executing.
    /// Returns `Ok(false)` when the path does not exist (idempotent).
    ///
    /// # Errors
    ///
    /// [`AdapterError::Io`] on filesystem failure;
    /// [`AdapterError::ClaimValidation`] if `path` fails boundary check.
    fn remove_tree(&self, path: &Path) -> Result<bool, AdapterError>;
}

// ---------------------------------------------------------------------------
// Driver trait
// ---------------------------------------------------------------------------

/// One framework's adapter semantics. Built-in and closed: new frameworks
/// ship in an ANOLISA release, never as a runtime plugin.
pub trait FrameworkDriver: Send + Sync {
    /// Framework name, e.g. `"openclaw"`.
    fn name(&self) -> &'static str;

    /// Read-only probe for whether the framework is usable. May inspect
    /// `PATH` and the filesystem; must not spawn the framework.
    fn detect(&self, env: &HostEnv) -> DetectResult;

    /// External roots (outside ANOLISA-owned roots) this driver is allowed
    /// to write into. The Manager validates every
    /// [`ClaimResourceKind::ExternalPath`](super::claim::ClaimResourceKind::ExternalPath)
    /// against this set. The result may expand `ctx.user_home`, but must
    /// not be derived from receipt contents (which would let a forged
    /// receipt authorize itself).
    fn allowed_external_roots(&self, ctx: &DriverCtx) -> Vec<PathBuf>;

    /// Parse the framework-native bundle from the resource directory.
    ///
    /// # Errors
    ///
    /// [`AdapterError::BundleInvalid`] when required files are missing or
    /// unreadable.
    fn read_bundle(&self, ctx: &DriverCtx) -> Result<AdapterBundle, AdapterError>;

    /// Describe what [`Self::apply_enable`] would do, for dry-run and
    /// confirmation.
    ///
    /// # Errors
    ///
    /// Propagates bundle/validation errors encountered while planning.
    fn plan_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<DriverPlan, AdapterError>;

    /// Build the pure-data receipt for a future enable operation without
    /// mutating framework state.
    ///
    /// The Manager validates and persists this claim before
    /// [`Self::apply_enable`] runs, so a later framework-side failure stays
    /// visible to status/disable.
    ///
    /// # Errors
    ///
    /// [`AdapterError::BundleInvalid`] when the bundle cannot produce the
    /// framework identifiers needed for a receipt.
    fn prepare_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<AdapterClaim, AdapterError>;

    /// Idempotently apply an already-persisted enable receipt to framework
    /// state.
    ///
    /// # Errors
    ///
    /// [`AdapterError::FrameworkCli`] or [`AdapterError::BundleInvalid`] on
    /// failure.
    fn apply_enable(&self, claim: &AdapterClaim, ctx: &DriverCtx) -> Result<(), AdapterError>;

    /// Read-only status check against a receipt. Must not mutate state.
    ///
    /// # Errors
    ///
    /// [`AdapterError::FrameworkCli`] when a verification probe cannot run
    /// (as opposed to running and finding the plugin absent).
    fn status(
        &self,
        claim: &AdapterClaim,
        ctx: &DriverCtx,
    ) -> Result<AdapterStatusReport, AdapterError>;

    /// Idempotently disable the adapter, removing only what the receipt
    /// declares ANOLISA took over.
    ///
    /// # Errors
    ///
    /// [`AdapterError::FrameworkCli`] when de-registration fails in a way
    /// that is not simply "framework absent".
    fn disable(&self, claim: &AdapterClaim, ctx: &DriverCtx)
    -> Result<DisableReport, AdapterError>;
}

/// Scan candidate directories of `PATH` for an executable named `name`,
/// honoring the executable bit on Unix. Shared by drivers' `detect`.
pub fn find_binary_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}
