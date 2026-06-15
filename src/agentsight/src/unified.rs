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
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::aggregator::Aggregator;
use crate::analyzer::Analyzer;
use crate::config::AgentsightConfig;
use crate::discovery::AgentScanner;
use crate::event::Event;
use crate::ffi::{FfiEvent, FfiEventSender};
use crate::genai::semantic::GenAISemanticEvent;
use crate::genai::{GenAIBuilder, GenAIExporter, GenAIStore, LogtailExporter};
use crate::interruption::{DetectorConfig, InterruptionDetector, recover_oom_events};
use crate::parser::Parser;
use crate::probes::{FileWatchEvent, FileWriteEvent, Probes, ProbesPoller};
use crate::response_map::ResponseSessionMapper;
use crate::storage::sqlite::{GenAISqliteStore, InterruptionStore};
use crate::storage::{SqliteConfig, Storage, TimePeriod, TokenQuery, TokenQueryResult};
use crate::tokenizer::LlmTokenizer;

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
    /// HTTP domain patterns from config, used for runtime DNS-based tcpsniff target addition
    http_domains: Vec<String>,
    /// Mailbox for watcher thread to deposit a dynamically-created LogtailExporter
    pending_logtail: Arc<Mutex<Option<Box<dyn GenAIExporter>>>>,
    /// DeadLoop auto-kill: enabled flag
    deadloop_kill_enabled: bool,
    /// DeadLoop auto-kill: trigger threshold (kill after N detections)
    deadloop_kill_after_count: usize,
}

/// GenAI events waiting for session_id resolution via ResponseSessionMapper.
/// If the mapper lookup succeeds within the timeout, session_id metadata is updated
/// before export. Otherwise, the events are exported with the response_id-based
/// fallback (`SHA256("session" + first_response_id)`).
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
    pub fn new(mut config: AgentsightConfig) -> Result<Self> {
        // Load rules from config file only when config_path is set (CLI --config)
        // FFI users provide rules via API, no config file needed.
        let mut config_load_ok = false;
        let mut config_load_err: Option<(PathBuf, anyhow::Error)> = None;
        if let Some(path) = config.config_path.clone() {
            let load_result = if path.exists() {
                config.load_from_file(&path)
            } else {
                match crate::config::ensure_default_agents_config(&path) {
                    Ok(()) => config.load_from_file(&path),
                    Err(e) => Err(e),
                }
            };
            match load_result {
                Ok(()) => config_load_ok = true,
                Err(e) => {
                    config_load_err = Some((path, e));
                    config.cmdline_rules = crate::config::default_cmdline_rules();
                }
            }
        }

        // Init logging after config file load so JSON `log_path` / `verbose` apply.
        config.apply_verbose();

        if config_load_ok {
            if let Some(path) = config.config_path.as_ref() {
                log::info!(
                    "Loaded {} cmdline rule(s), {} https rule(s), {} http target(s) from {:?}",
                    config.cmdline_rules.len(),
                    config.https_rules.len(),
                    config.http_targets.len(),
                    path
                );
            }
        } else if let Some((path, e)) = config_load_err {
            log::warn!("Failed to load config from {path:?}: {e}, using embedded defaults");
        }

        let all_cmdline_rules = config.cmdline_rules.clone();

        // Derive tcp_targets from http_targets (endpoint entries only)
        let mut tcp_targets: Vec<crate::config::TcpTarget> = config
            .http_targets
            .iter()
            .filter_map(|t| match t {
                crate::config::HttpTarget::Endpoint(ep) => Some(ep.clone()),
                crate::config::HttpTarget::Domain(_) => None,
            })
            .collect();

        // Collect http domain patterns for DNS-based resolution
        let http_domains: Vec<String> = config
            .http_targets
            .iter()
            .filter_map(|t| match t {
                crate::config::HttpTarget::Domain(d) => Some(d.clone()),
                crate::config::HttpTarget::Endpoint(_) => None,
            })
            .collect();

        // Startup DNS resolve: exact http domains → IPs → append to tcp_targets
        for domain in &http_domains {
            if domain.contains('*') || domain.contains('?') || domain.contains('[') {
                continue;
            }
            use std::net::ToSocketAddrs;
            match (domain.as_str(), 0u16).to_socket_addrs() {
                Ok(addrs) => {
                    for addr in addrs {
                        if let std::net::IpAddr::V4(ipv4) = addr.ip() {
                            log::info!("http domain resolve: {domain} → {ipv4}");
                            tcp_targets.push(crate::config::TcpTarget {
                                ip: Some(ipv4),
                                port: None,
                            });
                        }
                    }
                }
                Err(e) => {
                    log::warn!("http domain resolve failed for {domain}: {e}");
                }
            }
        }

        // Create probes - agent discovery is handled by AgentScanner via ProcMon events
        let enable_udpdns = !config.https_rules.is_empty() || !http_domains.is_empty();
        let mut probes = Probes::new_with_cgroup_filter(
            &[],
            config.target_uid,
            config.enable_filewatch,
            enable_udpdns,
            &tcp_targets,
            config.cgroup_filter_enabled,
        )
        .context("Failed to create probes")?;

        // Attach procmon for process monitoring
        probes.attach().context("Failed to attach probes")?;

        // Seed cgroup_filter map with pre-configured cgroup inode IDs
        if config.cgroup_filter_enabled && !config.cgroup_ids.is_empty() {
            for &cg_id in &config.cgroup_ids {
                probes
                    .add_traced_cgroup(cg_id)
                    .context("Failed to register cgroup_id")?;
                log::info!("Registered cgroup_id {cg_id}");
            }
        }

        // Create scanner with all rules (allow/deny/https)
        let mut scanner = AgentScanner::from_rules(&all_cmdline_rules, &config.https_rules);
        let existing_agents = scanner.scan();

        // Attach SSL probes to already-running agents
        for agent in &existing_agents {
            Self::attach_process_internal(&mut probes, agent.pid, &agent.agent_info.name);
        }

        // Connection scan: find processes with established connections to https_rules IPs
        let already_traced: HashSet<u32> = existing_agents.iter().map(|a| a.pid).collect();
        let conn_results = if scanner.has_domain_rules() {
            let conn_scanner = crate::discovery::ConnectionScanner::new(&scanner);
            conn_scanner.scan(&already_traced)
        } else {
            Vec::new()
        };

        // Build pid → agent_name cache from existing agents (persists after process exit)
        let mut pid_agent_name_cache = HashMap::new();
        for agent in &existing_agents {
            pid_agent_name_cache.insert(agent.pid, agent.agent_info.name.clone());
        }
        for result in &conn_results {
            let agent_name = format!("domain:{}", result.domain);
            Self::attach_process_internal(&mut probes, result.pid, &agent_name);
            pid_agent_name_cache.insert(result.pid, agent_name);
        }
        if !conn_results.is_empty() {
            log::info!(
                "Connection scan: attached {} process(es) via established connections",
                conn_results.len()
            );
        }

        // Start polling (non-blocking)
        let _poller = probes.run().context("Failed to start probe poller")?;

        // Initialize unified storage based on config
        let storage = Self::create_storage(&config)?;

        // Build GenAI exporters
        let mut genai_exporters: Vec<Box<dyn GenAIExporter>> = Vec::new();
        let mut genai_sqlite_store: Option<Arc<GenAISqliteStore>> = None;
        let sls_activated = Arc::new(AtomicBool::new(false));

        // If config has runtime.sls_logtail_path set, seed the dynamic path
        if let Some(ref sls_path) = config.sls_logtail_path {
            crate::genai::logtail::set_dynamic_logtail_path(sls_path);
        }

        let is_agentic_os = crate::genai::anolisa_release::is_anolisa();

        // Determine if Logtail is currently enabled (env var OR dynamic config)
        let logtail_currently_enabled = crate::genai::logtail::logtail_enabled();

        if is_agentic_os {
            // ── Anolisa OS: always register SQLite ──
            match GenAISqliteStore::new() {
                Ok(store) => {
                    log::info!("SQLite GenAI exporter enabled (agentic os)");
                    let store = Arc::new(store);
                    genai_sqlite_store = Some(Arc::clone(&store));
                    genai_exporters.push(Box::new(store));
                }
                Err(e) => {
                    log::warn!("Failed to initialize SQLite GenAI exporter: {e}");
                }
            }
            // If Logtail is enabled at startup, also register it (dual-write)
            if logtail_currently_enabled {
                if let Some(exporter) = LogtailExporter::new(
                    config.encryption_public_key.as_deref(),
                    config.trace_enabled,
                ) {
                    let uid = crate::genai::instance_id::get_owner_account_id();
                    if uid.is_empty() {
                        anyhow::bail!(
                            "SLS Logtail exporter is enabled but failed to \
                             fetch owner-account-id from ECS metadata service. \
                             Cannot upload logs without uid. Aborting."
                        );
                    }
                    log::info!(
                        "Logtail file exporter enabled (agentic os, {}), uid={}",
                        exporter.path().display(),
                        uid
                    );
                    genai_exporters.push(Box::new(exporter));
                    sls_activated.store(true, Ordering::SeqCst);
                }
            }
        } else {
            // ── Non-Anolisa OS: keep original mutual exclusion behavior ──
            if logtail_currently_enabled {
                if let Some(exporter) = LogtailExporter::new(
                    config.encryption_public_key.as_deref(),
                    config.trace_enabled,
                ) {
                    let uid = crate::genai::instance_id::get_owner_account_id();
                    if uid.is_empty() {
                        anyhow::bail!(
                            "SLS Logtail exporter is enabled (SLS_LOGTAIL_FILE set) but failed to \
                             fetch owner-account-id from ECS metadata service. \
                             Cannot upload logs without uid. Aborting."
                        );
                    }
                    log::info!(
                        "Logtail file exporter enabled ({}), uid={}",
                        exporter.path().display(),
                        uid
                    );
                    genai_exporters.push(Box::new(exporter));
                    sls_activated.store(true, Ordering::SeqCst);
                }
                // Also initialize SQLite store for detection queries + lifecycle
                match GenAISqliteStore::new() {
                    Ok(store) => {
                        log::info!("SQLite GenAI store initialized (detection + lifecycle)");
                        let store = Arc::new(store);
                        genai_sqlite_store = Some(Arc::clone(&store));
                        genai_exporters.push(Box::new(store));
                    }
                    Err(e) => {
                        log::warn!("Failed to initialize SQLite GenAI store: {e}");
                    }
                }
            } else {
                // No Logtail: use local JSONL + SQLite
                genai_exporters.push(Box::new(GenAIStore::new(&GenAIStore::default_path())));

                match GenAISqliteStore::new() {
                    Ok(store) => {
                        log::info!("SQLite GenAI exporter enabled");
                        let store = Arc::new(store);
                        genai_sqlite_store = Some(Arc::clone(&store));
                        genai_exporters.push(Box::new(store));
                    }
                    Err(e) => {
                        log::warn!("Failed to initialize SQLite GenAI exporter: {e}");
                    }
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
                        log::info!("Tokenizer loaded from: {tokenizer_path:?}");
                        Analyzer::with_tokenizer(tokenizer.clone(), tokenizer)
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to load tokenizer from {tokenizer_path:?}: {e}. Using analyzer without tokenizer."
                        );
                        Analyzer::new()
                    }
                }
            } else {
                log::warn!(
                    "Tokenizer file not found: {tokenizer_path:?}. Using analyzer without tokenizer."
                );
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
                    log::info!("Interruption events store initialized at {db_path:?}");
                    Some(Arc::new(store))
                }
                Err(e) => {
                    log::warn!("Failed to initialize interruption store: {e}");
                    None
                }
            }
        };

        // Run OOM recovery: scan dmesg for OOM kill events that occurred while
        // AgentSight was down (e.g. if AgentSight itself was OOM-killed).
        if let Some(ref istore) = interruption_store {
            recover_oom_events(istore, genai_sqlite_store.as_ref(), 0);
        }

        log::info!(
            "AgentSight initialized: {} existing agent(s), {} GenAI exporter(s)",
            existing_agents.len(),
            genai_exporters.len(),
        );

        // Shared mailbox for dynamic LogtailExporter activation
        let pending_logtail: Arc<Mutex<Option<Box<dyn GenAIExporter>>>> =
            Arc::new(Mutex::new(None));

        // Create `running` flag early so background threads can observe shutdown.
        let running = Arc::new(AtomicBool::new(true));

        // Spawn background threads (config watcher, token-collector, stale scanner).
        if let Some(ref cfg_path) = config.config_path {
            crate::background::start_config_watcher(
                cfg_path.clone(),
                Arc::clone(&sls_activated),
                Arc::clone(&pending_logtail),
                config.encryption_public_key.clone(),
                config.trace_enabled,
                Arc::clone(&running),
            );
            crate::background::start_token_collector_watcher(
                cfg_path.clone(),
                Arc::clone(&running),
            );
        }
        if let Some(ref sqlite_store) = genai_sqlite_store {
            crate::background::start_stale_scanner(Arc::clone(sqlite_store), Arc::clone(&running));
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
            running,
            event_count: 0,
            filewatch_callback: None,
            response_mapper: ResponseSessionMapper::new(),
            pending_genai: Vec::new(),
            ffi_sender: None,
            last_drain_check: std::time::Instant::now(),
            pid_agent_name_cache,
            http_domains,
            pending_logtail,
            deadloop_kill_enabled: config.deadloop_kill_enabled,
            deadloop_kill_after_count: config.deadloop_kill_after_count,
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
        log::debug!("Attaching to pid {pid}, agent name: {agent_name}");
        if let Err(e) = probes.add_traced_pid(pid) {
            log::warn!("Failed to add pid {pid} to traced_processes map: {e}");
        }
        if let Err(e) = probes.attach_process(pid as i32) {
            log::error!("Failed to attach SSL probe to pid {pid}: {e}");
        } else {
            log::info!("Attached to agent: {agent_name} (pid={pid})");
        }
    }

    /// Detach SSL probes from a specific agent process
    pub fn detach_process(&mut self, pid: u32, agent_name: &str) {
        log::debug!("Detaching from pid {pid}, agent name: {agent_name}");
        let _ = self.probes.remove_traced_pid(pid).inspect_err(|e| {
            log::debug!("traced pid {pid} already removed from BPF map (expected race with sched_process_exit): {e}");
        });
        self.probes.detach_ssl_probes(pid);
    }

    /// Add a cgroup inode id to the shared BPF cgroup_filter map at runtime.
    /// Delegates to the underlying `Probes` instance.
    pub fn add_traced_cgroup(&mut self, cgroup_id: u64) -> anyhow::Result<()> {
        self.probes.add_traced_cgroup(cgroup_id)
    }

    /// Remove a cgroup inode id from the shared BPF cgroup_filter map at runtime.
    /// Delegates to the underlying `Probes` instance.
    pub fn remove_traced_cgroup(&mut self, cgroup_id: u64) -> anyhow::Result<()> {
        self.probes.remove_traced_cgroup(cgroup_id)
    }

    /// Try to receive and process the next event (non-blocking)
    /// Returns None if no event is available
    pub fn try_process(&mut self) -> Option<u64> {
        if !self.running.load(Ordering::SeqCst) {
            return None;
        }

        let event = self.probes.try_recv()?;
        self.event_count += 1;

        log::trace!("Processing event: {:?}", event.event_type());

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

        // Handle UDP DNS events (domain-based attachment)
        if let Event::UdpDns(ref dns_event) = event {
            log::debug!(
                "[UDP-DNS] pid={} comm={} domain={}",
                dns_event.pid,
                dns_event.comm,
                dns_event.domain
            );

            // HTTPS rules: attach SSL probes to the process
            if self.scanner.on_dns_event(dns_event.pid, &dns_event.domain) {
                log::info!(
                    "[UDP-DNS] Attaching to pid={} via domain rule (domain={})",
                    dns_event.pid,
                    dns_event.domain
                );
                if let Err(e) = self.probes.attach_process(dns_event.pid as i32) {
                    log::warn!("[UDP-DNS] Failed to attach to pid={}: {}", dns_event.pid, e);
                }
            }

            // HTTP domains: resolve DNS domain → IP, add to tcpsniff BPF map
            if crate::discovery::matcher::match_domain_glob(&dns_event.domain, &self.http_domains) {
                use std::net::ToSocketAddrs;
                match (dns_event.domain.as_str(), 0u16).to_socket_addrs() {
                    Ok(addrs) => {
                        for addr in addrs {
                            if let std::net::IpAddr::V4(ipv4) = addr.ip() {
                                log::info!(
                                    "[UDP-DNS] Adding http target {} → {}",
                                    dns_event.domain,
                                    ipv4
                                );
                                let target = crate::config::TcpTarget {
                                    ip: Some(ipv4),
                                    port: None,
                                };
                                if let Err(e) = self.probes.add_tcp_target(&target) {
                                    log::warn!("[UDP-DNS] Failed to add tcp target {ipv4}: {e}");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "[UDP-DNS] DNS resolve failed for http domain {}: {}",
                            dns_event.domain,
                            e
                        );
                    }
                }
            }

            return None;
        }

        // Parse the event
        let result = self.parser.parse_event(event);

        // Process messages through aggregator
        let aggregated_results = self.aggregator.process_result(result);

        // Analyze and store results
        for agg_result in &aggregated_results {
            let mut analysis_results = self.analyzer.analyze_aggregated(agg_result);

            // Build GenAI semantic events AND pending info in one pass
            let (output, pending_info) = self.genai_builder.build_with_pending(
                &analysis_results,
                &self.response_mapper,
                &self.pid_agent_name_cache,
            );

            // Backfill TokenRecord.agent from pid_agent_name_cache, falling back to comm
            for ar in &mut analysis_results {
                if let crate::analyzer::AnalysisResult::Token(t) = ar {
                    if t.agent.is_none() {
                        t.agent = self
                            .pid_agent_name_cache
                            .get(&t.pid)
                            .cloned()
                            .or_else(|| Some(t.comm.clone()));
                    }
                }
            }

            if !output.events.is_empty() {
                if output.pending_response_id.is_some() {
                    // Session_id not yet resolved — queue for deferred resolution.
                    // Write a pending row NOW so crash detection can see this call
                    // during the deferral window (up to PENDING_SESSION_TIMEOUT).
                    if let Some(ref info) = pending_info {
                        if let Some(sqlite_store) = self.genai_sqlite_store.as_ref() {
                            if let Err(e) = sqlite_store.insert_pending(info) {
                                log::warn!(
                                    "Failed to insert deferred pending call {}: {}",
                                    info.call_id,
                                    e
                                );
                            }
                        }
                    } else {
                        log::warn!(
                            "Deferred GenAI call queued without pending_info (response_id={}), crash detection blind spot remains",
                            output.pending_response_id.as_deref().unwrap_or("unknown")
                        );
                    }
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
                                    log::warn!("Failed to complete pending call: {e}");
                                }
                            }
                            // Export to non-SQLite exporters only (SQLite already written)
                            for exporter in &self.genai_exporters {
                                if exporter.name() != "sqlite" {
                                    exporter.export(&output.events);
                                    log::debug!(
                                        "Exported {} GenAI events via '{}'",
                                        output.events.len(),
                                        exporter.name()
                                    );
                                }
                            }
                            if let Some(ref sender) = self.ffi_sender {
                                for event in &output.events {
                                    if let GenAISemanticEvent::LLMCall(call) = event {
                                        sender.send(FfiEvent::Llm(call.clone()));
                                    }
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
                        log::warn!("Failed to store analysis result: {e}");
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
                // Read cmdline for deny-check and custom matching
                let cmdline_args =
                    crate::discovery::scanner::read_cmdline(&format!("/proc/{pid}/cmdline"));

                // Phase 1: check deny rules first (blacklist overrides everything)
                if self.scanner.is_denied(&cmdline_args) {
                    log::debug!("ProcMon: pid={pid} denied by cmdline rule, skipping attach");
                    return;
                }

                // Phase 2: check if this is a known agent and start tracking
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
                    self.handle_agent_crash_detection(*pid, &agent_name);
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
        log::debug!(
            "FileWrite: pid={} file={} size={}",
            event.pid,
            event.filename,
            event.write_size
        );
        self.response_mapper.process_filewrite(event);
    }

    /// Run the event loop (blocking)
    pub fn run(&mut self) -> Result<u64> {
        log::debug!("Agent discovery running via ProcMon events");

        // Main event loop
        while self.running.load(Ordering::SeqCst) {
            if let Some(result) = self.try_process() {
                log::trace!("[Event {result}] Processed");
            } else {
                // No event available — flush any timed-out pending GenAI events
                self.flush_expired_pending_genai();
                // Drain orphaned connections from dead PIDs and persist as pending
                self.drain_and_persist_dead_connections();
                // Check if config watcher deposited a new LogtailExporter
                self.check_pending_logtail();
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
        // Checkpoint genai_events.db WAL so -wal/-shm are cleaned up on exit
        // (mirrors Storage::Drop which checkpoints agentsight.db).
        if let Some(ref store) = self.genai_sqlite_store {
            if let Err(e) = store.wal_checkpoint() {
                log::warn!("GenAI WAL checkpoint on shutdown failed: {e}");
            }
        }
    }

    /// Check and drain the pending_logtail mailbox.
    /// If the config watcher deposited a new LogtailExporter, register it.
    fn check_pending_logtail(&mut self) {
        if let Ok(mut guard) = self.pending_logtail.try_lock() {
            if let Some(exporter) = guard.take() {
                log::info!(
                    "Registering dynamically-activated LogtailExporter: '{}'",
                    exporter.name()
                );
                self.genai_exporters.push(exporter);
            }
        }
    }

    /// Install an FFI event sender for C API mode.
    /// When set, completed events are pushed through this channel.
    /// `pub(crate)` because `FfiEventSender` is a crate-internal type and the
    /// only caller lives in this crate's FFI layer.
    pub(crate) fn set_ffi_sender(&mut self, sender: FfiEventSender) {
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
                log::debug!(
                    "Exported {} GenAI events via '{}'",
                    events.len(),
                    exporter.name()
                );
            }
        }
    }

    /// Complete deferred GenAI events: promote their pending DB rows to
    /// 'complete', then export to non-SQLite exporters (or FFI).
    ///
    /// # Preconditions
    ///
    /// A `status='pending'` row for each event's `call_id` must already exist
    /// in `genai_events` (written by `insert_pending` at queue time).
    ///
    /// This mirrors the immediate path (try_process lines 717-744) but is used
    /// when events were queued in `pending_genai` and are now being drained.
    /// The pending row was written by the deferred-queue entry point; this
    /// method updates it via `complete_pending` and avoids double-writing by
    /// skipping the SQLite exporter in the fan-out.
    fn complete_and_export_deferred_genai(&self, events: &[GenAISemanticEvent]) {
        complete_deferred_genai(
            events,
            self.genai_sqlite_store.as_ref(),
            &self.genai_exporters,
            self.ffi_sender.as_ref(),
        );
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
                        // NOTE: RetryStorm detection only fires when conversation_id is Some.
                        // When None, each error inserts a separate row (no dedup, no storm detect).
                        if let Some(ref cid) = ie.conversation_id {
                            let error_msg = llm_call.error.as_deref();
                            if istore.exists_for_conversation(cid, &ie.interruption_type, error_msg)
                            {
                                log::debug!(
                                    "Skipping duplicate {:?} for conversation_id={} error={:?}",
                                    ie.interruption_type,
                                    cid,
                                    error_msg
                                );
                                // Still stamp the genai_events row so the call is marked
                                if let Some(ref sqlite) = self.genai_sqlite_store {
                                    let _ = sqlite.update_interruption_type(
                                        &llm_call.call_id,
                                        ie.interruption_type.as_str(),
                                    );
                                    // RetryStorm: if >= 5 total calls with same error type in
                                    // this conversation, emit critical alert
                                    let count = sqlite.count_interruption_type_for_conversation(
                                        cid,
                                        ie.interruption_type.as_str(),
                                    );
                                    if count >= 5
                                        && ie.interruption_type
                                            != crate::interruption::InterruptionType::RetryStorm
                                    {
                                        let storm_event =
                                            crate::interruption::InterruptionEvent::new(
                                                crate::interruption::InterruptionType::RetryStorm,
                                                ie.session_id.clone(),
                                                ie.trace_id.clone(),
                                                ie.conversation_id.clone(),
                                                ie.call_id.clone(),
                                                ie.pid,
                                                ie.agent_name.clone(),
                                                llm_call.end_timestamp_ns as i64,
                                                Some(serde_json::json!({
                                                    "repeated_type": ie.interruption_type.as_str(),
                                                    "count": count,
                                                })),
                                            );
                                        if !istore.exists_for_conversation(
                                            cid,
                                            &crate::interruption::InterruptionType::RetryStorm,
                                            None,
                                        ) {
                                            let _ = istore.insert(&storm_event);
                                            log::warn!(
                                                "RetryStorm detected: {} × {:?} in conversation {}",
                                                count,
                                                ie.interruption_type,
                                                cid
                                            );
                                        }
                                    }
                                }
                                continue;
                            }
                        }
                        if let Err(e) = istore.insert(ie) {
                            log::warn!("Failed to store interruption event: {e}");
                        }
                        // Also export to iLogtail file (no-op if SLS_LOGTAIL_FILE unset),
                        // so the SLS index keeps interruption records co-located with LLM calls.
                        crate::genai::logtail::export_interruption_events(std::slice::from_ref(ie));
                        // Also stamp genai_events row with interruption_type
                        if let Some(ref sqlite) = self.genai_sqlite_store {
                            let _ = sqlite.update_interruption_type(
                                &llm_call.call_id,
                                ie.interruption_type.as_str(),
                            );
                        }
                    }

                    // ── Cross-call DeadLoop detection ──────────────────────────────
                    // After single-call detection, check for repetitive patterns
                    // across the conversation's recent calls.
                    if let Some(ref cid) = llm_call.metadata.get("conversation_id") {
                        // When auto-kill is enabled, allow multiple detection events
                        // (up to kill_after_count) so the count threshold can be reached.
                        // When disabled, deduplicate to at most one event per conversation.
                        let existing_count = istore.count_for_conversation(
                            cid,
                            &crate::interruption::InterruptionType::DeadLoop,
                        );
                        let should_detect = if self.deadloop_kill_enabled {
                            existing_count <= self.deadloop_kill_after_count
                        } else {
                            existing_count == 0
                        };

                        if should_detect {
                            if let Some(ref sqlite) = self.genai_sqlite_store {
                                let loop_detector = crate::interruption::LoopDetector::default();
                                let recent = sqlite.get_recent_calls_for_conversation(
                                    cid,
                                    loop_detector.config.window_size,
                                );
                                if let Some(loop_event) = loop_detector.detect(
                                    cid,
                                    llm_call.metadata.get("session_id").map(|s| s.as_str()),
                                    llm_call.agent_name.as_deref(),
                                    Some(llm_call.pid),
                                    llm_call.end_timestamp_ns as i64,
                                    &recent,
                                ) {
                                    let _ = istore.insert(&loop_event);
                                    crate::genai::logtail::export_interruption_events(
                                        std::slice::from_ref(&loop_event),
                                    );
                                    log::warn!(
                                        "DeadLoop detected in conversation {}: {:?}",
                                        cid,
                                        loop_event.detail
                                    );

                                    // ── Auto-kill 止血 ──
                                    if self.deadloop_kill_enabled {
                                        let new_count = existing_count + 1;
                                        if new_count > self.deadloop_kill_after_count {
                                            if let Some(pid) = loop_event.pid {
                                                log::error!(
                                                    "DeadLoop auto-kill: escalating to SIGKILL for pid {pid} (conversation={cid}, detections={new_count})"
                                                );
                                                let ret = unsafe { libc::kill(pid, libc::SIGKILL) };
                                                if ret != 0 {
                                                    let err = std::io::Error::last_os_error();
                                                    log::error!(
                                                        "DeadLoop auto-kill: SIGKILL failed for pid {pid}: {err}"
                                                    );
                                                }
                                            }
                                        } else if new_count == self.deadloop_kill_after_count {
                                            if let Some(pid) = loop_event.pid {
                                                log::error!(
                                                    "DeadLoop auto-kill: sending SIGTERM to pid {pid} (conversation={cid}, detections={new_count})"
                                                );
                                                let ret = unsafe { libc::kill(pid, libc::SIGTERM) };
                                                if ret != 0 {
                                                    let err = std::io::Error::last_os_error();
                                                    log::error!(
                                                        "DeadLoop auto-kill: SIGTERM failed for pid {pid}: {err}"
                                                    );
                                                }
                                            }
                                        } else {
                                            log::warn!(
                                                "DeadLoop auto-kill: detection {}/{} for conversation {}, waiting...",
                                                new_count,
                                                self.deadloop_kill_after_count,
                                                cid
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Immediate crash detection when a tracked agent process exits.
    ///
    /// Called from `ProcMon::Exit` handler. Drains in-flight connections for
    /// the PID, persists them as pending calls, then generates an `agent_crash`
    /// interruption event if any pending calls exist.
    fn handle_agent_crash_detection(&mut self, pid: u32, agent_name: &str) {
        use crate::aggregator::ConnectionState;
        use crate::interruption::{InterruptionEvent, InterruptionType, was_pid_oom_killed};

        // 1. Drain in-flight connections for this PID from the aggregator
        let drained = self.aggregator.drain_connections_for_pid(pid);

        // 2. Persist drained connections as pending calls
        for (conn_id, state) in &drained {
            let (_state_name, request) = match state {
                ConnectionState::RequestPending { request } => ("RequestPending", request),
                ConnectionState::SseActive {
                    request: Some(req), ..
                } => ("SseActive", req),
                _ => continue,
            };

            if let Some(pending) = self.genai_builder.build_pending_from_request(
                request,
                conn_id,
                &self.pid_agent_name_cache,
            ) {
                if let Some(ref store) = self.genai_sqlite_store {
                    if let Err(e) = store.insert_pending(&pending) {
                        log::warn!("[CrashDetect] Failed to persist pending call: {e}");
                    }
                }
            }
        }

        // 3. Query all pending calls for this PID (including any persisted earlier)
        let pending_calls = if let Some(ref store) = self.genai_sqlite_store {
            store
                .list_pending_for_pids(&[pid as i32])
                .unwrap_or_default()
        } else {
            vec![]
        };

        if pending_calls.is_empty() {
            log::debug!(
                "[CrashDetect] Agent {agent_name} (pid={pid}) exited with no pending calls — normal shutdown",
            );
            return;
        }

        // 4. Generate agent_crash interruption event
        if let Some(ref istore) = self.interruption_store {
            let now_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);

            let is_oom = was_pid_oom_killed(pid as i32);

            // Group by (session_id, conversation_id) to produce one event per conversation
            let mut by_conv: std::collections::HashMap<
                (Option<String>, Option<String>),
                Vec<String>,
            > = std::collections::HashMap::new();
            for (call_id, session_id, _trace_id, conversation_id) in &pending_calls {
                by_conv
                    .entry((session_id.clone(), conversation_id.clone()))
                    .or_default()
                    .push(call_id.clone());
            }

            for ((session_id, conversation_id), call_ids) in &by_conv {
                let mut detail = serde_json::json!({
                    "pid": pid,
                    "agent_name": agent_name,
                    "call_ids": call_ids,
                    "source": "trace_procmon_exit",
                });
                if is_oom {
                    detail["oom"] = serde_json::json!(true);
                }
                let event = InterruptionEvent::new(
                    InterruptionType::AgentCrash,
                    session_id.clone(),
                    None,
                    conversation_id.clone(),
                    None,
                    Some(pid as i32),
                    Some(agent_name.to_string()),
                    now_ns,
                    Some(detail),
                );
                if let Err(e) = istore.insert(&event) {
                    log::warn!("[CrashDetect] Failed to record agent_crash for pid={pid}: {e}");
                } else {
                    log::info!(
                        "[CrashDetect] Recorded agent_crash for {} (pid={}, session={:?}, conversation={:?}, {} call(s), oom={})",
                        agent_name,
                        pid,
                        session_id,
                        conversation_id,
                        call_ids.len(),
                        is_oom,
                    );
                }
                crate::genai::logtail::export_interruption_events(std::slice::from_ref(&event));
            }

            // Mark all pending calls for this PID as interrupted
            if let Some(ref store) = self.genai_sqlite_store {
                let itype = if is_oom { "oom_crash" } else { "agent_crash" };
                if let Err(e) = store.mark_pending_interrupted_for_pid(pid as i32, itype) {
                    log::warn!(
                        "[CrashDetect] Failed to mark pending interrupted for pid={pid}: {e}"
                    );
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

        // Track persisted pending calls: (pid, call_id, session_id, agent_name, conversation_id)
        let mut persisted_pending: Vec<(
            u32,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        )> = Vec::new();

        for (conn_id, state) in drained {
            // Destructure to capture both request AND sse_events
            let (_state_name, request, sse_events) = match state {
                ConnectionState::RequestPending { request } => ("RequestPending", request, vec![]),
                ConnectionState::SseActive {
                    request: Some(req),
                    sse_events,
                    ..
                } => ("SseActive", req, sse_events),
                _ => continue,
            };

            if let Some(pending) = self.genai_builder.build_pending_from_request(
                &request,
                &conn_id,
                &self.pid_agent_name_cache,
            ) {
                if let Some(ref store) = self.genai_sqlite_store {
                    let call_id = pending.call_id.clone();
                    let pid = pending.pid;

                    if let Err(e) = store.insert_pending(&pending) {
                        log::warn!("[DrainCheck] FAIL persist: {e}");
                        continue;
                    }
                    // Track for OOM detection below
                    persisted_pending.push((
                        conn_id.pid,
                        pending.call_id.clone(),
                        pending.session_id.clone(),
                        pending.agent_name.clone(),
                        pending.conversation_id.clone(),
                    ));
                    // ── Session ID reconciliation ──────────────────────────
                    // The drain path computes session_id via the response_id
                    // domain-separated hash fallback (`SHA256("session"+rid)`),
                    // but normal flow uses ResponseSessionMapper (agent .jsonl UUID).
                    // Look up the real session_id from completed records for the same PID.
                    match store.lookup_session_for_pid(pid) {
                        Ok(Some(ref real_session_id)) => {
                            if pending.session_id.as_deref() != Some(real_session_id.as_str()) {
                                if let Err(e) = store.update_session_id(&call_id, real_session_id) {
                                    log::warn!("[DrainCheck] FAIL update session_id: {e}");
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            log::warn!("[DrainCheck] FAIL lookup session: {e}");
                        }
                    }

                    // ── SSE enrichment ────────────────────────────────────
                    // Parse captured SSE events for model, trace_id, tokens, output content
                    if !sse_events.is_empty() {
                        if let Some(mut enrichment) =
                            GenAIBuilder::extract_sse_enrichment(&sse_events)
                        {
                            // If SSE didn't carry usage data (stream was interrupted before
                            // the final chunk), compute tokens via the real tokenizer.
                            if enrichment.input_tokens.is_none()
                                || enrichment.output_tokens.is_none()
                            {
                                let model_name = enrichment
                                    .model
                                    .as_deref()
                                    .or(pending.model.as_deref())
                                    .unwrap_or("unknown");
                                if let Ok(tokenizer) =
                                    crate::tokenizer::get_global_tokenizer(model_name)
                                {
                                    // ── input tokens ──
                                    if enrichment.input_tokens.is_none() {
                                        if let Some(body) = request.json_body() {
                                            if let Some(messages) =
                                                body.get("messages").and_then(|m| m.as_array())
                                            {
                                                let mut msgs = messages.clone();
                                                // Parse tool_calls.arguments from string to object
                                                for msg in msgs.iter_mut() {
                                                    if let Some(tcs) = msg
                                                        .get_mut("tool_calls")
                                                        .and_then(|tc| tc.as_array_mut())
                                                    {
                                                        for tc in tcs.iter_mut() {
                                                            if let Some(f) = tc.get_mut("function")
                                                            {
                                                                if let Some(a) = f
                                                                    .get("arguments")
                                                                    .and_then(|a| a.as_str())
                                                                {
                                                                    if let Ok(p) =
                                                                        serde_json::from_str::<
                                                                            serde_json::Value,
                                                                        >(
                                                                            a
                                                                        )
                                                                    {
                                                                        f["arguments"] = p;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                let tools_json: Option<Vec<serde_json::Value>> =
                                                    body.get("tools")
                                                        .and_then(|t| t.as_array())
                                                        .map(|a| a.to_vec());
                                                let count = match tokenizer
                                                    .apply_chat_template_with_tools(
                                                        &msgs,
                                                        tools_json.as_deref(),
                                                        true,
                                                    ) {
                                                    Ok(formatted) => {
                                                        tokenizer.count(&formatted).unwrap_or(0)
                                                    }
                                                    Err(_) => {
                                                        // Fallback: raw message count
                                                        msgs.iter()
                                                            .filter_map(|m| {
                                                                serde_json::to_string(m).ok()
                                                            })
                                                            .map(|s| {
                                                                tokenizer.count(&s).unwrap_or(0)
                                                            })
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
                                                if let Some((content, reasoning, tool_calls)) =
                                                    extract_response_content(Some(&chunk))
                                                {
                                                    if !content.is_empty() {
                                                        all_content.push_str(&content);
                                                    }
                                                    if let Some(r) = reasoning {
                                                        if !r.is_empty() {
                                                            all_reasoning.push_str(&r);
                                                        }
                                                    }
                                                    for tc in tool_calls {
                                                        if !tc.is_empty() {
                                                            all_tool_calls.push(tc);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        let mut total = 0usize;
                                        if !all_reasoning.is_empty() {
                                            let wrapped =
                                                format!("<think>\n{all_reasoning}\n</think>\n\n");
                                            total += tokenizer.count(&wrapped).unwrap_or(0);
                                        }
                                        if !all_content.is_empty() {
                                            total += tokenizer.count(&all_content).unwrap_or(0);
                                        }
                                        if !all_tool_calls.is_empty() {
                                            total += tokenizer
                                                .count(&all_tool_calls.join(""))
                                                .unwrap_or(0);
                                        }
                                        if total > 0 {
                                            enrichment.output_tokens = Some(total as i64);
                                        }
                                    }
                                } else {
                                    log::warn!(
                                        "[DrainCheck] tokenizer unavailable for model {:?}, skipping token computation",
                                        enrichment.model.as_deref().or(pending.model.as_deref())
                                    );
                                }
                            }
                            if let Err(e) = store.enrich_pending_from_sse(&call_id, &enrichment) {
                                log::warn!("[DrainCheck] FAIL enrich SSE: {e}");
                            }
                        }
                    }
                }
            } else {
                log::debug!(
                    "[DrainCheck] build_pending returned None: pid={} path={} body_len={}",
                    conn_id.pid,
                    request.path,
                    request.body_len
                );
            }
        }

        // ── OOM detection for dead PIDs ──────────────────────────────────────
        // After persisting pending calls for dead PIDs, check if any were OOM-killed.
        // This runs in the trace process (every 1s) and catches OOM events much faster
        // than the HealthChecker (30s cycle in serve process).
        if !persisted_pending.is_empty() {
            if let Some(ref istore) = self.interruption_store {
                use crate::interruption::{
                    InterruptionEvent, InterruptionType, was_pid_oom_killed,
                };

                let now_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as i64)
                    .unwrap_or(0);

                let mut checked_pids: HashSet<u32> = HashSet::new();
                for (pid, _call_id, session_id, agent_name, conversation_id) in &persisted_pending {
                    if !checked_pids.insert(*pid) {
                        continue; // already checked this PID
                    }
                    if was_pid_oom_killed(*pid as i32) {
                        let call_ids: Vec<&str> = persisted_pending
                            .iter()
                            .filter(|(p, _, _, _, _)| *p == *pid)
                            .map(|(_, c, _, _, _)| c.as_str())
                            .collect();
                        log::info!(
                            "[DrainCheck] PID {} was OOM-killed (confirmed via dmesg), agent={}, calls={:?}",
                            pid,
                            agent_name.as_deref().unwrap_or("unknown"),
                            call_ids
                        );
                        let detail = serde_json::json!({
                            "pid": pid,
                            "agent_name": agent_name,
                            "call_ids": call_ids,
                            "oom": true,
                            "source": "drain+dmesg",
                        });
                        let event = InterruptionEvent::new(
                            InterruptionType::AgentCrash,
                            session_id.clone(),
                            None,
                            conversation_id.clone(),
                            None,
                            Some(*pid as i32),
                            agent_name.clone(),
                            now_ns,
                            Some(detail),
                        );
                        if let Err(e) = istore.insert(&event) {
                            log::warn!(
                                "[DrainCheck] Failed to record OOM agent_crash for pid={pid}: {e}"
                            );
                        } else {
                            log::info!("[DrainCheck] Recorded OOM agent_crash for pid={pid}");
                        }
                        // Mark all pending calls for this PID as interrupted
                        if let Some(ref store) = self.genai_sqlite_store {
                            if let Err(e) =
                                store.mark_pending_interrupted_for_pid(*pid as i32, "oom_crash")
                            {
                                log::warn!(
                                    "[DrainCheck] Failed to mark pending interrupted for pid={pid}: {e}"
                                );
                            }
                        }
                    }
                }
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
            if let Some(session_id) = self
                .response_mapper
                .get_session_by_response_id(&pending.response_id)
                .map(|s| s.to_string())
            {
                // Resolved — update session_id in all event metadata
                log::debug!(
                    "Deferred session_id resolved: response_id={} → session_id={}",
                    pending.response_id,
                    session_id
                );
                for event in &mut pending.events {
                    if let GenAISemanticEvent::LLMCall(call) = event {
                        call.metadata
                            .insert("session_id".to_string(), session_id.clone());
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
            self.complete_and_export_deferred_genai(events);
            self.detect_and_store_interruptions(events);
        }
    }

    /// Flush any pending GenAI events that have exceeded the timeout.
    /// Called during idle periods of the event loop.
    pub fn flush_expired_pending_genai(&mut self) {
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
            self.complete_and_export_deferred_genai(events);
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
            self.complete_and_export_deferred_genai(&pending.events);
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

/// Complete deferred GenAI events: promote pending DB rows to 'complete',
/// then export to non-SQLite exporters (or FFI).
///
/// Extracted as a free function so the persistence policy is unit-testable
/// without constructing a full `AgentSight` instance.
fn complete_deferred_genai(
    events: &[GenAISemanticEvent],
    sqlite_store: Option<&Arc<GenAISqliteStore>>,
    exporters: &[Box<dyn GenAIExporter>],
    ffi_sender: Option<&FfiEventSender>,
) {
    if let Some(store) = sqlite_store {
        for event in events {
            if let Err(e) = store.complete_pending(event) {
                log::warn!("Failed to complete deferred pending call: {e}");
            }
        }
        if let Some(sender) = ffi_sender {
            for event in events {
                if let GenAISemanticEvent::LLMCall(call) = event {
                    sender.send(FfiEvent::Llm(call.clone()));
                }
            }
        } else {
            for exporter in exporters {
                if exporter.name() != "sqlite" {
                    exporter.export(events);
                }
            }
        }
    } else {
        // No SQLite store — export to all exporters (or FFI)
        if let Some(sender) = ffi_sender {
            for event in events {
                if let GenAISemanticEvent::LLMCall(call) = event {
                    sender.send(FfiEvent::Llm(call.clone()));
                }
            }
        } else {
            for exporter in exporters {
                exporter.export(events);
                log::debug!(
                    "Exported {} GenAI events via '{}'",
                    events.len(),
                    exporter.name()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Generate a unique temp directory for each test invocation.
    fn unique_tmp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let pid = std::process::id();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("agentsight-tc-{pid}-{tag}-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    // ── Tests for complete_deferred_genai + complete_pending guard ──

    /// Stub exporter that records exported events for assertion.
    struct RecordingExporter {
        name: String,
        events: std::sync::Mutex<Vec<GenAISemanticEvent>>,
    }

    impl RecordingExporter {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                events: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl GenAIExporter for RecordingExporter {
        fn name(&self) -> &str {
            &self.name
        }
        fn export(&self, events: &[GenAISemanticEvent]) {
            self.events.lock().unwrap().extend_from_slice(events);
        }
    }

    fn make_test_llm_call(call_id: &str) -> crate::genai::LLMCall {
        use crate::genai::semantic::{LLMRequest, LLMResponse};
        crate::genai::LLMCall {
            call_id: call_id.to_string(),
            start_timestamp_ns: 1_000_000_000,
            end_timestamp_ns: 2_000_000_000,
            duration_ns: 1_000_000_000,
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            request: LLMRequest {
                messages: vec![],
                temperature: None,
                max_tokens: None,
                frequency_penalty: None,
                presence_penalty: None,
                top_p: None,
                top_k: None,
                seed: None,
                stop_sequences: None,
                stream: false,
                tools: None,
                raw_body: None,
            },
            response: LLMResponse {
                messages: vec![],
                streamed: false,
                raw_body: None,
            },
            token_usage: None,
            error: None,
            pid: 1234,
            process_name: "test".to_string(),
            agent_name: Some("test-agent".to_string()),
            metadata: HashMap::new(),
        }
    }

    fn make_test_pending_info(call_id: &str) -> crate::storage::sqlite::genai::PendingCallInfo {
        crate::storage::sqlite::genai::PendingCallInfo {
            call_id: call_id.to_string(),
            trace_id: None,
            conversation_id: None,
            session_id: None,
            start_timestamp_ns: 1_000_000_000,
            pid: 1234,
            process_name: "test".to_string(),
            agent_name: Some("test-agent".to_string()),
            http_method: Some("POST".to_string()),
            http_path: Some("/v1/chat/completions".to_string()),
            input_messages: None,
            system_instructions: None,
            user_query: None,
            is_sse: false,
            model: Some("gpt-4".to_string()),
            provider: Some("openai".to_string()),
        }
    }

    #[test]
    fn test_complete_pending_skips_insert_when_row_already_interrupted() {
        let dir = unique_tmp_dir("cp-interrupted");
        let db_path = dir.join("genai_events.db");
        let store = Arc::new(GenAISqliteStore::new_with_path(&db_path).expect("create test store"));

        let info = make_test_pending_info("call-1");
        store.insert_pending(&info).expect("insert_pending");

        // Simulate crash detection marking it as interrupted
        store
            .mark_pending_interrupted_for_pid(1234, "agent_crash")
            .expect("mark interrupted");

        // Now complete_pending should NOT create a duplicate row
        let event = GenAISemanticEvent::LLMCall(make_test_llm_call("call-1"));
        store.complete_pending(&event).expect("complete_pending");

        // Verify: exactly 1 row, status = interrupted (not a second 'complete' row)
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM genai_events WHERE call_id = 'call-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "should have exactly 1 row, not 2 (double-write)");

        let status: String = conn
            .query_row(
                "SELECT status FROM genai_events WHERE call_id = 'call-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "interrupted");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_complete_pending_fallback_inserts_when_no_row_exists() {
        let dir = unique_tmp_dir("cp-fallback");
        let db_path = dir.join("genai_events.db");
        let store = Arc::new(GenAISqliteStore::new_with_path(&db_path).expect("create test store"));

        // No insert_pending — simulate DB restart scenario
        let event = GenAISemanticEvent::LLMCall(make_test_llm_call("call-2"));
        store.complete_pending(&event).expect("complete_pending");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM genai_events WHERE call_id = 'call-2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "fallback INSERT should create exactly 1 row");

        let status: String = conn
            .query_row(
                "SELECT status FROM genai_events WHERE call_id = 'call-2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "complete");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_complete_deferred_genai_promotes_pending_and_exports_non_sqlite() {
        let dir = unique_tmp_dir("deferred-export");
        let db_path = dir.join("genai_events.db");
        let store = Arc::new(GenAISqliteStore::new_with_path(&db_path).expect("create test store"));

        // Insert a pending row
        let info = make_test_pending_info("call-3");
        store.insert_pending(&info).expect("insert_pending");

        // Build event + exporters
        let event = GenAISemanticEvent::LLMCall(make_test_llm_call("call-3"));
        let recorder = RecordingExporter::new("test-recorder");
        let sqlite_exporter = RecordingExporter::new("sqlite");
        let exporters: Vec<Box<dyn GenAIExporter>> =
            vec![Box::new(recorder), Box::new(sqlite_exporter)];

        // Call the free function
        complete_deferred_genai(&[event], Some(&store), &exporters, None);

        // DB row should be promoted to 'complete'
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let status: String = conn
            .query_row(
                "SELECT status FROM genai_events WHERE call_id = 'call-3'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "complete");

        // test-recorder should have received the event; sqlite exporter should NOT
        // (We can't inspect after move, but the function skips name()=="sqlite")
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM genai_events WHERE call_id = 'call-3'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "exactly 1 row (no double-write from sqlite exporter)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_complete_deferred_genai_no_sqlite_exports_to_all() {
        let event = GenAISemanticEvent::LLMCall(make_test_llm_call("call-4"));
        let exporters: Vec<Box<dyn GenAIExporter>> =
            vec![Box::new(RecordingExporter::new("test-recorder"))];

        complete_deferred_genai(&[event], None, &exporters, None);
    }
}
