use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level configuration.
///
/// `deny_unknown_fields` on every struct turns config typos into hard
/// errors at load time. Without it, a misspelt key (`max_read_byes`)
/// silently maps to `default()` and you spend an hour wondering why a
/// limit isn't taking effect.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default)]
    pub global: GlobalConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    /// User identifier; namespace dir name will be `user-<user_id>`.
    #[serde(default = "default_user_id")]
    pub user_id: String,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            user_id: default_user_id(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfig {
    /// Intelligence profile that biases tool selection (P4+ honors this).
    #[serde(default = "default_profile")]
    pub profile: Profile,
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub embedding: crate::embedding::EmbeddingConfig,
    #[serde(default)]
    pub mount: MountConfig,
    #[serde(default)]
    pub audit: AuditConfig,
    #[serde(default)]
    pub cgroup: crate::cgroup::CgroupConfig,
    #[serde(default)]
    pub git: crate::git_repo::GitConfig,
    /// Maximum bytes returned by a single mem_read call. Files exceeding
    /// this cap are rejected with InvalidArgument to prevent multi-GB
    /// blobs from exhausting memory. Default 1 MiB.
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: u64,
    /// Maximum bytes accepted by a single mem_write call. Caps disk and
    /// JSON-RPC buffer growth from a runaway agent. Default 16 MiB.
    #[serde(default = "default_max_write_bytes")]
    pub max_write_bytes: u64,
    /// Maximum bytes accepted by a single mem_append call. Default 4 MiB
    /// — one append should be a chunk, not a blob; use mem_write for that.
    /// Total file size is still unbounded across many appends, which is
    /// intentional for append-style logging.
    #[serde(default = "default_max_append_bytes")]
    pub max_append_bytes: u64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            profile: default_profile(),
            paths: PathsConfig::default(),
            session: SessionConfig::default(),
            index: IndexConfig::default(),
            embedding: crate::embedding::EmbeddingConfig::default(),
            mount: MountConfig::default(),
            audit: AuditConfig::default(),
            cgroup: crate::cgroup::CgroupConfig::default(),
            git: crate::git_repo::GitConfig::default(),
            max_read_bytes: default_max_read_bytes(),
            max_write_bytes: default_max_write_bytes(),
            max_append_bytes: default_max_append_bytes(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditConfig {
    /// When true, mirror audit entries to systemd-journald in addition to
    /// `<mount>/.anolisa/audit.log`. Linux-only; silently a no-op on
    /// other platforms or when journald is unreachable.
    #[serde(default)]
    pub journald: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountConfig {
    /// `auto` (Linux→userns, fallback userland; non-Linux→userland),
    /// `userland`, or `userns`. Override via `MEMORY_MOUNT_STRATEGY`.
    #[serde(default)]
    pub strategy: crate::mount::MountStrategyKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexConfig {
    /// Enable the BM25 index worker. Disable on `expert` profile or when
    /// you don't want the .anolisa/index/ subdirectory.
    #[serde(default = "default_index_enabled")]
    pub enabled: bool,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            enabled: default_index_enabled(),
        }
    }
}

fn default_index_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    /// Base directory for per-process session scratch + log.
    /// Default: `/run/anolisa/sessions` (Linux tmpfs); set
    /// `MEMORY_SESSION_DIR` to override for tests.
    #[serde(default = "default_session_dir")]
    pub base_dir: String,
    /// What to do with the session directory on shutdown.
    #[serde(default)]
    pub end_action: crate::session::EndAction,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            base_dir: default_session_dir(),
            end_action: crate::session::EndAction::default(),
        }
    }
}

fn default_session_dir() -> String {
    "/run/anolisa/sessions".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Weak models: structured API preferred (Tier B).
    Basic,
    /// Strong models (default): file tools preferred.
    Advanced,
    /// Frontier models: file tools only — Tier B is hidden.
    Expert,
}

impl Profile {
    /// Whether the given tool is exposed under this profile. The result
    /// gates BOTH `tools/list` (the tool is hidden) AND `tools/call`
    /// (an explicit invocation is rejected with `METHOD_NOT_FOUND`), so
    /// `expert`-profile clients cannot bypass the filter by hard-coding
    /// a Tier B tool name.
    pub fn tool_visible(&self, tool_name: &str) -> bool {
        // Tier B: structured API. Hidden on `expert`.
        let tier_b = matches!(
            tool_name,
            "memory_search" | "memory_observe" | "memory_get_context"
        );
        if tier_b && *self == Profile::Expert {
            return false;
        }
        true
    }
}

fn default_profile() -> Profile {
    Profile::Advanced
}

fn default_max_read_bytes() -> u64 {
    1_048_576 // 1 MiB
}

fn default_max_write_bytes() -> u64 {
    16 * 1_048_576 // 16 MiB
}

fn default_max_append_bytes() -> u64 {
    4 * 1_048_576 // 4 MiB
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathsConfig {
    /// Base directory under which each namespace lives.
    /// Default: `~/.anolisa/memory`.
    #[serde(default = "default_base_dir")]
    pub base_dir: String,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            base_dir: default_base_dir(),
        }
    }
}

/// Parse an env var as a boolean using the systemd-style truthy /
/// falsy token list. Unknown values fall back to `current` with a
/// `warn!` log — pre-fix any typo silently flipped the flag to `false`.
fn env_bool(name: &str, current: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => match v.trim().to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            other => {
                tracing::warn!(
                    "env {name}={other:?} not a boolean; keeping current value {current}"
                );
                current
            }
        },
        Err(_) => current,
    }
}

/// Read an env var that ought to be a valid `user_id`, validate it, and
/// return `Some` only on success. Invalid values are dropped with a
/// `warn!` log so the caller can fall back to the next source instead of
/// silently using an unsafe value (`USER_ID="../escape"` would otherwise
/// land outside the base dir).
fn read_validated_user_id_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if v.is_empty() => None,
        Ok(v) => match crate::ns::validate_user_id(&v) {
            Ok(()) => Some(v),
            Err(e) => {
                tracing::warn!("env {name}={v:?} rejected ({e}); ignoring");
                None
            }
        },
        Err(_) => None,
    }
}

fn default_user_id() -> String {
    if let Some(v) = read_validated_user_id_env("USER_ID") {
        return v;
    }
    if let Some(v) = read_validated_user_id_env("USER") {
        return v;
    }
    // Fall back to the OS uid syscall — unforgeable and always succeeds.
    nix::unistd::Uid::current().as_raw().to_string()
}

fn default_base_dir() -> String {
    "~/.anolisa/memory".to_string()
}

impl AppConfig {
    pub fn load(config_path: Option<&Path>) -> Result<Self> {
        let path = match config_path {
            Some(p) => p.to_path_buf(),
            None => Self::default_config_path(),
        };

        let mut config = if path.exists() {
            let content = std::fs::read_to_string(&path).context("Failed to read config file")?;
            toml::from_str(&content).context("Failed to parse config TOML")?
        } else {
            Self::default()
        };

        config.apply_env_overrides();
        Ok(config)
    }

    fn default_config_path() -> PathBuf {
        let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join(".anolisa").join("memory.toml")
    }

    fn apply_env_overrides(&mut self) {
        if let Some(user_id) = read_validated_user_id_env("USER_ID") {
            self.global.user_id = user_id;
        }
        if let Ok(base) = std::env::var("MEMORY_BASE_DIR") {
            self.memory.paths.base_dir = base;
        }
        if let Ok(p) = std::env::var("MEMORY_PROFILE") {
            self.memory.profile = match p.to_lowercase().as_str() {
                "basic" => Profile::Basic,
                "advanced" => Profile::Advanced,
                "expert" => Profile::Expert,
                _ => self.memory.profile,
            };
        }
        if let Ok(s) = std::env::var("MEMORY_SESSION_DIR") {
            self.memory.session.base_dir = s;
        }
        if let Ok(e) = std::env::var("MEMORY_SESSION_END") {
            self.memory.session.end_action = match e.to_lowercase().as_str() {
                "discard" => crate::session::EndAction::Discard,
                "keep" => crate::session::EndAction::Keep,
                _ => self.memory.session.end_action,
            };
        }
        self.memory.index.enabled = env_bool("MEMORY_INDEX_ENABLED", self.memory.index.enabled);
        if let Ok(s) = std::env::var("MEMORY_MOUNT_STRATEGY") {
            if let Some(k) = crate::mount::MountStrategyKind::from_str_loose(&s) {
                self.memory.mount.strategy = k;
            }
        }
        self.memory.audit.journald = env_bool("MEMORY_AUDIT_JOURNALD", self.memory.audit.journald);
        // Embedding backend env overrides
        if let Ok(backend) = std::env::var("MEMORY_EMBEDDING_BACKEND") {
            match backend.to_lowercase().as_str() {
                "none" => self.memory.embedding = crate::embedding::EmbeddingConfig::None,
                "openai" => {
                    let api_key = std::env::var("MEMORY_OPENAI_API_KEY")
                        .or_else(|_| std::env::var("OPENAI_API_KEY"))
                        .unwrap_or_default();
                    let model = std::env::var("MEMORY_OPENAI_MODEL")
                        .unwrap_or_else(|_| "text-embedding-3-small".to_string());
                    let base_url = std::env::var("MEMORY_OPENAI_BASE_URL").ok();
                    self.memory.embedding = crate::embedding::EmbeddingConfig::OpenAI {
                        api_key,
                        model,
                        base_url,
                    };
                }
                "ollama" => {
                    let model = std::env::var("MEMORY_OLLAMA_MODEL")
                        .unwrap_or_else(|_| "nomic-embed-text".to_string());
                    let base_url = std::env::var("MEMORY_OLLAMA_BASE_URL")
                        .unwrap_or_else(|_| "http://localhost:11434".to_string());
                    self.memory.embedding =
                        crate::embedding::EmbeddingConfig::Ollama { model, base_url };
                }
                _ => {
                    tracing::warn!("unknown MEMORY_EMBEDDING_BACKEND={backend:?}; keeping config");
                }
            }
        }
        self.memory.cgroup.enabled = env_bool("MEMORY_CGROUP_ENABLED", self.memory.cgroup.enabled);
        if let Ok(v) = std::env::var("MEMORY_CGROUP_MEMORY_MAX") {
            self.memory.cgroup.memory_max = v;
        }
        self.memory.git.enabled = env_bool("MEMORY_GIT_ENABLED", self.memory.git.enabled);
        self.memory.git.auto_commit =
            env_bool("MEMORY_GIT_AUTO_COMMIT", self.memory.git.auto_commit);
        if let Ok(v) = std::env::var("MEMORY_MAX_READ_BYTES") {
            match v.parse::<u64>() {
                Ok(n) => self.memory.max_read_bytes = n,
                Err(e) => tracing::warn!("MEMORY_MAX_READ_BYTES={v:?} not a u64: {e}; ignoring"),
            }
        }
        if let Ok(v) = std::env::var("MEMORY_MAX_WRITE_BYTES") {
            match v.parse::<u64>() {
                Ok(n) => self.memory.max_write_bytes = n,
                Err(e) => tracing::warn!("MEMORY_MAX_WRITE_BYTES={v:?} not a u64: {e}; ignoring"),
            }
        }
        if let Ok(v) = std::env::var("MEMORY_MAX_APPEND_BYTES") {
            match v.parse::<u64>() {
                Ok(n) => self.memory.max_append_bytes = n,
                Err(e) => tracing::warn!("MEMORY_MAX_APPEND_BYTES={v:?} not a u64: {e}; ignoring"),
            }
        }
    }

    /// Resolve `~` and return the absolute base dir.
    pub fn resolved_base_dir(&self) -> PathBuf {
        let expanded = shellexpand::tilde(&self.memory.paths.base_dir);
        PathBuf::from(expanded.as_ref())
    }

    /// Resolve `~` in the session base dir.
    pub fn resolved_session_dir(&self) -> PathBuf {
        let expanded = shellexpand::tilde(&self.memory.session.base_dir);
        PathBuf::from(expanded.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::EmbeddingConfig;

    #[test]
    fn embedding_config_default_is_none() {
        let cfg = AppConfig::default();
        assert!(matches!(cfg.memory.embedding, EmbeddingConfig::None));
    }

    #[test]
    fn embedding_config_parses_openai_from_toml() {
        let toml = r#"
            [memory.embedding]
            backend = "openai"
            api_key = "sk-test"
            model = "text-embedding-3-large"
            "#;
        let cfg: AppConfig = toml::from_str(toml).unwrap();
        match &cfg.memory.embedding {
            EmbeddingConfig::OpenAI {
                api_key,
                model,
                base_url,
            } => {
                assert_eq!(api_key, "sk-test");
                assert_eq!(model, "text-embedding-3-large");
                assert!(base_url.is_none());
            }
            other => panic!("expected OpenAI, got {other:?}"),
        }
    }

    #[test]
    fn embedding_config_parses_ollama_from_toml() {
        let toml = r#"
            [memory.embedding]
            backend = "ollama"
            model = "bge-m3"
            base_url = "http://gpu-box:11434"
            "#;
        let cfg: AppConfig = toml::from_str(toml).unwrap();
        match &cfg.memory.embedding {
            EmbeddingConfig::Ollama { model, base_url } => {
                assert_eq!(model, "bge-m3");
                assert_eq!(base_url, "http://gpu-box:11434");
            }
            other => panic!("expected Ollama, got {other:?}"),
        }
    }

    #[test]
    fn embedding_config_env_override_defaults_to_none() {
        // When MEMORY_EMBEDDING_BACKEND is not set, config stays at default (None).
        let mut cfg = AppConfig::default();
        cfg.apply_env_overrides();
        assert!(matches!(cfg.memory.embedding, EmbeddingConfig::None));
    }
}
