use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::audit::AuditLogger;
use crate::config::AppConfig;
use crate::consolidation::{OwnedAuditEntry, run_consolidation_owned};
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
    pub embedding: Option<Arc<dyn crate::embedding::EmbeddingProvider>>,
    pub git: Option<Arc<crate::git_repo::GitHandle>>,
    pub config: AppConfig,
    /// Whether the active mount strategy entered a user namespace.
    pub entered_userns: bool,
    pub mount_strategy_name: &'static str,
    /// Counter for incremental consolidation. Incremented on every audit_log
    /// call; when it reaches `consolidation.incremental_interval`, an
    /// incremental consolidation is triggered and the counter resets.
    audit_counter: AtomicUsize,
    /// Prevents recursive consolidation: set to true while consolidate() is
    /// running, so audit_log calls from within consolidation don't re-enter.
    consolidating: std::sync::atomic::AtomicBool,
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

        // Build embedding provider from config. Best-effort.
        let embedding = match crate::embedding::build_provider(&config.memory.embedding) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("embedding provider unavailable: {e}");
                None
            }
        };

        // Start the BM25 index worker if enabled.
        let embedding_clone = embedding.clone();
        let index = if config.memory.index.enabled {
            let decay_lambda = config.memory.index.time_decay_lambda;
            let alpha = config.memory.index.time_decay_alpha;
            let exclude_cold = config.memory.index.exclude_cold_on_search;
            match IndexHandle::open(&mount, embedding_clone, decay_lambda, alpha, exclude_cold) {
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
            embedding,
            git,
            config,
            entered_userns,
            mount_strategy_name: strategy_name,
            audit_counter: AtomicUsize::new(0),
            consolidating: std::sync::atomic::AtomicBool::new(false),
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

    pub fn memory_search(
        &self,
        query: &str,
        top_k: usize,
        mode: Option<&str>,
        category: Option<&str>,
        agent_scope: Option<&str>,
    ) -> Result<Vec<SearchHit>> {
        crate::tools::memory_search(self, query, top_k, mode, category, agent_scope)
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
    /// in-tmpfs log.jsonl and the persistent mirror under
    /// `<mount>/.anolisa/session-logs/<sid>.jsonl`. P6.2: when git auto-commit
    /// is enabled, also fires a best-effort `git commit -am ...`. Errors are
    /// swallowed (audit must never break the foreground tool call).
    ///
    /// Incremental consolidation: counts tool calls and triggers consolidation
    /// when `consolidation.incremental_interval` is reached (0 = disabled).
    /// This ensures session data is persisted even if the process is killed
    /// (SIGKILL) before the normal shutdown consolidation.
    pub(crate) fn audit_log(&self, entry: crate::audit::AuditEntry) {
        let _ = self.audit.log(entry.clone());
        if let Some(s) = &self.session {
            let _ = s.append_log(entry.clone());
        }
        if let Some(g) = &self.git {
            g.auto_commit_for(&entry);
        }

        // Incremental consolidation: count tool calls, trigger when threshold
        // is reached. Skip internal tools (consolidate, compact) to avoid
        // feedback loops, and skip if consolidation is already running.
        let interval = self.config.memory.consolidation.incremental_interval;
        if interval > 0
            && !self.consolidating.load(Ordering::Acquire)
            && entry.tool != "consolidate"
            && entry.tool != "compact"
        {
            let prev = self.audit_counter.fetch_add(1, Ordering::AcqRel);
            if prev + 1 >= interval {
                self.audit_counter.store(0, Ordering::Release);
                self.consolidating.store(true, Ordering::Release);
                let n = self.consolidate();
                self.consolidating.store(false, Ordering::Release);
                if n > 0 {
                    tracing::info!("incremental consolidation: {n} facts written");
                }
            }
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

    /// Consolidate the current session's audit log into L1 atomic facts.
    /// Called during shutdown, after the session log is complete but before
    /// the session directory is discarded. Best-effort — failures are logged
    /// but do not block shutdown.
    ///
    /// Returns the number of facts written (0 when consolidation was skipped
    /// or produced nothing).
    pub fn consolidate(&self) -> usize {
        let config = &self.config.memory.consolidation;
        if !config.enabled {
            tracing::debug!("consolidation disabled, skipping");
            return 0;
        }

        let session = match &self.session {
            Some(s) => s,
            None => {
                tracing::debug!("no session available, skipping consolidation");
                return 0;
            }
        };

        // Read the session log.
        let log_content = match session.read_log() {
            Ok(s) if !s.is_empty() => s,
            Ok(_) => {
                tracing::debug!("session log is empty, skipping consolidation");
                return 0;
            }
            Err(e) => {
                tracing::warn!("failed to read session log for consolidation: {e}");
                return 0;
            }
        };

        // Parse JSONL entries into owned structs (AuditEntry uses &'static str
        // for tool which can't be deserialized).
        let entries: Vec<OwnedAuditEntry> = log_content
            .lines()
            .filter_map(|line| serde_json::from_str::<OwnedAuditEntry>(line).ok())
            .collect();

        if entries.is_empty() {
            tracing::debug!("no parseable audit entries, skipping consolidation");
            return 0;
        }

        let session_id = session.sid().as_str();

        // Quality filter: check mutual exclusion BEFORE running heuristics
        // to avoid wasted I/O and CPU.
        let manual_count = entries
            .iter()
            .filter(|e| e.tool == "memory_observe")
            .count();
        if crate::consolidation::quality::should_skip_consolidation(manual_count) {
            tracing::info!(
                "skipping consolidation: session {session_id} has {manual_count} manual observations"
            );
            return 0;
        }

        // Convert to OwnedAuditEntry for heuristics.
        let mut facts = run_consolidation_owned(&entries, session_id, config);

        // Quality filter: remove derivable facts and normalize dates
        let before = facts.len();
        facts.retain(|f| !crate::consolidation::quality::is_derivable(&f.content));
        facts.iter_mut().for_each(|f| {
            f.content = crate::consolidation::quality::normalize_relative_dates(&f.content);
        });
        let filtered = before - facts.len();
        if filtered > 0 {
            tracing::debug!("filtered {filtered} derivable facts");
        }

        if facts.is_empty() {
            tracing::debug!("consolidation produced no facts for session {session_id}");
            return 0;
        }

        // Write facts to the memory store via sandboxed FactWriter.
        let mut writer = crate::consolidation::FactWriter::new(&self.mount.root)
            .with_root_fd(self.mount.root_fd.clone());
        // Wire conflict detection when both config and index are available.
        if config.conflict_detection {
            if let Some(ref index_handle) = self.index {
                let store = index_handle.store_arc();
                writer = writer.with_index(store, config.conflict_bm25_threshold);
            }
        }
        match writer.write_batch(&facts) {
            Ok(n) => {
                tracing::info!(
                    "consolidation complete: {n}/{} facts written from session {session_id}",
                    facts.len()
                );
                // Log an audit entry for consolidation.
                self.audit_log(
                    crate::audit::AuditEntry::new("consolidate")
                        .path(format!("{n} facts from session {session_id}"))
                        .bytes(n as u64),
                );
                n
            }
            Err(e) => {
                tracing::warn!("consolidation write failed: {e}");
                0
            }
        }
    }

    /// Compact the memory index: mark old, never-accessed files as cold.
    pub fn compact(&self) -> Result<usize> {
        let index = match self.index.as_ref() {
            Some(i) => i,
            None => {
                return Err(crate::error::MemoryError::NotImplemented(
                    "index disabled; compact requires an active index",
                ));
            }
        };
        let cold_after = self.config.memory.index.cold_after_days;
        index.compact(cold_after)
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

    // Persistent mirror directory: <mount>/.anolisa/session-logs/
    // Session logs are mirrored here so they survive SIGKILL and tmpfs loss.
    let mirror_dir = mount_root_for_session_mirror(config, ns);

    SessionLogService::start(
        &base,
        sid,
        &config.global.user_id,
        agent_id.as_deref(),
        &ns.dir_name(),
        mirror_dir.as_deref(),
    )
}

/// Compute the persistent mirror directory for session logs.
/// Returns `<base_dir>/<ns>/session-logs/` under the mount root.
fn mount_root_for_session_mirror(config: &AppConfig, ns: &Namespace) -> Option<PathBuf> {
    let base = config.resolved_base_dir();
    let mirror = base
        .join(ns.dir_name())
        .join(".anolisa")
        .join("session-logs");
    Some(mirror)
}
