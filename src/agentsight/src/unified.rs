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
use crate::parser::Parser;
use crate::probes::{Probes, ProbesPoller, FileWatchEvent, FileWriteEvent};
use crate::storage::{
    SqliteConfig, Storage, TimePeriod, TokenQuery, TokenQueryResult,
};
use crate::storage::sqlite::GenAISqliteStore;
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

        // Start polling (non-blocking)
        let _poller = probes.run().context("Failed to start probe poller")?;

        // Initialize unified storage based on config
        let storage = Self::create_storage(&config)?;

        // Build GenAI exporters
        let mut genai_exporters: Vec<Box<dyn GenAIExporter>> = Vec::new();

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

        log::info!(
            "AgentSight initialized: {} existing agent(s), {} GenAI exporter(s)",
            existing_agents.len(),
            genai_exporters.len(),
        );

        Ok(AgentSight {
            probes,
            parser: Parser::new(),
            aggregator: Aggregator::new(),
            analyzer,
            genai_builder: GenAIBuilder::new(),
            genai_exporters,
            storage,
            scanner,
            _poller,
            running: Arc::new(AtomicBool::new(true)),
            event_count: 0,
            filewatch_callback: None,
            response_mapper: ResponseSessionMapper::new(),
            pending_genai: Vec::new(),
            ffi_sender: None,
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

            // Build GenAI semantic events from analysis results (reuse extracted data)
            let output = self.genai_builder.build(&analysis_results, &self.response_mapper);

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
                    // Session_id resolved (or no response_id) — export immediately
                    self.export_genai_events(&output.events);
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
