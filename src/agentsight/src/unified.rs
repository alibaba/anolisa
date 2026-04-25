//! AgentSight - Unified entry point for AI Agent observability
//!
//! This module provides the main `AgentSight` struct that orchestrates the entire
//! data pipeline: probes → parser → aggregator → analyzer → storage.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                            AgentSight                                │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │   probes     parser    aggregator    analyzer    genai    storage    │
//! │     ↓          ↓           ↓            ↓          ↓        ↓       │
//! │   Event   ParsedMessage  Aggregated   Analysis  Semantic  持久化    │
//! │                          Result       Result    Events              │
//! │                                                  ↓                  │
//! │                                            GenAI Storage            │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::aggregator::Aggregator;
use crate::analyzer::Analyzer;
use crate::config::AgentsightConfig;
use crate::discovery::AgentScanner;
use crate::event::Event;
use crate::ffi::{FfiEvent, FfiEventSender};
use crate::genai::{GenAIBuilder, GenAIExporter, GenAIStore, SlsUploader};
use crate::genai::semantic::GenAISemanticEvent;
use crate::interruption::{InterruptionDetector, DetectorConfig};
use crate::parser::Parser;
use crate::probes::{Probes, ProbesPoller, FileWatchEvent, FileWriteEvent};
use crate::storage::{
    SqliteConfig, Storage, TimePeriod, TokenQuery, TokenQueryResult,
};
use crate::storage::sqlite::{GenAISqliteStore, InterruptionStore};
use crate::tokenizer::LlmTokenizer;
use crate::response_map::ResponseSessionMapper;

/// Main AgentSight struct for tracing AI agent activity
///
/// This is the unified entry point that orchestrates:
/// - `Probes`: eBPF-based event capture
/// - `Parser`: Message parsing
/// - `Aggregator`: Event aggregation
/// - `Analyzer`: Analysis and record extraction
/// - `Storage`: Persistence
/// - `AgentScanner`: Process lifecycle tracking
pub struct AgentSight {
    /// BPF probes manager
    probes: Probes,
    /// Message parser (unified)
    parser: Parser,
    /// Event aggregator (unified)
    aggregator: Aggregator,
    /// Unified analyzer
    analyzer: Analyzer,
    /// GenAI semantic builder
    genai_builder: GenAIBuilder,
    /// Pluggable GenAI event exporters (JSONL, SLS, etc.)
    genai_exporters: Vec<Box<dyn GenAIExporter>>,
    /// Direct reference to the SQLite GenAI store for two-phase pending/complete writes.
    /// `None` when SLS is configured (SQLite exporter is not registered in that case).
    genai_sqlite_store: Option<Arc<GenAISqliteStore>>,
    /// Interruption event detector (online rules)
    interruption_detector: InterruptionDetector,
    /// Interruption event store (SQLite)
    interruption_store: Option<Arc<InterruptionStore>>,
    /// Unified storage
    storage: Storage,
    /// Agent scanner for process lifecycle tracking
    scanner: AgentScanner,
    /// Poller handle
    _poller: ProbesPoller,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Event counter
    event_count: u64,
    /// File watch callback for .jsonl file open events
    filewatch_callback: Option<Box<dyn Fn(FileWatchEvent) + Send + 'static>>,
    /// ResponseId → SessionId mapper for FileWrite events
    response_mapper: ResponseSessionMapper,
    /// Pending GenAI events awaiting session_id resolution from ResponseSessionMapper
    pending_genai: Vec<PendingGenAI>,
    /// Optional FFI event sender (set when running in FFI/C-API mode)
    ffi_sender: Option<FfiEventSender>,
    /// Rate-limiter for dead-PID connection drain (at most once per second)
    last_drain_check: std::time::Instant,
    /// Cache of pid → agent_name, persists after process exit for deferred resolution
    pid_agent_name_cache: HashMap<u32, String>,
}

/// GenAI events waiting for session_id resolution via ResponseSessionMapper.
/// If the mapper lookup succeeds within the timeout, session_id metadata is updated
/// before export. Otherwise, the events are exported with the hash-based fallback.
struct PendingGenAI {
    events: Vec<GenAISemanticEvent>,
    response_id: String,
    created_at: std::time::Instant,
}

/// Maximum time to wait for ResponseSessionMapper to resolve a session_id
const PENDING_SESSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl AgentSight {
    /// Create a new AgentSight instance from configuration
    ///
    /// # Arguments
    /// * `config` - AgentsightConfig containing all configuration parameters
    ///
    /// # Example
    /// ```rust,ignore
    /// use agentsight::{AgentSight, AgentsightConfig};
    ///
    /// let config = AgentsightConfig::new();
    /// let mut sight = AgentSight::new(config)?;
    /// ```
    pub fn new(config: AgentsightConfig) -> Result<Self> {
        config.apply_verbose();

        // Create probes - agent discovery is handled by AgentScanner via ProcMon events
        let mut probes =
            Probes::new(&[], config.target_uid, config.enable_filewatch).context("Failed to create probes")?;

        // Attach procmon for process monitoring
        probes.attach().context("Failed to attach probes")?;

        // Create scanner and scan for existing agent processes
        let mut scanner = AgentScanner::new();
        let existing_agents = scanner.scan();

        // Attach SSL probes to already-running agents
        for agent in &existing_agents {
            Self::attach_process_internal(&mut probes, agent.pid, &agent.agent_info.name);
        }

        // Build pid → agent_name cache from existing agents (persists after process exit)
        let mut pid_agent_name_cache = HashMap::new();
        for agent in &existing_agents {
            pid_agent_name_cache.insert(agent.pid, agent.agent_info.name.clone());
        }

        // Start polling (non-blocking)
        let _poller = probes.run().context("Failed to start probe poller")?;

        // Initialize unified storage based on config
        let storage = Self::create_storage(&config)?;

        // Build GenAI exporters
        let mut genai_exporters: Vec<Box<dyn GenAIExporter>> = Vec::new();
        let mut genai_sqlite_store: Option<Arc<GenAISqliteStore>> = None;

        // Always add local JSONL exporter
        genai_exporters.push(Box::new(GenAIStore::new(&GenAIStore::default_path())));

        // Add SLS exporter if configured, otherwise fallback to SQLite
        if config.sls_enabled() {
            match SlsUploader::new(&config) {
                Ok(uploader) => {
                    log::info!("SLS exporter enabled");
                    genai_exporters.push(Box::new(uploader));
                }
                Err(e) => {
                    log::warn!("Failed to initialize SLS exporter: {}", e);
                }
            }
        } else {
            // No SLS credentials configured, use SQLite as local storage
            match GenAISqliteStore::new() {
                Ok(store) => {
                    log::info!("SQLite GenAI exporter enabled (SLS not configured)");
                    let store = Arc::new(store);
                    genai_sqlite_store = Some(Arc::clone(&store));
                    genai_exporters.push(Box::new(store));
                }
                Err(e) => {
                    log::warn!("Failed to initialize SQLite GenAI exporter: {}", e);
                }
            }
        }

        // Create analyzer with tokenizer if configured
        let analyzer = if let Some(ref tokenizer_path) = config.tokenizer_path {
            if Path::new(tokenizer_path).exists() {
                // Assume tokenizer_config.json is in the same directory
                let config_path = Path::new(tokenizer_path)
                    .parent()
                    .map(|p| p.join("tokenizer_config.json"))
                    .unwrap_or_else(|| Path::new("tokenizer_config.json").to_path_buf());
                
                match LlmTokenizer::from_file(tokenizer_path, &config_path) {
                    Ok(tokenizer) => {
                        log::info!(
                            "Tokenizer loaded from: {:?}",
                            tokenizer_path
                        );
                        Analyzer::with_tokenizer(tokenizer.clone(), tokenizer)
                    }
                    Err(e) => {
                        log::warn!("Failed to load tokenizer from {:?}: {}. Using analyzer without tokenizer.", tokenizer_path, e);
                        Analyzer::new()
                    }
                }
            } else {
                log::warn!("Tokenizer file not found: {:?}. Using analyzer without tokenizer.", tokenizer_path);
                Analyzer::new()
            }
        } else {
            Analyzer::new()
        };

        // Initialize interruption store (co-located in same directory as genai db)
        let interruption_store: Option<Arc<InterruptionStore>> = {
            let db_path = GenAISqliteStore::default_path()
                .parent()
                .unwrap_or(std::path::Path::new("/var/log/sysak/.agentsight"))
                .join("interruption_events.db");
            match InterruptionStore::new_with_path(&db_path) {
                Ok(store) => {
                    log::info!("Interruption events store initialized at {:?}", db_path);
                    Some(Arc::new(store))
                }
                Err(e) => {
                    log::warn!("Failed to initialize interruption store: {}", e);
                    None
                }
            }
        };

        log::info!(
            "AgentSight initialized: {} existing agent(s), {} GenAI exporter(s)",
            existing_agents.len(),
            genai_exporters.len(),
        );

        // Spawn background thread that marks stale PENDING calls as 'interrupted'.
        // Fires every 60 seconds; any pending call older than 5 minutes is assumed lost.
        if let Some(ref sqlite_store) = genai_sqlite_store {
            let store_ref = Arc::clone(sqlite_store);
            std::thread::Builder::new()
                .name("genai-stale-scanner".to_string())
                .spawn(move || {
                    log::info!("GenAI stale-pending scanner started (interval=60s, timeout=300s)");
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(60));
                        if let Err(e) = store_ref.mark_interrupted_stale(300) {
                            log::warn!("Stale-pending scan failed: {}", e);
                        }
                    }
                })
                .ok();
        }

        Ok(AgentSight {
            probes,
            parser: Parser::new(),
            aggregator: Aggregator::new(),
            analyzer,
            genai_builder: GenAIBuilder::new(),
            genai_exporters,
            genai_sqlite_store,
            interruption_detector: InterruptionDetector::new(DetectorConfig::default()),
            interruption_store,
            storage,
            scanner,
            _poller,
            running: Arc::new(AtomicBool::new(true)),
            event_count: 0,
            filewatch_callback: None,
            response_mapper: ResponseSessionMapper::new(),
            pending_genai: Vec::new(),
            ffi_sender: None,
            last_drain_check: std::time::Instant::now(),
            pid_agent_name_cache,
        })
    }

    /// Create storage backend from configuration
    fn create_storage(config: &AgentsightConfig) -> Result<Storage> {
        let sqlite_config = SqliteConfig {
            base_path: config.storage_base_path.clone(),
            db_name: config.db_name.clone(),
            audit_table: config.audit_table.clone(),
            token_table: config.token_table.clone(),
            http_table: config.http_table.clone(),
            token_consumption_table: "token_consumption".to_string(),
            retention_days: config.retention_days,
            purge_interval: config.purge_interval,
        };
        Storage::with_sqlite_config(&sqlite_config)
    }

    /// Check if running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get a clone of the running flag for use in signal handlers
    pub fn running_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.running)
    }

    /// Get event count
    pub fn event_count(&self) -> u64 {
        self.event_count
    }

    /// Attach SSL probes to a specific agent process
    pub fn attach_process(&mut self, pid: u32, agent_name: &str) {
        Self::attach_process_internal(&mut self.probes, pid, agent_name);
    }

    /// Internal helper to attach SSL probes to a process
    fn attach_process_internal(probes: &mut Probes, pid: u32, agent_name: &str) {
        log::debug!("Attaching to pid {}, agent name: {}", pid, agent_name);
        if let Err(e) = probes.attach_process(pid as i32) {
            log::error!("Failed to attach SSL probe to pid {}: {}", pid, e);
        } else {
            log::info!("Attached to agent: {} (pid={})", agent_name, pid);
        }
    }

    /// Detach SSL probes from a specific agent process
    pub fn detach_process(&mut self, pid: u32, agent_name: &str) {
        log::debug!("Detaching from pid {}, agent name: {}", pid, agent_name);
        let _ = self.probes.remove_traced_pid(pid).inspect_err(|e| {
            log::error!("failed to delete {pid} from traced pid map: {e}");
        });
    }

    /// Try to receive and process the next event (non-blocking)
    /// Returns None if no event is available
    pub fn try_process(&mut self) -> Option<u64> {
        if !self.running.load(Ordering::SeqCst) {
            return None;
        }

        let event = self.probes.try_recv()?;
        self.event_count += 1;

        log::debug!("Processing event: {:?}", event.event_type());

        // Handle ProcMon events for agent lifecycle tracking
        if let Event::ProcMon(ref procmon_event) = event {
            self.handle_procmon_event(procmon_event);
            return None;
        }

        // Handle FileWatch events via callback (not through the pipeline)
        if let Event::FileWatch(ref fw_event) = event {
            self.handle_filewatch_event(fw_event);
            return None;
        }

        // Handle FileWrite events via callback (not through the pipeline)
        if let Event::FileWrite(ref fw_event) = event {
            self.handle_filewrite_event(fw_event);
            // After mapper is updated, try to resolve any pending GenAI events
            self.resolve_pending_genai();
            return None;
        }

        // Parse the event
        let result = self.parser.parse_event(event);

        // Process messages through aggregator
        let aggregated_results = self.aggregator.process_result(result);

        // Analyze and store results
        for agg_result in &aggregated_results {
            let analysis_results = self.analyzer.analyze_aggregated(agg_result);

            // Build GenAI semantic events AND pending info in one pass
            let (output, pending_info) = self.genai_builder.build_with_pending(&analysis_results, &self.response_mapper, &self.pid_agent_name_cache);

            if !output.events.is_empty() {
                if output.pending_response_id.is_some() {
                    // Session_id not yet resolved — queue for deferred resolution
                    self.pending_genai.push(PendingGenAI {
                        events: output.events,
                        response_id: output.pending_response_id.unwrap(),
                        created_at: std::time::Instant::now(),
                    });
                    log::debug!("GenAI events queued for deferred session_id resolution");
                } else {
                    // Session_id resolved (or no response_id) — export immediately.
                    // For SQLite: write pending first, then complete_pending;
                    // for other exporters: normal export.
                    if let Some(ref info) = pending_info {
                        if let Some(sqlite_store) = self.genai_sqlite_store.as_ref() {
                            if let Err(e) = sqlite_store.insert_pending(info) {
                                log::warn!("Failed to insert pending call {}: {}", info.call_id, e);
                            }
                            for event in &output.events {
                                if let Err(e) = sqlite_store.complete_pending(event) {
                                    log::warn!("Failed to complete pending call: {}", e);
                                }
                            }
                            // Export to non-SQLite exporters only (SQLite already written)
                            for exporter in &self.genai_exporters {
                                if exporter.name() != "sqlite" {
                                    exporter.export(&output.events);
                                    log::debug!("Exported {} GenAI events via '{}'", output.events.len(), exporter.name());
                                }
                            }
                        } else {
                            self.export_genai_events(&output.events);
                        }
                    } else {
                        self.export_genai_events(&output.events);
                    }

                    // ── Online interruption detection ─────────────────────────────
                    // Run after export so the call is already persisted.
                    self.detect_and_store_interruptions(&output.events);
                }
            } else if let Some(ref sender) = self.ffi_sender {
                // No LLM event produced — send plain HTTP data via FFI channel
                for ar in &analysis_results {
                    if let crate::analyzer::AnalysisResult::Http(record) = ar {
                        sender.send(FfiEvent::Https(record.clone()));
                    }
                }
            }

            // In FFI mode data is delivered via callbacks; skip local storage.
            if self.ffi_sender.is_none() {
                for analysis_result in &analysis_results {
                    if let Err(e) = self.storage.store(analysis_result) {
                        log::warn!("Failed to store analysis result: {}", e);
                    } else {
                        log::debug!("Analysis result saved");
                    }
                }
            }
        }

        Some(self.event_count)
    }

    /// Handle ProcMon event for agent lifecycle tracking
    fn handle_procmon_event(&mut self, event: &crate::probes::procmon::Event) {
        use crate::probes::procmon::Event as ProcMonEvent;

        match event {
            ProcMonEvent::Exec { pid, comm, .. } => {
                // Check if this is a known agent and start tracking
                if let Some(agent) = self.scanner.on_process_create(*pid, comm) {
                    let agent_name = agent.agent_info.name.clone();
                    self.pid_agent_name_cache.insert(*pid, agent_name.clone());
                    self.attach_process(*pid, &agent_name);
                }
            }
            ProcMonEvent::Exit { pid, .. } => {
                // Remove from tracking if it was an agent
                if let Some(agent) = self.scanner.on_process_exit(*pid) {
                    let agent_name = agent.agent_info.name.clone();
                    self.detach_process(*pid, &agent_name);
                }
            }
        }
    }

    /// Handle FileWatch event via registered callback
    fn handle_filewatch_event(&self, event: &FileWatchEvent) {
        log::debug!("FileWatch: pid={} file={}", event.pid, event.filename);
        if let Some(ref cb) = self.filewatch_callback {
            cb(event.clone());
        }
    }

    /// Register a callback for file watch events (.jsonl file opens)
    pub fn on_filewatch<F>(&mut self, callback: F)
    where
        F: Fn(FileWatchEvent) + Send + 'static,
    {
        self.filewatch_callback = Some(Box::new(callback));
    }

    /// Handle FileWrite event: extract responseId→sessionId mapping, then call callback
    fn handle_filewrite_event(&mut self, event: &FileWriteEvent) {
        log::debug!("FileWrite: pid={} file={} size={}", event.pid, event.filename, event.write_size);
        self.response_mapper.process_filewrite(event);
    }


    /// Run the event loop (blocking)
    pub fn run(&mut self) -> Result<u64> {
        log::debug!("Agent discovery running via ProcMon events");

        // Main event loop
        while self.running.load(Ordering::SeqCst) {
            if let Some(result) = self.try_process() {
                log::trace!("[Event {}] Processed", result);
            } else {
                // No event available — flush any timed-out pending GenAI events
                self.flush_expired_pending_genai();
                // Drain orphaned connections from dead PIDs and persist as pending
                self.drain_and_persist_dead_connections();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        // On shutdown, flush all remaining pending events with fallback session_id
        self.flush_all_pending_genai();

        Ok(self.event_count)
    }

    /// Shutdown gracefully
    pub fn shutdown(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Flush all pending GenAI events before exit
        self.flush_all_pending_genai();
        // poller will be dropped automatically when AgentSight is dropped
    }

    /// Install an FFI event sender for C API mode.
    /// When set, completed events are pushed through this channel.
    pub fn set_ffi_sender(&mut self, sender: FfiEventSender) {
        self.ffi_sender = Some(sender);
    }

    /// Export GenAI events to all registered exporters
    fn export_genai_events(&self, events: &[GenAISemanticEvent]) {
        if let Some(ref sender) = self.ffi_sender {
            // FFI mode: deliver LLMCall events via callback channel only.
            for event in events {
                if let GenAISemanticEvent::LLMCall(call) = event {
                    sender.send(FfiEvent::Llm(call.clone()));
                }
            }
        } else {
            // Normal mode: export to all registered exporters.
            for exporter in &self.genai_exporters {
                exporter.export(events);
                log::debug!("Exported {} GenAI events via '{}'", events.len(), exporter.name());
            }
        }
    }

    /// Online interruption detection: inspect exported events and persist any
    /// detected interruption records.  Also stamps the `interruption_type`
    /// column on the corresponding `genai_events` row when SQLite is in use.
    fn detect_and_store_interruptions(&self, events: &[GenAISemanticEvent]) {
        if let Some(ref istore) = self.interruption_store {
            for event in events {
                if let GenAISemanticEvent::LLMCall(llm_call) = event {
                    let interruptions = self.interruption_detector.detect(llm_call);
                    for ie in &interruptions {
                        // Deduplicate: skip if same (conversation_id, type, error_msg)
                        // already recorded.  Same error retried N times produces only
                        // 1 interruption; different errors each get 1.
                        if let Some(ref cid) = ie.conversation_id {
                            let error_msg = llm_call.error.as_deref();
                            if istore.exists_for_conversation(cid, &ie.interruption_type, error_msg) {
                                log::debug!(
                                    "Skipping duplicate {:?} for conversation_id={} error={:?}",
                                    ie.interruption_type, cid, error_msg
                                );
                                // Still stamp the genai_events row so the call is marked
                                if let Some(ref sqlite) = self.genai_sqlite_store {
                                    let _ = sqlite.update_interruption_type(
                                        &llm_call.call_id,
                                        ie.interruption_type.as_str(),
                                    );
                                }
                                continue;
                            }
                        }
                        if let Err(e) = istore.insert(ie) {
                            log::warn!("Failed to store interruption event: {}", e);
                        }
                        // Also stamp genai_events row with interruption_type
                        if let Some(ref sqlite) = self.genai_sqlite_store {
                            let _ = sqlite.update_interruption_type(
                                &llm_call.call_id,
                                ie.interruption_type.as_str(),
                            );
                        }
                    }
                }
            }
        }
    }

    /// Drain aggregator connections whose PID is no longer alive and persist
    /// them as `pending` records in `genai_events`.  Rate-limited to once per
    /// second to avoid excessive `/proc` scanning.
    fn drain_and_persist_dead_connections(&mut self) {
        if self.last_drain_check.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        self.last_drain_check = std::time::Instant::now();

        let drained = self.aggregator.drain_dead_pid_connections();
        if drained.is_empty() {
            return;
        }

        use crate::aggregator::ConnectionState;
        use crate::genai::GenAIBuilder;

        for (conn_id, state) in drained {
            // Destructure to capture both request AND sse_events
            let (state_name, request, sse_events) = match state {
                ConnectionState::RequestPending { request } => {
                    ("RequestPending", request, vec![])
                }
                ConnectionState::SseActive { request: Some(req), sse_events, .. } => {
                    ("SseActive", req, sse_events)
                }
                _ => continue,
            };

            if let Some(pending) = self.genai_builder.build_pending_from_request(&request, &conn_id, &self.pid_agent_name_cache) {
                if let Some(ref store) = self.genai_sqlite_store {
                    let call_id = pending.call_id.clone();
                    let pid = pending.pid;

                    if let Err(e) = store.insert_pending(&pending) {
                        log::warn!("[DrainCheck] FAIL persist: {}", e);
                        continue;
                    }
                    // ── Session ID reconciliation ──────────────────────────
                    // The drain path computes session_id via SHA256 hash fallback,
                    // but normal flow uses ResponseSessionMapper (agent .jsonl UUID).
                    // Look up the real session_id from completed records for the same PID.
                    match store.lookup_session_for_pid(pid) {
                        Ok(Some(ref real_session_id)) => {
                            if pending.session_id.as_deref() != Some(real_session_id.as_str()) {
                                if let Err(e) = store.update_session_id(&call_id, real_session_id) {
                                    log::warn!("[DrainCheck] FAIL update session_id: {}", e);
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            log::warn!("[DrainCheck] FAIL lookup session: {}", e);
                        }
                    }

                    // ── SSE enrichment ────────────────────────────────────
                    // Parse captured SSE events for model, trace_id, tokens, output content
                    if !sse_events.is_empty() {
                        if let Some(mut enrichment) = GenAIBuilder::extract_sse_enrichment(&sse_events) {
                            // If SSE didn't carry usage data (stream was interrupted before
                            // the final chunk), compute tokens via the real tokenizer.
                            if enrichment.input_tokens.is_none() || enrichment.output_tokens.is_none() {
                                let model_name = enrichment.model.as_deref()
                                    .or(pending.model.as_deref())
                                    .unwrap_or("unknown");
                                if let Ok(tokenizer) = crate::tokenizer::get_global_tokenizer(model_name) {
                                    // ── input tokens ──
                                    if enrichment.input_tokens.is_none() {
                                        if let Some(body) = request.json_body() {
                                            if let Some(messages) = body.get("messages").and_then(|m| m.as_array()) {
                                                let mut msgs = messages.clone();
                                                // Parse tool_calls.arguments from string to object
                                                for msg in msgs.iter_mut() {
                                                    if let Some(tcs) = msg.get_mut("tool_calls").and_then(|tc| tc.as_array_mut()) {
                                                        for tc in tcs.iter_mut() {
                                                            if let Some(f) = tc.get_mut("function") {
                                                                if let Some(a) = f.get("arguments").and_then(|a| a.as_str()) {
                                                                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(a) {
                                                                        f["arguments"] = p;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                let tools_json: Option<Vec<serde_json::Value>> = body.get("tools")
                                                    .and_then(|t| t.as_array()).map(|a| a.to_vec());
                                                let count = match tokenizer.apply_chat_template_with_tools(&msgs, tools_json.as_deref(), true) {
                                                    Ok(formatted) => tokenizer.count(&formatted).unwrap_or(0),
                                                    Err(_) => {
                                                        // Fallback: raw message count
                                                        msgs.iter().filter_map(|m| serde_json::to_string(m).ok())
                                                            .map(|s| tokenizer.count(&s).unwrap_or(0))
                                                            .sum()
                                                    }
                                                };
                                                if count > 0 {
                                                    enrichment.input_tokens = Some(count as i64);
                                                }
                                            }
                                        }
                                    }
                                    // ── output tokens ──
                                    if enrichment.output_tokens.is_none() {
                                        use crate::analyzer::token::extract_response_content;
                                        let mut all_content = String::new();
                                        let mut all_reasoning = String::new();
                                        let mut all_tool_calls = Vec::new();
                                        for ev in &sse_events {
                                            if let Some(chunk) = ev.json_body() {
                                                if let Some((content, reasoning, tool_calls)) = extract_response_content(Some(&chunk)) {
                                                    if !content.is_empty() { all_content.push_str(&content); }
                                                    if let Some(r) = reasoning { if !r.is_empty() { all_reasoning.push_str(&r); } }
                                                    for tc in tool_calls { if !tc.is_empty() { all_tool_calls.push(tc); } }
                                                }
                                            }
                                        }
                                        let mut total = 0usize;
                                        if !all_reasoning.is_empty() {
                                            let wrapped = format!("<think>\n{}\n</think>\n\n", all_reasoning);
                                            total += tokenizer.count(&wrapped).unwrap_or(0);
                                        }
                                        if !all_content.is_empty() {
                                            total += tokenizer.count(&all_content).unwrap_or(0);
                                        }
                                        if !all_tool_calls.is_empty() {
                                            total += tokenizer.count(&all_tool_calls.join("")).unwrap_or(0);
                                        }
                                        if total > 0 {
                                            enrichment.output_tokens = Some(total as i64);
                                        }
                                    }
                                } else {
                                    log::warn!("[DrainCheck] tokenizer unavailable for model {:?}, skipping token computation",
                                        enrichment.model.as_deref().or(pending.model.as_deref()));
                                }
                            }
                            if let Err(e) = store.enrich_pending_from_sse(&call_id, &enrichment) {
                                log::warn!("[DrainCheck] FAIL enrich SSE: {}", e);
                            }
                        }
                    }
                }
            } else {
                log::debug!("[DrainCheck] build_pending returned None: pid={} path={} body_len={}",
                    conn_id.pid, request.path, request.body_len);
            }
        }
    }

    /// Try to resolve pending GenAI events whose session_id can now be looked up.
    /// Called after FileWrite events update the ResponseSessionMapper.
    fn resolve_pending_genai(&mut self) {
        if self.pending_genai.is_empty() {
            return;
        }

        let pending_items: Vec<_> = self.pending_genai.drain(..).collect();
        let mut still_pending = Vec::new();
        let mut to_export: Vec<Vec<GenAISemanticEvent>> = Vec::new();

        for mut pending in pending_items {
            if let Some(session_id) = self.response_mapper
                .get_session_by_response_id(&pending.response_id)
                .map(|s| s.to_string())
            {
                // Resolved — update session_id in all event metadata
                log::debug!(
                    "Deferred session_id resolved: response_id={} → session_id={}",
                    pending.response_id, session_id
                );
                for event in &mut pending.events {
                    if let GenAISemanticEvent::LLMCall(call) = event {
                        call.metadata.insert("session_id".to_string(), session_id.clone());
                    }
                }
                to_export.push(pending.events);
            } else if pending.created_at.elapsed() >= PENDING_SESSION_TIMEOUT {
                // Timed out — export with fallback session_id
                log::debug!(
                    "Deferred session_id timed out for response_id={}, using fallback",
                    pending.response_id
                );
                to_export.push(pending.events);
            } else {
                // Still waiting
                still_pending.push(pending);
            }
        }

        self.pending_genai = still_pending;

        for events in &to_export {
            self.export_genai_events(events);
            self.detect_and_store_interruptions(events);
        }
    }

    /// Flush any pending GenAI events that have exceeded the timeout.
    /// Called during idle periods of the event loop.
    fn flush_expired_pending_genai(&mut self) {
        if self.pending_genai.is_empty() {
            return;
        }

        let pending_items: Vec<_> = self.pending_genai.drain(..).collect();
        let mut still_pending = Vec::new();
        let mut to_export: Vec<Vec<GenAISemanticEvent>> = Vec::new();

        for pending in pending_items {
            if pending.created_at.elapsed() >= PENDING_SESSION_TIMEOUT {
                log::debug!(
                    "Deferred session_id expired for response_id={}, using fallback",
                    pending.response_id
                );
                to_export.push(pending.events);
            } else {
                still_pending.push(pending);
            }
        }

        self.pending_genai = still_pending;

        for events in &to_export {
            self.export_genai_events(events);
            self.detect_and_store_interruptions(events);
        }
    }

    /// Flush all remaining pending GenAI events (on shutdown).
    fn flush_all_pending_genai(&mut self) {
        let pending_items: Vec<_> = self.pending_genai.drain(..).collect();
        for pending in &pending_items {
            log::debug!(
                "Flushing pending GenAI event on shutdown: response_id={}",
                pending.response_id
            );
        }
        for pending in pending_items {
            self.export_genai_events(&pending.events);
            self.detect_and_store_interruptions(&pending.events);
        }
    }

    /// Get reference to aggregator
    pub fn aggregator(&self) -> &Aggregator {
        &self.aggregator
    }

    /// Get mutable reference to aggregator
    pub fn aggregator_mut(&mut self) -> &mut Aggregator {
        &mut self.aggregator
    }

    /// Get reference to analyzer
    pub fn analyzer(&self) -> &Analyzer {
        &self.analyzer
    }

    /// Get reference to storage
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// Get reference to GenAI exporters
    pub fn genai_exporters(&self) -> &[Box<dyn GenAIExporter>] {
        &self.genai_exporters
    }

    /// Add a custom GenAI exporter at runtime
    pub fn add_genai_exporter(&mut self, exporter: Box<dyn GenAIExporter>) {
        log::info!("Registered GenAI exporter: '{}'", exporter.name());
        self.genai_exporters.push(exporter);
    }

    /// Get reference to agent scanner
    pub fn scanner(&self) -> &AgentScanner {
        &self.scanner
    }

    /// Get mutable reference to agent scanner
    pub fn scanner_mut(&mut self) -> &mut AgentScanner {
        &mut self.scanner
    }

    /// Query token usage by time period
    pub fn query_tokens(&self, period: TimePeriod) -> TokenQueryResult {
        let query = TokenQuery::new(self.storage.token());
        query.by_period(period)
    }

    /// Query token usage by last N hours
    pub fn query_tokens_by_hours(&self, hours: u64) -> TokenQueryResult {
        let query = TokenQuery::new(self.storage.token());
        query.by_hours(hours)
    }

    /// Query token usage with comparison
    pub fn query_tokens_with_compare(&self, period: TimePeriod) -> TokenQueryResult {
        let query = TokenQuery::new(self.storage.token());
        query.by_period_with_compare(period)
    }

    /// Query token usage with breakdown
    pub fn query_tokens_with_breakdown(&self, period: TimePeriod) -> TokenQueryResult {
        let query = TokenQuery::new(self.storage.token());
        query.by_period_with_breakdown(period)
    }

    /// Full token query with comparison and breakdown
    pub fn query_tokens_full(&self, period: TimePeriod) -> TokenQueryResult {
        let query = TokenQuery::new(self.storage.token());
        query.full_query(period)
    }
}

impl Drop for AgentSight {
    fn drop(&mut self) {
        self.shutdown();
    }
}
