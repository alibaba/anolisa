use std::sync::Arc;

use crate::audit::AuditLogger;
use crate::config::AppConfig;
use crate::error::Result;
use crate::index::{IndexHandle, SearchHit};
use crate::mount::pick_strategy;
use crate::ns::{MountPoint, Namespace};
use crate::session::{EndAction, SessionId, SessionLogService};
use crate::tools::{GrepHit, GrepOptions, ListEntry, ListOptions};

/// MemoryService is the top-level entry point used by both the MCP server and
/// the CLI. It owns the namespace mount, audit logger, and (for P3+) a
/// per-process Session Log scratch area, plus (for P4+) a background index,
/// plus (for P6.2+) an optional git versioning handle.
pub struct MemoryService {
    pub mount: MountPoint,
    pub audit: Arc<AuditLogger>,
    pub session: Option<Arc<SessionLogService>>,
    pub index: Option<Arc<IndexHandle>>,
    pub git: Option<Arc<crate::git_repo::GitHandle>>,
    pub config: AppConfig,
    /// Whether the active mount strategy entered a user namespace.
    pub entered_userns: bool,
    pub mount_strategy_name: &'static str,
}

impl MemoryService {
    /// Build the service from configuration.
    /// Always ensures the mount; starts a Session Log if the configured base
    /// directory is writable. Failure to start the session is logged and
    /// degrades gracefully (mem_promote / mem_session_log will return errors).
    pub fn new(config: AppConfig) -> Result<Self> {
        let base = config.resolved_base_dir();
        std::fs::create_dir_all(&base)?;

        // Phase 2: pick mount strategy (may unshare into a user namespace).
        let picked = pick_strategy(config.memory.mount.strategy)?;
        let entered_userns = picked.entered_userns;
        let strategy_name = picked.strategy.name();

        let ns = Namespace::user(&config.global.user_id)?;
        let mount = MountPoint::ensure_with(ns.clone(), &base, picked.strategy.as_ref())?;
        let audit = Arc::new(AuditLogger::new_with_journald(
            mount.audit_log_path(),
            config.memory.audit.journald,
        )?);

        // Start a session if the configured directory is usable.
        let session = match start_session(&config, &ns) {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => {
                tracing::warn!(
                    "session log unavailable ({e}); mem_promote / mem_session_log will return errors"
                );
                None
            }
        };

        // Start the BM25 index worker if enabled.
        let index = if config.memory.index.enabled {
            match IndexHandle::open(&mount) {
                Ok(h) => Some(Arc::new(h)),
                Err(e) => {
                    tracing::warn!(
                        "index unavailable ({e}); memory_search / memory_observe will degrade"
                    );
                    None
                }
            }
        } else {
            None
        };

        // Optional git versioning (P6.2). Best-effort: failure logs and
        // continues with git=None.
        let git = match crate::git_repo::GitHandle::open(config.memory.git.clone(), &mount.root) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("git versioning disabled: {e}");
                None
            }
        };

        Ok(Self {
            mount,
            audit,
            session,
            index,
            git,
            config,
            entered_userns,
            mount_strategy_name: strategy_name,
        })
    }

    // ---- Tier A facade methods ----

    pub fn read(&self, path: &str) -> Result<String> {
        crate::tools::read(self, path)
    }

    pub fn write(&self, path: &str, content: &str, overwrite: bool) -> Result<u64> {
        crate::tools::write(self, path, content, overwrite)
    }

    pub fn edit(&self, path: &str, old_str: &str, new_str: &str) -> Result<()> {
        crate::tools::edit(self, path, old_str, new_str)
    }

    pub fn append(&self, path: &str, content: &str) -> Result<u64> {
        crate::tools::append(self, path, content)
    }

    pub fn list(&self, dir: &str, opts: ListOptions) -> Result<Vec<ListEntry>> {
        crate::tools::list(self, dir, opts)
    }

    pub fn grep(&self, pattern: &str, opts: GrepOptions) -> Result<Vec<GrepHit>> {
        crate::tools::grep(self, pattern, opts)
    }

    pub fn diff(&self, path1: &str, path2: &str) -> Result<String> {
        crate::tools::diff(self, path1, path2)
    }

    pub fn mkdir(&self, path: &str) -> Result<()> {
        crate::tools::mkdir(self, path)
    }

    pub fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        crate::tools::remove(self, path, recursive)
    }

    pub fn promote(&self, session_path: &str, store_path: &str) -> Result<u64> {
        crate::tools::promote(self, session_path, store_path)
    }

    pub fn session_log(&self) -> Result<String> {
        crate::tools::session_log(self)
    }

    // ---- Tier B facade methods ----

    pub fn memory_search(&self, query: &str, top_k: usize) -> Result<Vec<SearchHit>> {
        crate::tools::memory_search(self, query, top_k)
    }

    pub fn memory_observe(&self, content: &str, hint: Option<&str>) -> Result<String> {
        crate::tools::memory_observe(self, content, hint)
    }

    pub fn memory_get_context(&self, max_tokens: usize) -> Result<String> {
        crate::tools::memory_get_context(self, max_tokens)
    }

    // ---- Tier C facade methods (P6 governance) ----

    pub fn mem_snapshot(&self, name: Option<&str>) -> Result<crate::snapshot::SnapshotInfo> {
        crate::tools::snapshot(self, name)
    }

    pub fn mem_snapshot_list(&self) -> Result<Vec<crate::snapshot::SnapshotInfo>> {
        crate::tools::snapshot_list(self)
    }

    pub fn mem_snapshot_restore(&self, id: &str) -> Result<()> {
        crate::tools::snapshot_restore(self, id)
    }

    /// Convenience for shutdown handlers that don't have ownership of the
    /// MemoryService: clean the session directory if we still hold the only Arc.
    pub fn try_end_session(&self, action: EndAction) {
        if let Some(arc) = &self.session {
            if action == EndAction::Discard {
                let root = arc.root().to_path_buf();
                if root.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&root) {
                        tracing::warn!("failed to discard session at {}: {}", root.display(), e);
                    }
                }
            }
        }
    }

    /// Audit-log helper used by all tools: writes to the durable mount audit
    /// log AND, if a session is active, also appends to the session's
    /// in-tmpfs log.jsonl. P6.2: when git auto-commit is enabled, also
    /// fires a best-effort `git commit -am ...`. Errors are swallowed
    /// (audit must never break the foreground tool call).
    pub(crate) fn audit_log(&self, entry: crate::audit::AuditEntry) {
        let _ = self.audit.log(entry.clone());
        if let Some(s) = &self.session {
            let _ = s.append_log(entry.clone());
        }
        if let Some(g) = &self.git {
            g.auto_commit_for(&entry);
        }
    }

    pub fn mem_log(
        &self,
        limit: usize,
        path: Option<&str>,
    ) -> Result<Vec<crate::git_repo::LogEntry>> {
        crate::tools::mem_log(self, limit, path)
    }

    pub fn mem_revert(&self, path: &str) -> Result<String> {
        crate::tools::mem_revert(self, path)
    }
}

fn start_session(config: &AppConfig, ns: &Namespace) -> Result<SessionLogService> {
    let base = config.resolved_session_dir();
    std::fs::create_dir_all(&base)?;
    let sid = match std::env::var("MEMORY_SESSION_ID") {
        Ok(s) if !s.is_empty() => match SessionId::from_string(&s) {
            Ok(sid) => sid,
            Err(e) => {
                tracing::warn!("MEMORY_SESSION_ID={s:?} rejected ({e}); generating a fresh id");
                SessionId::generate()
            }
        },
        _ => SessionId::generate(),
    };
    let agent_id = std::env::var("MCP_CLIENT_NAME").ok();
    SessionLogService::start(
        &base,
        sid,
        &config.global.user_id,
        agent_id.as_deref(),
        &ns.dir_name(),
    )
}
