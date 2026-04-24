//! Background health check loop
//!
//! Periodically scans for agent processes, detects their listening ports,
//! and probes them via HTTP to determine health status.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::discovery::AgentScanner;
use crate::storage::sqlite::{GenAISqliteStore, InterruptionStore};
use crate::interruption::{InterruptionEvent, InterruptionType};
use super::port_detector::detect_listening_ports;
use super::store::{AgentHealthState, AgentHealthStatus, HealthStore, now_ms};

/// Background health checker that periodically probes discovered agents
pub struct HealthChecker {
    store: Arc<RwLock<HealthStore>>,
    interval: Duration,
    http_timeout: Duration,
    /// Optional interruption store for writing agent_crash events
    interruption_store: Option<Arc<InterruptionStore>>,
    /// Optional GenAI store for querying pending calls and marking them interrupted
    genai_store: Option<Arc<GenAISqliteStore>>,
}

impl HealthChecker {
    /// Create a new HealthChecker.
    ///
    /// # Arguments
    /// * `store` - Shared health store for writing results
    /// * `interval` - Time between health check cycles
    pub fn new(store: Arc<RwLock<HealthStore>>, interval: Duration) -> Self {
        Self {
            store,
            interval,
            http_timeout: Duration::from_secs(5),
            interruption_store: None,
            genai_store: None,
        }
    }

    /// Create with an interruption store so offline events trigger `agent_crash`.
    pub fn with_interruption_store(
        mut self,
        interruption_store: Arc<InterruptionStore>,
    ) -> Self {
        self.interruption_store = Some(interruption_store);
        self
    }

    /// Create with a GenAI store to query pending calls on agent crash.
    pub fn with_genai_store(
        mut self,
        genai_store: Arc<GenAISqliteStore>,
    ) -> Self {
        self.genai_store = Some(genai_store);
        self
    }

    /// Spawn the health check loop on a background thread.
    ///
    /// Returns the `JoinHandle` — the thread runs until the process exits.
    pub fn start(self) -> thread::JoinHandle<()> {
        thread::spawn(move || self.run())
    }

    /// Main health check loop (blocking).
    fn run(self) {
        log::info!(
            "Health checker started: interval={}s, http_timeout={}s",
            self.interval.as_secs(),
            self.http_timeout.as_secs(),
        );

        // Do an initial check immediately, then loop with interval
        loop {
            self.check_once();
            thread::sleep(self.interval);
        }
    }

    /// Perform a single health check cycle for all discovered agents.
    fn check_once(&self) {
        let mut scanner = AgentScanner::new();
        let agents = scanner.scan();

        let active_pids: HashSet<u32> = agents.iter().map(|a| a.pid).collect();

        // Mark gone processes as Offline (instead of deleting immediately)
        let newly_offline = if let Ok(mut store) = self.store.write() {
            store.last_scan_time = now_ms();
            store.mark_stale_offline(&active_pids)
        } else {
            vec![]
        };

        // Write agent_crash interruption events for processes that just went offline
        if !newly_offline.is_empty() {
            if let Some(ref istore) = self.interruption_store {
                let now_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as i64)
                    .unwrap_or(0);

                for offline in &newly_offline {
                    // Query pending calls for this pid from genai_events
                    // and mark them as interrupted + create linked InterruptionEvent
                    let pending_calls = self.get_pending_calls_for_pid(offline.pid);
                    if !pending_calls.is_empty() {
                        for (call_id, session_id, trace_id) in &pending_calls {
                            // Create InterruptionEvent linked to the pending call
                            let detail = serde_json::json!({
                                "pid": offline.pid,
                                "agent_name": offline.agent_name,
                                "exe_path": offline.exe_path.clone(),
                                "call_id": call_id,
                            });
                            let event = InterruptionEvent::new(
                                InterruptionType::AgentCrash,
                                session_id.clone(),
                                trace_id.clone(),
                                Some(call_id.clone()),
                                Some(offline.pid as i32),
                                Some(offline.agent_name.clone()),
                                now_ns,
                                Some(detail),
                            );
                            if let Err(e) = istore.insert(&event) {
                                log::warn!("Failed to record agent_crash interruption for pid={}: {}", offline.pid, e);
                            } else {
                                log::info!("Recorded agent_crash interruption for {} (pid={}, call_id={})",
                                    offline.agent_name, offline.pid, call_id);
                            }
                        }
                        // Mark all pending calls for this pid as interrupted in genai_events
                        self.mark_pending_interrupted(offline.pid, "agent_crash");
                    } else {
                        // No pending calls — check if any agentic loop was cut short:
                        // the last LLM call for a session finished with finish_reason=tool_calls,
                        // meaning the agent was still thinking when it crashed.
                        let incomplete = self.get_incomplete_agentic_sessions_for_pid(offline.pid);
                        if !incomplete.is_empty() {
                            for (call_id, session_id, trace_id) in &incomplete {
                                let detail = serde_json::json!({
                                    "pid": offline.pid,
                                    "agent_name": offline.agent_name,
                                    "exe_path": offline.exe_path.clone(),
                                    "call_id": call_id,
                                    "reason": "agentic_loop_incomplete",
                                });
                                let event = InterruptionEvent::new(
                                    InterruptionType::AgentCrash,
                                    session_id.clone(),
                                    trace_id.clone(),
                                    Some(call_id.clone()),
                                    Some(offline.pid as i32),
                                    Some(offline.agent_name.clone()),
                                    now_ns,
                                    Some(detail),
                                );
                                if let Err(e) = istore.insert(&event) {
                                    log::warn!("Failed to record agent_crash (incomplete loop) for pid={}: {}", offline.pid, e);
                                } else {
                                    log::info!("Recorded agent_crash (incomplete agentic loop) for {} (pid={}, call_id={})",
                                        offline.agent_name, offline.pid, call_id);
                                }
                            }
                        } else {
                            // Truly no in-flight activity — still record crash for observability
                            let detail = serde_json::json!({
                                "pid": offline.pid,
                                "agent_name": offline.agent_name,
                                "exe_path": offline.exe_path,
                                "last_check_time": offline.last_check_time,
                            });
                            let event = InterruptionEvent::new(
                                InterruptionType::AgentCrash,
                                None,
                                None,
                                None,
                                Some(offline.pid as i32),
                                Some(offline.agent_name.clone()),
                                now_ns,
                                Some(detail),
                            );
                            if let Err(e) = istore.insert(&event) {
                                log::warn!("Failed to record agent_crash interruption for pid={}: {}", offline.pid, e);
                            } else {
                                log::info!("Recorded agent_crash interruption for {} (pid={}, no pending call)",
                                    offline.agent_name, offline.pid);
                            }
                        }
                    }
                }
            }
        }

        log::debug!("Health check: found {} agent(s)", agents.len());

        for agent in &agents {
            let ports = detect_listening_ports(agent.pid);
            // 构造重启命令：exe + 原始 cmdline args
            let restart_cmd = build_restart_cmd(&agent.exe_path, &agent.cmdline_args);
            let status = if ports.is_empty() {
                AgentHealthStatus {
                    pid: agent.pid,
                    agent_name: agent.agent_info.name.clone(),
                    category: agent.agent_info.category.clone(),
                    exe_path: agent.exe_path.clone(),
                    ports: vec![],
                    status: AgentHealthState::NoPort,
                    last_check_time: now_ms(),
                    latency_ms: None,
                    error_message: None,
                    restart_cmd: Some(restart_cmd),
                }
            } else {
                self.probe_agent(agent, &ports, restart_cmd)
            };

            if let Ok(mut store) = self.store.write() {
                store.update(agent.pid, status);
            }
        }
    }

    /// Probe an agent's ports via HTTP and return its health status.
    ///
    /// Tries each port in order.
    /// - 2xx/3xx/4xx/5xx response         → Healthy
    /// - Read timeout (process is hung)   → Hung
    /// - Connection refused / unreachable → Unhealthy
    fn probe_agent(
        &self,
        agent: &crate::discovery::DiscoveredAgent,
        ports: &[u16],
        restart_cmd: Vec<String>,
    ) -> AgentHealthStatus {
        let mut last_error = String::new();
        // 标记是否遇到了超时错误（区分 hung vs unreachable）
        let mut timed_out = false;

        for &port in ports {
            let url = format!("http://127.0.0.1:{}/", port);
            let start = Instant::now();

            let result = ureq::AgentBuilder::new()
                .timeout_connect(self.http_timeout)
                .timeout_read(self.http_timeout)
                .build()
                .get(&url)
                .call();

            let latency = start.elapsed().as_millis() as u64;

            match result {
                Ok(_) => {
                    return AgentHealthStatus {
                        pid: agent.pid,
                        agent_name: agent.agent_info.name.clone(),
                        category: agent.agent_info.category.clone(),
                        exe_path: agent.exe_path.clone(),
                        ports: ports.to_vec(),
                        status: AgentHealthState::Healthy,
                        last_check_time: now_ms(),
                        latency_ms: Some(latency),
                        error_message: None,
                        restart_cmd: Some(restart_cmd),
                    };
                }
                Err(ureq::Error::Status(_code, _resp)) => {
                    // 非 2xx 响应 — 进程仍在响应
                    return AgentHealthStatus {
                        pid: agent.pid,
                        agent_name: agent.agent_info.name.clone(),
                        category: agent.agent_info.category.clone(),
                        exe_path: agent.exe_path.clone(),
                        ports: ports.to_vec(),
                        status: AgentHealthState::Healthy,
                        last_check_time: now_ms(),
                        latency_ms: Some(latency),
                        error_message: None,
                        restart_cmd: Some(restart_cmd),
                    };
                }
                Err(ureq::Error::Transport(e)) => {
                    let msg = e.to_string();
                    // ureq 的读超时 / 写超时消息均包含 "timed out"
                    if msg.to_lowercase().contains("timed out") {
                        timed_out = true;
                        last_error = format!("响应超时 ({}ms): {}", latency, msg);
                    } else {
                        last_error = msg.clone();
                    }
                    log::debug!(
                        "Health probe failed for {} (pid={}) on port {}: {}",
                        agent.agent_info.name, agent.pid, port, msg
                    );
                }
            }
        }

        // 所有端口均失败 — 根据错误类型决定状态
        let status = if timed_out {
            AgentHealthState::Hung
        } else {
            AgentHealthState::Unhealthy
        };

        AgentHealthStatus {
            pid: agent.pid,
            agent_name: agent.agent_info.name.clone(),
            category: agent.agent_info.category.clone(),
            exe_path: agent.exe_path.clone(),
            ports: ports.to_vec(),
            status,
            last_check_time: now_ms(),
            latency_ms: None,
            error_message: Some(last_error),
            restart_cmd: Some(restart_cmd),
        }
    }

    /// Query pending LLM calls for a specific PID from genai_events.
    ///
    /// Returns a list of (call_id, session_id, trace_id) tuples.
    fn get_pending_calls_for_pid(&self, pid: u32) -> Vec<(String, Option<String>, Option<String>)> {
        if let Some(ref genai_store) = self.genai_store {
            match genai_store.list_pending_for_pid(pid as i32) {
                Ok(calls) => calls,
                Err(e) => {
                    log::warn!("Failed to query pending calls for pid={}: {}", pid, e);
                    vec![]
                }
            }
        } else {
            vec![]
        }
    }

    /// Query sessions whose agentic loop was incomplete for a specific PID.
    ///
    /// Returns (call_id, session_id, conversation_id) for the last call of each
    /// (session, conversation) that ended with finish_reason=tool_calls,
    /// indicating the agent was mid-loop when it crashed.  Only considers calls
    /// from the last 5 minutes to avoid false positives from historical data.
    fn get_incomplete_agentic_sessions_for_pid(
        &self,
        pid: u32,
    ) -> Vec<(String, Option<String>, Option<String>)> {
        if let Some(ref genai_store) = self.genai_store {
            // 5-minute lookback window (in nanoseconds)
            let since_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64 - 300_000_000_000i64)
                .unwrap_or(0);
            match genai_store.list_incomplete_agentic_sessions_for_pid(pid as i32, since_ns) {
                Ok(calls) => calls,
                Err(e) => {
                    log::warn!("Failed to query incomplete agentic sessions for pid={}: {}", pid, e);
                    vec![]
                }
            }
        } else {
            vec![]
        }
    }

    /// Mark all pending calls for a PID as interrupted in genai_events.
    fn mark_pending_interrupted(&self, pid: u32, itype: &str) {
        if let Some(ref genai_store) = self.genai_store {
            if let Err(e) = genai_store.mark_pending_interrupted_for_pid(pid as i32, itype) {
                log::warn!("Failed to mark pending calls as interrupted for pid={}: {}", pid, e);
            }
        }
    }
}

/// 构造重启命令向量：[exe, arg1, arg2, ...]
///
/// 过滤掉空字符串，保证命令有效。
fn build_restart_cmd(exe_path: &str, cmdline_args: &[String]) -> Vec<String> {
    let mut cmd = vec![exe_path.to_string()];
    // cmdline_args[0] 通常是 exe 本身（argv[0]），跳过以避免重复
    let args: Vec<_> = cmdline_args.iter()
        .skip(1)
        .filter(|a| !a.is_empty())
        .cloned()
        .collect();
    cmd.extend(args);
    cmd
}
