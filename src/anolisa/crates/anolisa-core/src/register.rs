//! Token collection consent management — core business logic
//!
//! Maintains read/write and state-machine transitions for `/etc/anolisa/register.json`.
//!
//! ## State machine
//!
//! ```text
//! INIT (fresh)  ──[register()]──► REGISTERED
//! INIT (fresh)  ──[unregister()]─► UNREGISTERED
//! UNREGISTERED  ──[register()]──► REGISTERED
//! REGISTERED    ──[unregister()]─► UNREGISTERED
//! ```
//!
//! ## Schema v2
//!
//! Starting from schema_version "2", `register.json` carries a `history`
//! array that records every register/unregister transition with operator
//! and timestamp. The `state` field only reflects the *current* state.
//!
//! Backward compat: v1 files (with `version: u32`) are transparently
//! migrated to v2 on first write.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Current register.json schema version
pub const REGISTER_SCHEMA_VERSION: &str = "2";

/// Maximum number of history entries to retain in register.json
const MAX_HISTORY_ENTRIES: usize = 100;

// ── Product type ─────────────────────────────────────────────────────

/// Product type, read from the `PRODUCT_TYPE` field in `/etc/anolisa-release`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProductType {
    /// Alibaba Cloud ECS
    Ecs,
    /// Simple Application Server (SWAS)
    Swas,
    /// Elastic Desktop Service (EDS)
    Eds,
    /// Unknown / self-hosted environment
    Unknown,
}

impl ProductType {
    pub fn display_name(&self) -> &str {
        match self {
            ProductType::Ecs => "ECS",
            ProductType::Swas => "Simple Application Server",
            ProductType::Eds => "Elastic Desktop Service",
            ProductType::Unknown => "Unknown",
        }
    }
}

impl std::fmt::Display for ProductType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ── History tracking (schema v2) ────────────────────────────────────

/// Action recorded in the history array
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryAction {
    Register,
    Unregister,
}

/// A single entry in the `history` array of register.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub action: HistoryAction,
    pub operator: String,
    pub timestamp: DateTime<Utc>,
}

// ── register.json state field ────────────────────────────────────────

/// Raw enum for the `state` field in register.json (used for serialization)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegisterState {
    Init,
    Unregistered,
    Registered,
}

// ── register.json full structure (schema v2) ─────────────────────────

/// Full structure of `/etc/anolisa/register.json` (schema version 2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRecord {
    pub schema_version: String,
    pub state: RegisterState,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    /// Registration source (cli / console). Preserved for status display and
    /// migration from schema v1. Omitted when missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RegisterSource>,
}

impl RegisterRecord {
    /// Append a history entry and truncate to [`MAX_HISTORY_ENTRIES`].
    fn push_history(&mut self, entry: HistoryEntry) {
        self.history.push(entry);
        if self.history.len() > MAX_HISTORY_ENTRIES {
            let excess = self.history.len() - MAX_HISTORY_ENTRIES;
            self.history.drain(0..excess);
        }
    }
}

// ── Legacy v1 structure (for migration) ──────────────────────────────

/// Legacy v1 format of register.json, used only for backward-compatible
/// deserialization and migration to v2.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RegisterRecordV1 {
    version: u32,
    state: RegisterState,
    product_type: Option<ProductType>,
    later_start_time: Option<DateTime<Utc>>,
    registration_time: Option<DateTime<Utc>>,
    source: Option<RegisterSource>,
    operator: Option<String>,
}

impl From<RegisterRecordV1> for RegisterRecord {
    fn from(v1: RegisterRecordV1) -> Self {
        let action = match v1.state {
            RegisterState::Registered => HistoryAction::Register,
            RegisterState::Unregistered => HistoryAction::Unregister,
            RegisterState::Init => {
                // INIT state has no meaningful history entry
                return RegisterRecord {
                    schema_version: REGISTER_SCHEMA_VERSION.to_string(),
                    state: v1.state,
                    history: Vec::new(),
                    source: v1.source,
                };
            }
        };
        let entry = HistoryEntry {
            action,
            operator: v1.operator.unwrap_or_else(|| "unknown".to_string()),
            timestamp: v1.registration_time.unwrap_or_else(Utc::now),
        };
        RegisterRecord {
            schema_version: REGISTER_SCHEMA_VERSION.to_string(),
            state: v1.state,
            history: vec![entry],
            source: v1.source,
        }
    }
}

// ── Registration source (kept for backward compat in migration) ──────

/// Operation source
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterSource {
    Cli,
    Console,
}

impl std::fmt::Display for RegisterSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegisterSource::Cli => write!(f, "cli"),
            RegisterSource::Console => write!(f, "console"),
        }
    }
}

// ── Logical state ─────────────────────────────────────────────────────

/// Logical state derived after parsing register.json
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentState {
    /// INIT — no decision has been made yet
    InitFresh,
    /// Explicitly refused or withdrew registration
    Unregistered,
    /// Consent granted, upload active
    Registered,
}

// ── RegistrationManager ───────────────────────────────────────────────

/// Single entry point for accessing `/etc/anolisa/register.json`.
///
/// Production code uses `RegistrationManager::new()`;
/// unit tests use `RegistrationManager::with_paths()` to inject temporary paths.
pub struct RegistrationManager {
    /// Path to register.json
    pub register_path: PathBuf,
    /// Path to /etc/anolisa-release
    pub release_path: PathBuf,
}

impl Default for RegistrationManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Product type detection ─────────────────────────────────────────

/// Parse the non-empty `PRODUCT_TYPE` value from `/etc/anolisa-release` content.
pub(crate) fn find_product_type_in_release(content: &str) -> Option<&str> {
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("PRODUCT_TYPE=") {
            let pt = val.trim();
            if !pt.is_empty() {
                return Some(pt);
            }
        }
    }
    None
}

impl RegistrationManager {
    /// Construct with production paths
    pub fn new() -> Self {
        Self {
            register_path: PathBuf::from("/etc/anolisa/register.json"),
            release_path: PathBuf::from("/etc/anolisa-release"),
        }
    }

    /// Construct with custom paths (for unit tests only)
    pub fn with_paths(register_path: PathBuf, release_path: PathBuf) -> Self {
        Self {
            register_path,
            release_path,
        }
    }

    // ── Read ─────────────────────────────────────────────────────────

    /// Read and parse register.json, returning the logical `ConsentState`.
    ///
    /// - File missing → `InitFresh`
    /// - Corrupt JSON or permissions not in {0600, 0644} (Linux) → warn log + `InitFresh`, no panic
    pub fn read_state(&self) -> ConsentState {
        self.read_state_and_record().0
    }

    /// Single file read returning both `ConsentState` and raw `RegisterRecord`,
    /// avoiding the double I/O of calling `read_state()` + `read_record()` separately.
    pub fn read_state_and_record(&self) -> (ConsentState, Option<RegisterRecord>) {
        match self.read_record() {
            Some(rec) => {
                let state = self.record_to_state(&rec);
                (state, Some(rec))
            }
            None => (ConsentState::InitFresh, None),
        }
    }

    /// Read the raw `RegisterRecord` (for the `status` command to display detailed fields).
    /// Returns `None` on error; callers treat this as `InitFresh`.
    ///
    /// Supports transparent v1 → v2 migration.
    pub fn read_record(&self) -> Option<RegisterRecord> {
        let path = &self.register_path;
        if !path.exists() {
            return None;
        }

        // Linux: reject files with unexpected permissions.
        // 0600 is legacy; 0644 is the new default so any user can run `status`.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(path) {
                let mode = meta.permissions().mode() & 0o777;
                const VALID_MODES: &[u32] = &[0o600, 0o644];
                if !VALID_MODES.contains(&mode) {
                    eprintln!(
                        "[anolisa] warn: {} has unexpected permissions {:o}; treating as INIT",
                        path.display(),
                        mode
                    );
                    return None;
                }
            }
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[anolisa] warn: cannot read {}: {}", path.display(), e);
                return None;
            }
        };

        // Try v2 first
        if let Ok(rec) = serde_json::from_str::<RegisterRecord>(&content) {
            // Reject future schema versions we don't understand, but accept
            // older versions (v1 is handled separately below; any v2-compatible
            // version should continue to work).
            let current = REGISTER_SCHEMA_VERSION.parse::<u32>().ok();
            let parsed = rec.schema_version.parse::<u32>().ok();
            if parsed > current {
                eprintln!(
                    "[anolisa] warn: {} has schema_version {} (expected <= {}); treating as INIT",
                    path.display(),
                    rec.schema_version,
                    REGISTER_SCHEMA_VERSION
                );
                return None;
            }
            return Some(rec);
        }

        // Try v1 migration
        if let Ok(v1) = serde_json::from_str::<RegisterRecordV1>(&content) {
            let migrated: RegisterRecord = v1.into();
            return Some(migrated);
        }

        eprintln!(
            "[anolisa] warn: failed to parse {}; treating as INIT",
            path.display()
        );
        None
    }

    /// Map a `RegisterRecord` to `ConsentState`.
    fn record_to_state(&self, rec: &RegisterRecord) -> ConsentState {
        match rec.state {
            RegisterState::Registered => ConsentState::Registered,
            RegisterState::Unregistered => ConsentState::Unregistered,
            RegisterState::Init => ConsentState::InitFresh,
        }
    }

    // ── File lock ──────────────────────────────────────────────────────

    /// Acquire an exclusive lock on register.json to prevent TOCTOU from concurrent writes.
    /// The returned `Flock<File>` holds the lock; dropping it releases automatically.
    #[cfg(unix)]
    fn acquire_lock(&self) -> io::Result<nix::fcntl::Flock<File>> {
        let dir = self
            .register_path
            .parent()
            .unwrap_or_else(|| Path::new("/etc/anolisa"));
        fs::create_dir_all(dir)?;
        let lock_path = dir.join(".register.lock");
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)?;
        nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive)
            .map_err(|(_, e)| io::Error::other(e))
    }

    #[cfg(not(unix))]
    fn acquire_lock(&self) -> io::Result<File> {
        let dir = self
            .register_path
            .parent()
            .unwrap_or_else(|| Path::new("/etc/anolisa"));
        fs::create_dir_all(dir)?;
        let lock_path = dir.join(".register.lock");
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)
    }

    // ── State transition operations ──────────────────────────────────

    /// Perform registration: state → `registered`, appends to history,
    /// writes register.json.
    ///
    /// Only allowed when not already registered (idempotent for re-registration
    /// from init/unregistered states). Returns an error if already registered,
    /// serving as a defensive guard against non-CLI callers bypassing validation.
    pub fn do_register(
        &self,
        operator: &str,
        source: RegisterSource,
    ) -> Result<(), SubscriptionError> {
        let _lock = self.acquire_lock()?;
        let (current, existing) = self.read_state_and_record();
        if current == ConsentState::Registered {
            return Err(SubscriptionError::AlreadyRegistered);
        }

        let mut record = existing.unwrap_or_else(|| RegisterRecord {
            schema_version: REGISTER_SCHEMA_VERSION.to_string(),
            state: RegisterState::Init,
            history: Vec::new(),
            source: None,
        });

        record.state = RegisterState::Registered;
        record.source = Some(source);
        record.push_history(HistoryEntry {
            action: HistoryAction::Register,
            operator: operator.to_string(),
            timestamp: Utc::now(),
        });

        self.atomic_write(&record)?;
        Ok(())
    }

    /// Perform unregistration: state → `unregistered`, appends to history,
    /// writes register.json.
    ///
    /// Only allowed when currently registered. Returns an error if already
    /// unregistered or managed by sysom, serving as a defensive guard against
    /// non-CLI callers bypassing validation.
    pub fn do_unregister(&self, operator: &str) -> Result<(), SubscriptionError> {
        let _lock = self.acquire_lock()?;
        // Check sysom inside the lock to prevent TOCTOU races
        if self.is_sysom_registered() {
            return Err(SubscriptionError::SysomManaged);
        }
        let (current, existing) = self.read_state_and_record();
        if current == ConsentState::Unregistered {
            return Err(SubscriptionError::NotRegistered);
        }

        let mut record = existing.unwrap_or_else(|| RegisterRecord {
            schema_version: REGISTER_SCHEMA_VERSION.to_string(),
            state: RegisterState::Init,
            history: Vec::new(),
            source: None,
        });

        record.state = RegisterState::Unregistered;
        record.push_history(HistoryEntry {
            action: HistoryAction::Unregister,
            operator: operator.to_string(),
            timestamp: Utc::now(),
        });

        self.atomic_write(&record)?;
        Ok(())
    }

    // ── Atomic write ────────────────────────────────────────────────────

    /// Atomically write register.json:
    /// 1. Serialize to `.register.json.tmp.<pid>`
    /// 2. `fsync` to ensure data hits disk
    /// 3. `rename` to atomically replace the target file
    fn atomic_write(&self, record: &RegisterRecord) -> io::Result<()> {
        let dir = self
            .register_path
            .parent()
            .unwrap_or_else(|| Path::new("/etc/anolisa"));
        fs::create_dir_all(dir)?;

        let tmp_path = dir.join(format!(".register.json.tmp.{}", std::process::id()));

        let result = self.atomic_write_inner(&tmp_path, record);
        if result.is_err() {
            let _ = fs::remove_file(&tmp_path);
        }
        result
    }

    fn atomic_write_inner(&self, tmp_path: &Path, record: &RegisterRecord) -> io::Result<()> {
        let content =
            serde_json::to_string_pretty(record).map_err(|e| io::Error::other(e.to_string()))?;

        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.flush()?;
            file.sync_all()?; // fsync
        }

        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            // 0644 so any user can read the file (e.g. `anolisa register status`).
            fs::set_permissions(tmp_path, fs::Permissions::from_mode(0o644))?;
        }

        fs::rename(tmp_path, &self.register_path)?;
        Ok(())
    }

    // ── Product type detection ─────────────────────────────────────────

    /// Read `PRODUCT_TYPE` from `/etc/anolisa-release`, falling back to `Unknown`.
    pub fn detect_product_type(&self) -> ProductType {
        fs::read_to_string(&self.release_path)
            .ok()
            .and_then(|content| {
                find_product_type_in_release(&content).map(|pt| match pt {
                    "ecs" => ProductType::Ecs,
                    "swas" => ProductType::Swas,
                    "eds" => ProductType::Eds,
                    _ => ProductType::Unknown,
                })
            })
            .unwrap_or(ProductType::Unknown)
    }

    /// Detect whether sysom services are active (sysak_meta active).
    /// When both services are running, the system has been registered via the sysom platform.
    pub fn is_sysom_registered(&self) -> bool {
        Self::is_service_running("sysak_meta") && Self::is_service_running("sysak_agentsight")
    }

    /// Detect whether the agentsight service is running.
    pub fn is_agentsight_running(&self) -> bool {
        Self::is_service_running("agentsight")
    }

    /// Check if a service is running (compatible with multiple init systems).
    ///
    /// Detection strategy:
    /// 1. If systemd is available, use `systemctl is-active --quiet <unit>`
    /// 2. Otherwise fall back to `service <name> status` (SysVinit / OpenRC compatible)
    fn is_service_running(unit: &str) -> bool {
        if Self::has_systemd() {
            return std::process::Command::new("systemctl")
                .args(["is-active", "--quiet", unit])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        }
        // fallback: `service <name> status` returns exit 0 when running
        std::process::Command::new("service")
            .args([unit, "status"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Detect whether the current system runs systemd (via presence of /run/systemd/system).
    fn has_systemd() -> bool {
        Path::new("/run/systemd/system").is_dir()
    }
}

// ── Permissions and operator ─────────────────────────────────────────

/// Check if the current process is running as root.
/// On non-Linux platforms (dev macOS), compiles but does not enforce.
pub fn require_root() -> Result<(), SubscriptionError> {
    #[cfg(unix)]
    {
        if nix::unistd::geteuid().is_root() {
            Ok(())
        } else {
            Err(SubscriptionError::NotRoot)
        }
    }
    #[cfg(not(unix))]
    Ok(())
}

/// Get the current operator username; prefers `$SUDO_USER`, then `$USER` / `$LOGNAME`.
pub fn current_operator() -> String {
    std::env::var("SUDO_USER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

// ── Error types ───────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum SubscriptionError {
    #[error("this command requires root or sudo privileges")]
    NotRoot,
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("already registered. Use 'anolisa register status' to check.")]
    AlreadyRegistered,
    #[error("not currently registered.")]
    NotRegistered,
    #[error("Please operate from the OS console.")]
    SysomManaged,
}

// ── Unit tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn mgr(dir: &TempDir) -> RegistrationManager {
        RegistrationManager::with_paths(
            dir.path().join("register.json"),
            dir.path().join("anolisa-release"),
        )
    }

    /// Write register.json content and set production-like 0o644 permissions.
    ///
    /// `read_record()` accepts files with mode 0o600 (legacy) or 0o644 on Linux,
    /// so tests that exercise the read path must match the production layout.
    fn write_register_file(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    #[test]
    fn test_missing_file_is_init_fresh() {
        let dir = TempDir::new().unwrap();
        assert_eq!(mgr(&dir).read_state(), ConsentState::InitFresh);
    }

    #[test]
    fn test_corrupt_json_returns_init_fresh() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        fs::write(&m.register_path, "not valid json {{").unwrap();
        assert_eq!(m.read_state(), ConsentState::InitFresh);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_register_writes_0644_permissions() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("admin", RegisterSource::Cli).unwrap();

        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(&m.register_path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o644);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_legacy_0600_permission_accepted() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);

        let v2_json = r#"{"schema_version":"2","state":"registered","history":[]}"#;
        fs::write(&m.register_path, v2_json).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&m.register_path, fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(m.read_state(), ConsentState::Registered);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_unexpected_permission_rejected() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);

        let v2_json = r#"{"schema_version":"2","state":"registered","history":[]}"#;
        fs::write(&m.register_path, v2_json).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&m.register_path, fs::Permissions::from_mode(0o777)).unwrap();

        assert_eq!(m.read_state(), ConsentState::InitFresh);
    }

    #[test]
    fn test_register_writes_registered_state() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("admin", RegisterSource::Cli).unwrap();

        assert_eq!(m.read_state(), ConsentState::Registered);
        let rec = m.read_record().unwrap();
        assert_eq!(rec.state, RegisterState::Registered);
        assert_eq!(rec.schema_version, "2");
        assert_eq!(rec.history.len(), 1);
        assert_eq!(rec.history[0].action, HistoryAction::Register);
        assert_eq!(rec.history[0].operator, "admin");
    }

    #[test]
    fn test_unregister_appends_history() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("admin", RegisterSource::Cli).unwrap();

        m.do_unregister("bob").unwrap();
        assert_eq!(m.read_state(), ConsentState::Unregistered);

        let rec = m.read_record().unwrap();
        assert_eq!(rec.history.len(), 2);
        assert_eq!(rec.history[0].action, HistoryAction::Register);
        assert_eq!(rec.history[1].action, HistoryAction::Unregister);
        assert_eq!(rec.history[1].operator, "bob");
    }

    #[test]
    fn test_history_preserved_across_register_cycles() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("alice", RegisterSource::Cli).unwrap();
        m.do_unregister("alice").unwrap();
        m.do_register("carol", RegisterSource::Cli).unwrap();

        let rec = m.read_record().unwrap();
        assert_eq!(rec.history.len(), 3);
        assert_eq!(rec.history[0].action, HistoryAction::Register);
        assert_eq!(rec.history[0].operator, "alice");
        assert_eq!(rec.history[1].action, HistoryAction::Unregister);
        assert_eq!(rec.history[2].action, HistoryAction::Register);
        assert_eq!(rec.history[2].operator, "carol");
    }

    #[test]
    fn test_product_type_from_release_file() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        fs::write(&m.release_path, "PRODUCT_TYPE=ecs\n").unwrap();
        assert_eq!(m.detect_product_type(), ProductType::Ecs);

        fs::write(&m.release_path, "PRODUCT_TYPE=swas\n").unwrap();
        assert_eq!(m.detect_product_type(), ProductType::Swas);
    }

    #[test]
    fn test_product_type_fallback_unknown() {
        let dir = TempDir::new().unwrap();
        assert_eq!(mgr(&dir).detect_product_type(), ProductType::Unknown);
    }

    #[test]
    fn test_re_register_after_unregister() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("admin", RegisterSource::Cli).unwrap();
        m.do_unregister("admin").unwrap();
        m.do_register("admin", RegisterSource::Cli).unwrap();
        assert_eq!(m.read_state(), ConsentState::Registered);
    }

    #[test]
    fn test_v1_migration_registered() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);

        // Write a v1-format file
        let v1_json = r#"{"version":1,"state":"registered","product_type":"ecs","registration_time":"2026-01-10T09:00:00Z","source":"cli","operator":"alice"}"#;
        write_register_file(&m.register_path, v1_json);

        let rec = m.read_record().unwrap();
        assert_eq!(rec.schema_version, "2");
        assert_eq!(rec.state, RegisterState::Registered);
        assert_eq!(rec.history.len(), 1);
        assert_eq!(rec.history[0].action, HistoryAction::Register);
        assert_eq!(rec.history[0].operator, "alice");
        assert_eq!(
            rec.history[0].timestamp,
            DateTime::parse_from_rfc3339("2026-01-10T09:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn test_v1_migration_unregistered() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);

        let v1_json = r#"{"version":1,"state":"unregistered","operator":"bob"}"#;
        write_register_file(&m.register_path, v1_json);

        let rec = m.read_record().unwrap();
        assert_eq!(rec.schema_version, "2");
        assert_eq!(rec.state, RegisterState::Unregistered);
        assert_eq!(rec.history.len(), 1);
        assert_eq!(rec.history[0].action, HistoryAction::Unregister);
        assert_eq!(rec.history[0].operator, "bob");
    }

    #[test]
    fn test_v1_migration_init_no_history() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);

        let v1_json = r#"{"version":1,"state":"init"}"#;
        write_register_file(&m.register_path, v1_json);

        let rec = m.read_record().unwrap();
        assert_eq!(rec.schema_version, "2");
        assert_eq!(rec.state, RegisterState::Init);
        assert!(rec.history.is_empty());
    }
}
