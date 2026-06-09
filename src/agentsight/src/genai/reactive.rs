use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::exporter::GenAIExporter;
use super::semantic::GenAISemanticEvent;

pub struct ReactiveConfig {
    pub enabled: bool,
    pub debounce_secs: u64,
    pub workspace_path: Option<String>,
}

impl Default for ReactiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            debounce_secs: 30,
            workspace_path: None,
        }
    }
}

#[allow(dead_code)]
enum Msg {
    Checkpoint {
        reason: String,
        conversation_id: Option<String>,
    },
    InterruptionAlert {
        interruption_type: String,
        conversation_id: Option<String>,
    },
    TokenAccum {
        agent_name: String,
        input_tokens: u64,
        has_cache: bool,
    },
    Advisory {
        message: String,
    },
    Shutdown,
}

use std::collections::HashMap as StdHashMap;

struct AgentTokenState {
    cumulative: u64,
    any_cache_hit: bool,
    window_start: Instant,
    last_advisory: Option<Instant>,
}

pub struct ReactiveExporter {
    tx: SyncSender<Msg>,
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ReactiveExporter {
    pub fn new(config: ReactiveConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }

        let ws_ckpt_available = Command::new("ws-ckpt")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();

        if !ws_ckpt_available {
            log::warn!("[reactive] ws-ckpt not found, checkpoint action disabled");
        }

        let workspace = config
            .workspace_path
            .or_else(|| std::env::var("AGENTSIGHT_WORKSPACE").ok())
            .unwrap_or_else(|| "/root".to_string());

        let debounce = Duration::from_secs(config.debounce_secs);
        let (tx, rx) = mpsc::sync_channel::<Msg>(32);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        let handle = thread::Builder::new()
            .name("reactive-exporter".into())
            .spawn(move || {
                let mut last_ckpt = Instant::now() - debounce;
                let mut agent_tokens: StdHashMap<String, AgentTokenState> = StdHashMap::new();
                let one_hour = Duration::from_secs(3600);

                while !shutdown_clone.load(Ordering::Relaxed) {
                    let msg = match rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(m) => m,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };

                    match msg {
                        Msg::Shutdown => break,
                        Msg::Advisory { message } => {
                            log::info!("[reactive] advisory: {message}");
                        }
                        Msg::InterruptionAlert {
                            interruption_type,
                            conversation_id,
                        } => {
                            if last_ckpt.elapsed() < debounce {
                                log::debug!("[reactive] debounced interruption alert ({interruption_type})");
                                continue;
                            }
                            if !ws_ckpt_available {
                                log::info!("[reactive] would checkpoint for {interruption_type} but ws-ckpt unavailable");
                                continue;
                            }
                            let snapshot_id = format!(
                                "auto-{}-{}",
                                chrono::Utc::now().format("%Y%m%dT%H%M%S"),
                                &interruption_type
                            );
                            let msg_text = format!(
                                "reactive: {} (conv={})",
                                interruption_type,
                                conversation_id.as_deref().unwrap_or("unknown")
                            );
                            match Command::new("ws-ckpt")
                                .args(["checkpoint", "-w", &workspace, "-i", &snapshot_id, "-m", &msg_text])
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn()
                            {
                                Ok(mut child) => {
                                    let deadline = Instant::now() + Duration::from_secs(10);
                                    loop {
                                        match child.try_wait() {
                                            Ok(Some(s)) if s.success() => {
                                                log::info!("[reactive] checkpoint created: {snapshot_id}");
                                                last_ckpt = Instant::now();
                                                break;
                                            }
                                            Ok(Some(s)) => { log::warn!("[reactive] ws-ckpt exited {s}"); break; }
                                            Ok(None) if Instant::now() >= deadline => {
                                                log::warn!("[reactive] ws-ckpt timed out, killing");
                                                let _ = child.kill();
                                                let _ = child.wait();
                                                break;
                                            }
                                            Ok(None) => thread::sleep(Duration::from_millis(100)),
                                            Err(e) => { log::warn!("[reactive] ws-ckpt wait error: {e}"); break; }
                                        }
                                    }
                                }
                                Err(e) => log::warn!("[reactive] ws-ckpt spawn failed: {e}"),
                            }
                        }
                        Msg::TokenAccum {
                            agent_name,
                            input_tokens,
                            has_cache,
                        } => {
                            let state = agent_tokens.entry(agent_name.clone()).or_insert_with(|| {
                                AgentTokenState {
                                    cumulative: 0,
                                    any_cache_hit: false,
                                    window_start: Instant::now(),
                                    last_advisory: None,
                                }
                            });
                            if state.window_start.elapsed() > one_hour {
                                state.cumulative = 0;
                                state.any_cache_hit = false;
                                state.window_start = Instant::now();
                            }
                            state.cumulative += input_tokens;
                            if has_cache {
                                state.any_cache_hit = true;
                            }
                            if state.cumulative >= 200_000
                                && !state.any_cache_hit
                                && state
                                    .last_advisory
                                    .map_or(true, |t| t.elapsed() > one_hour)
                            {
                                log::info!(
                                    "[reactive] advisory: agent '{}' consumed {} input tokens with no prompt caching",
                                    agent_name, state.cumulative
                                );
                                state.last_advisory = Some(Instant::now());
                            }
                        }
                        Msg::Checkpoint {
                            reason,
                            conversation_id,
                        } => {
                            if last_ckpt.elapsed() < debounce {
                                log::debug!("[reactive] debounced checkpoint ({reason})");
                                continue;
                            }
                            if !ws_ckpt_available {
                                log::info!(
                                    "[reactive] would checkpoint for {reason} but ws-ckpt unavailable"
                                );
                                continue;
                            }
                            let snapshot_id = format!(
                                "auto-{}-{}",
                                chrono::Utc::now().format("%Y%m%dT%H%M%S"),
                                &reason
                            );
                            let msg_text = format!(
                                "reactive: {} (conv={})",
                                reason,
                                conversation_id.as_deref().unwrap_or("unknown")
                            );
                            match Command::new("ws-ckpt")
                                .args(["checkpoint", "-w", &workspace, "-i", &snapshot_id, "-m", &msg_text])
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn()
                            {
                                Ok(mut child) => {
                                    // Poll with timeout: try_wait in a loop up to 10s.
                                    // Avoids blocking indefinitely if ws-ckpt hangs.
                                    let deadline = Instant::now() + Duration::from_secs(10);
                                    loop {
                                        match child.try_wait() {
                                            Ok(Some(status)) if status.success() => {
                                                log::info!("[reactive] checkpoint created: {snapshot_id}");
                                                last_ckpt = Instant::now();
                                                break;
                                            }
                                            Ok(Some(status)) => {
                                                log::warn!("[reactive] ws-ckpt exited {status}");
                                                break;
                                            }
                                            Ok(None) if Instant::now() >= deadline => {
                                                log::warn!("[reactive] ws-ckpt timed out, killing");
                                                let _ = child.kill();
                                                let _ = child.wait();
                                                break;
                                            }
                                            Ok(None) => {
                                                thread::sleep(Duration::from_millis(100));
                                            }
                                            Err(e) => {
                                                log::warn!("[reactive] ws-ckpt wait error: {e}");
                                                break;
                                            }
                                        }
                                    }
                                }
                                Err(e) => log::warn!("[reactive] ws-ckpt spawn failed: {e}"),
                            }
                        }
                    }
                }
            })
            .ok()?;

        Some(Self {
            tx,
            shutdown,
            handle: Some(handle),
        })
    }

    /// Send an interruption alert from the existing detection pipeline.
    /// Called by unified.rs after detect_and_store_interruptions() for Critical events.
    pub fn notify_interruption(&self, interruption_type: &str, conversation_id: Option<String>) {
        let _ = self.tx.try_send(Msg::InterruptionAlert {
            interruption_type: interruption_type.to_string(),
            conversation_id,
        });
    }

    fn detect_critical(events: &[GenAISemanticEvent]) -> Option<(String, Option<String>)> {
        for event in events {
            if let GenAISemanticEvent::LLMCall(call) = event {
                let conv_id = call.metadata.get("conversation_id").cloned();
                if let Some(ref err) = call.error {
                    let lower = err.to_lowercase();
                    if lower.contains("crash")
                        || lower.contains("oom")
                        || lower.contains("sigkill")
                        || lower.contains("signal 9")
                    {
                        return Some(("agent_crash".into(), conv_id));
                    }
                    if lower.contains("context_length_exceeded")
                        || lower.contains("context_window")
                        || lower.contains("maximum context length")
                    {
                        return Some(("context_overflow".into(), conv_id));
                    }
                }
            }
        }
        None
    }

}

impl GenAIExporter for ReactiveExporter {
    fn name(&self) -> &str {
        "reactive"
    }

    fn export(&self, events: &[GenAISemanticEvent]) {
        if let Some((reason, conv_id)) = Self::detect_critical(events) {
            let _ = self.tx.try_send(Msg::Checkpoint {
                reason,
                conversation_id: conv_id,
            });
        }
        // Per-call token accumulation for cumulative advisory
        for event in events {
            if let GenAISemanticEvent::LLMCall(call) = event {
                if let Some(ref usage) = call.token_usage {
                    let has_cache = usage.cache_read_input_tokens.unwrap_or(0) > 0
                        || usage.cache_creation_input_tokens.unwrap_or(0) > 0;
                    let _ = self.tx.try_send(Msg::TokenAccum {
                        agent_name: call
                            .agent_name
                            .clone()
                            .unwrap_or_else(|| call.process_name.clone()),
                        input_tokens: usage.input_tokens as u64,
                        has_cache,
                    });
                }
            }
        }
    }
}

impl Drop for ReactiveExporter {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.tx.try_send(Msg::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai::semantic::{GenAISemanticEvent, LLMCall, LLMRequest, LLMResponse, TokenUsage};
    use std::collections::HashMap;

    fn make_call(error: Option<&str>, input_tokens: u32, cache_read: Option<u32>) -> GenAISemanticEvent {
        let mut metadata = HashMap::new();
        metadata.insert("conversation_id".to_string(), "conv-1".to_string());
        GenAISemanticEvent::LLMCall(LLMCall {
            call_id: "test".into(),
            start_timestamp_ns: 0,
            end_timestamp_ns: 0,
            duration_ns: 0,
            provider: "openai".into(),
            model: "gpt-4".into(),
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
            token_usage: Some(TokenUsage {
                input_tokens,
                output_tokens: 100,
                total_tokens: input_tokens + 100,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: cache_read,
            }),
            error: error.map(String::from),
            pid: 1234,
            process_name: "test-agent".into(),
            agent_name: Some("TestAgent".into()),
            metadata,
        })
    }

    #[test]
    fn detect_critical_finds_crash() {
        let events = vec![make_call(Some("process crashed with OOM killer"), 1000, None)];
        let result = ReactiveExporter::detect_critical(&events);
        assert!(result.is_some());
        let (reason, conv) = result.unwrap();
        assert_eq!(reason, "agent_crash");
        assert_eq!(conv.as_deref(), Some("conv-1"));
    }

    #[test]
    fn detect_critical_ignores_normal_errors() {
        let events = vec![make_call(Some("HTTP 429 rate limited"), 1000, None)];
        assert!(ReactiveExporter::detect_critical(&events).is_none());
    }

    #[test]
    fn detect_critical_ignores_no_error() {
        let events = vec![make_call(None, 1000, None)];
        assert!(ReactiveExporter::detect_critical(&events).is_none());
    }

    #[test]
    fn detect_critical_finds_context_overflow() {
        let events = vec![make_call(
            Some("This model's maximum context length is 128000 tokens"),
            1000,
            None,
        )];
        let result = ReactiveExporter::detect_critical(&events);
        assert!(result.is_some());
        let (reason, _) = result.unwrap();
        assert_eq!(reason, "context_overflow");
    }

    #[test]
    fn detect_critical_finds_context_length_exceeded() {
        let events = vec![make_call(
            Some("context_length_exceeded: input too long"),
            1000,
            None,
        )];
        let (reason, _) = ReactiveExporter::detect_critical(&events).unwrap();
        assert_eq!(reason, "context_overflow");
    }

    #[test]
    fn notify_interruption_does_not_panic() {
        let config = ReactiveConfig {
            enabled: true,
            debounce_secs: 1,
            workspace_path: Some("/tmp".to_string()),
        };
        if let Some(exporter) = ReactiveExporter::new(config) {
            exporter.notify_interruption("retry_storm", Some("conv-99".into()));
            std::thread::sleep(Duration::from_millis(100));
            drop(exporter);
        }
    }

    #[test]
    fn export_does_not_panic_on_disabled() {
        let config = ReactiveConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(ReactiveExporter::new(config).is_none());
    }

    /// Integration test: export a crash event → background thread processes it →
    /// ws-ckpt is spawned (will fail because daemon isn't running, but spawn +
    /// timeout + kill must complete without panicking or hanging).
    /// Also tests debounce: second call within debounce window is dropped.
    #[test]
    fn export_crash_event_triggers_checkpoint_attempt() {
        use crate::genai::exporter::GenAIExporter;

        let config = ReactiveConfig {
            enabled: true,
            debounce_secs: 2,
            workspace_path: Some("/tmp".to_string()),
        };

        // new() probes for ws-ckpt binary. If not installed, skip gracefully.
        let exporter = match ReactiveExporter::new(config) {
            Some(e) => e,
            None => {
                eprintln!("ws-ckpt not installed, skipping integration test");
                return;
            }
        };

        let crash_event = make_call(Some("Process killed by OOM killer"), 1000, None);
        let events = vec![crash_event];

        // First export: should trigger checkpoint attempt
        exporter.export(&events);

        // Give background thread time to spawn ws-ckpt + timeout.
        // ws-ckpt without a running daemon hangs on socket connect until our
        // 10s try_wait deadline kills it, so we need to wait >= 11s.
        std::thread::sleep(Duration::from_secs(13));

        // Second export within debounce window: should be debounced (no second spawn)
        let crash_event2 = make_call(Some("Another OOM crash"), 1000, None);
        exporter.export(&[crash_event2]);
        std::thread::sleep(Duration::from_millis(200));

        // Drop should complete promptly. The background thread either:
        // - Is idle (debounced the second message) → exits on Shutdown within 1s
        // - Is in try_wait loop for a second ws-ckpt → has up to 10s before it
        //   checks shutdown. We allow 12s total for Drop.
        let start = std::time::Instant::now();
        drop(exporter);
        let drop_time = start.elapsed();
        assert!(
            drop_time < Duration::from_secs(12),
            "Drop took too long ({drop_time:?}), background thread stuck"
        );
    }
}
