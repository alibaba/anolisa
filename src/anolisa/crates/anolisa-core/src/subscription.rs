//! Token collection consent management — core business logic
//!
//! Maintains read/write and state-machine transitions for `/etc/anolisa/register.json`.
//!
//! ## State machine
//!
//! ```text
//! INIT (fresh)  ──[register()]──► REGISTERED
//! INIT (fresh)  ──[unregister()]─► UNREGISTERED
//! INIT (fresh)  ──[later()]──────► INIT (later)  ──(30d)──► INIT (fresh)
//! UNREGISTERED  ──[register()]──► REGISTERED
//! REGISTERED    ──[unregister()]─► UNREGISTERED
//! ```
//!
//! **Irreversibility**: once INIT → UNREGISTERED (explicit refusal), the login
//! script will no longer show the interactive prompt.
//! INIT internal fresh ↔ later can cycle repeatedly (each later() resets the timer).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// register.json schema version, starting at 1
pub const REGISTER_JSON_VERSION: u32 = 1;

/// Later expiry threshold: 30 days (in seconds), hard-coded and non-configurable
pub const LATER_EXPIRE_SECS: i64 = 30 * 86_400;

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

// ── Registration source ──────────────────────────────────────────────

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

// ── register.json state field ────────────────────────────────────────

/// Raw enum for the `state` field in register.json (used for serialization)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegisterState {
    Init,
    Unregistered,
    Registered,
}

// ── register.json full structure ─────────────────────────────────────

/// Full structure of `/etc/anolisa/register.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRecord {
    /// Always required
    pub version: u32,
    pub state: RegisterState,
    /// Product type (required for all states except init-fresh)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_type: Option<ProductType>,
    /// INIT-later start time; refreshed to now on each later() call
    #[serde(skip_serializing_if = "Option::is_none")]
    pub later_start_time: Option<DateTime<Utc>>,
    /// Consent timestamp (required when registered; preserved on unregister; absent for init)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_time: Option<DateTime<Utc>>,
    /// Operation source (required for all states except init-fresh)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<RegisterSource>,
    /// Operator username (required for all states except init-fresh)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
}

// ── Logical state (distinguishes fresh / later) ─────────────────────

/// Logical state derived after parsing register.json
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentState {
    /// Fresh INIT — no decision has been made yet
    InitFresh,
    /// User previously chose Later and the 30-day window has not expired
    InitLater { later_start_time: DateTime<Utc> },
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
    /// - Corrupt JSON or permissions != 0644 (Linux) → warn log + `InitFresh`, no panic
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
    pub fn read_record(&self) -> Option<RegisterRecord> {
        let path = &self.register_path;
        if !path.exists() {
            return None;
        }

        // Linux: reject files with unexpected permissions (not 0644)
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(path) {
                let mode = meta.permissions().mode() & 0o777;
                if mode != 0o644 {
                    eprintln!(
                        "[anolisa] warn: {} has unexpected permissions {:o}; treating as INIT",
                        path.display(),
                        mode
                    );
                    return None;
                }
            }
        }

        match fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str::<RegisterRecord>(&content) {
                Ok(rec) => {
                    if rec.version > REGISTER_JSON_VERSION {
                        eprintln!(
                            "[anolisa] warn: {} has version {} (expected <= {}); treating as INIT",
                            path.display(),
                            rec.version,
                            REGISTER_JSON_VERSION
                        );
                        return None;
                    }
                    Some(rec)
                }
                Err(e) => {
                    eprintln!(
                        "[anolisa] warn: failed to parse {}: {}; treating as INIT",
                        path.display(),
                        e
                    );
                    None
                }
            },
            Err(e) => {
                eprintln!("[anolisa] warn: cannot read {}: {}", path.display(), e);
                None
            }
        }
    }

    /// Map a `RegisterRecord` to `ConsentState` (including later-expiry check).
    fn record_to_state(&self, rec: &RegisterRecord) -> ConsentState {
        match rec.state {
            RegisterState::Registered => ConsentState::Registered,
            RegisterState::Unregistered => ConsentState::Unregistered,
            RegisterState::Init => match rec.later_start_time {
                None => ConsentState::InitFresh,
                Some(ts) => {
                    let elapsed = (Utc::now() - ts).num_seconds();
                    // elapsed < 0: clock skew / tampering — treat as expired, show prompt immediately
                    if !(0..LATER_EXPIRE_SECS).contains(&elapsed) {
                        ConsentState::InitFresh
                    } else {
                        ConsentState::InitLater {
                            later_start_time: ts,
                        }
                    }
                }
            },
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

    /// Perform registration: state → `registered`, writes register.json.
    ///
    /// Only allowed when not already registered (idempotent for re-registration
    /// from init/unregistered states). Returns an error if already registered,
    /// serving as a defensive guard against non-CLI callers bypassing validation.
    pub fn do_register(&self, operator: &str) -> Result<(), SubscriptionError> {
        let _lock = self.acquire_lock()?;
        let (current, _) = self.read_state_and_record();
        if current == ConsentState::Registered {
            return Err(SubscriptionError::AlreadyRegistered);
        }

        let product_type = self.detect_product_type();

        let record = RegisterRecord {
            version: REGISTER_JSON_VERSION,
            state: RegisterState::Registered,
            product_type: Some(product_type),
            later_start_time: None,
            registration_time: Some(Utc::now()),
            source: Some(RegisterSource::Cli),
            operator: Some(operator.to_string()),
        };
        self.atomic_write(&record)?;
        Ok(())
    }

    /// Perform unregistration: state → `unregistered`, writes register.json.
    /// Preserves the historical `registration_time` as an audit record.
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

        let product_type = self.detect_product_type();
        let registration_time = existing.as_ref().and_then(|r| r.registration_time);

        let record = RegisterRecord {
            version: REGISTER_JSON_VERSION,
            state: RegisterState::Unregistered,
            product_type: Some(product_type),
            later_start_time: None,
            registration_time,
            source: Some(RegisterSource::Cli),
            operator: Some(operator.to_string()),
        };
        self.atomic_write(&record)?;
        Ok(())
    }

    /// Perform Later: write/refresh `later_start_time` to now, state stays `init`.
    ///
    /// Only allowed in Init state (fresh / later);
    /// calling from Registered / Unregistered returns an error.
    ///
    /// The CLI layer performs the same state check before calling (for friendlier messages);
    /// this serves as a defensive guard against non-CLI callers bypassing validation.
    pub fn do_later(&self, operator: &str) -> Result<(), SubscriptionError> {
        let _lock = self.acquire_lock()?;
        let (current, _) = self.read_state_and_record();
        match current {
            ConsentState::Registered => return Err(SubscriptionError::AlreadyRegistered),
            ConsentState::Unregistered => return Err(SubscriptionError::NotRegistered),
            ConsentState::InitFresh | ConsentState::InitLater { .. } => {}
        }

        let product_type = self.detect_product_type();

        let record = RegisterRecord {
            version: REGISTER_JSON_VERSION,
            state: RegisterState::Init,
            product_type: Some(product_type),
            later_start_time: Some(Utc::now()),
            registration_time: None,
            source: Some(RegisterSource::Cli),
            operator: Some(operator.to_string()),
        };
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
                for line in content.lines() {
                    if let Some(val) = line.strip_prefix("PRODUCT_TYPE=") {
                        return Some(match val.trim() {
                            "ecs" => ProductType::Ecs,
                            "swas" => ProductType::Swas,
                            "eds" => ProductType::Eds,
                            _ => ProductType::Unknown,
                        });
                    }
                }
                None
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
    #[error("already registered. Use 'anolisa subscription status' to check.")]
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
    fn test_register_writes_registered_state() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("admin").unwrap();

        assert_eq!(m.read_state(), ConsentState::Registered);
        let rec = m.read_record().unwrap();
        assert_eq!(rec.state, RegisterState::Registered);
        assert!(rec.registration_time.is_some());
        assert_eq!(rec.source, Some(RegisterSource::Cli));
        assert_eq!(rec.operator, Some("admin".to_string()));
    }

    #[test]
    fn test_unregister_preserves_registration_time() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("admin").unwrap();
        let reg_time = m.read_record().unwrap().registration_time;

        m.do_unregister("admin").unwrap();
        assert_eq!(m.read_state(), ConsentState::Unregistered);
        // Historical registration_time should be preserved
        assert_eq!(m.read_record().unwrap().registration_time, reg_time);
    }

    #[test]
    fn test_later_sets_init_later() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_later("admin").unwrap();
        assert!(matches!(m.read_state(), ConsentState::InitLater { .. }));
    }

    #[test]
    fn test_later_expired_returns_init_fresh() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        // Manually write a later_start_time 31 days in the past
        let old_ts = Utc::now() - chrono::Duration::days(31);
        let rec = RegisterRecord {
            version: 1,
            state: RegisterState::Init,
            product_type: Some(ProductType::Unknown),
            later_start_time: Some(old_ts),
            registration_time: None,
            source: Some(RegisterSource::Cli),
            operator: Some("admin".into()),
        };
        fs::write(
            &m.register_path,
            serde_json::to_string_pretty(&rec).unwrap(),
        )
        .unwrap();
        assert_eq!(m.read_state(), ConsentState::InitFresh);
    }

    #[test]
    fn test_later_future_timestamp_returns_init_fresh() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        // Clock-skew simulation: later_start_time is in the future
        let future_ts = Utc::now() + chrono::Duration::days(1);
        let rec = RegisterRecord {
            version: 1,
            state: RegisterState::Init,
            product_type: Some(ProductType::Unknown),
            later_start_time: Some(future_ts),
            registration_time: None,
            source: Some(RegisterSource::Cli),
            operator: Some("admin".into()),
        };
        fs::write(
            &m.register_path,
            serde_json::to_string_pretty(&rec).unwrap(),
        )
        .unwrap();
        assert_eq!(m.read_state(), ConsentState::InitFresh);
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
    fn test_later_rejects_registered() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("admin").unwrap();
        let err = m.do_later("admin").unwrap_err();
        assert!(matches!(err, SubscriptionError::AlreadyRegistered));
    }

    #[test]
    fn test_later_rejects_unregistered() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("admin").unwrap();
        m.do_unregister("admin").unwrap();
        let err = m.do_later("admin").unwrap_err();
        assert!(matches!(err, SubscriptionError::NotRegistered));
    }

    #[test]
    fn test_re_register_after_unregister() {
        let dir = TempDir::new().unwrap();
        let m = mgr(&dir);
        m.do_register("admin").unwrap();
        m.do_unregister("admin").unwrap();
        m.do_register("admin").unwrap();
        assert_eq!(m.read_state(), ConsentState::Registered);
    }
}
