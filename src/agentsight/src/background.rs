use crate::genai::GenAIExporter;
use crate::genai::logtail::LogtailExporter;
use crate::storage::sqlite::GenAISqliteStore;
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) fn start_stale_scanner(store: Arc<GenAISqliteStore>, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("genai-stale-scanner".to_string())
        .spawn(move || {
            log::info!("GenAI stale-pending scanner started (interval=60s, timeout=300s)");
            stale_scanner_loop(&store, &stop, 60);
            log::info!("GenAI stale-pending scanner stopped");
        })
        .ok();
}

/// Marks stale pending calls as interrupted every `interval_secs`, until `stop`
/// is cleared. `interval_secs` is a parameter so tests can exercise the loop
/// body without a 60-second wait; production always passes 60.
pub(crate) fn stale_scanner_loop(store: &GenAISqliteStore, stop: &AtomicBool, interval_secs: u64) {
    while crate::utils::thread::sleep_or_stop(stop, interval_secs) {
        if let Err(e) = store.mark_interrupted_stale(300) {
            log::warn!("Stale-pending scan failed: {e}");
        }
    }
}

pub(crate) fn start_config_watcher(
    config_path: PathBuf,
    sls_activated: Arc<AtomicBool>,
    pending_logtail: Arc<Mutex<Option<Box<dyn GenAIExporter>>>>,
    encryption_pem: Option<String>,
    trace_enabled: bool,
    stop: Arc<AtomicBool>,
) {
    use notify::{Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    let watch_path = config_path.clone();
    std::thread::Builder::new()
        .name("config-watcher".to_string())
        .spawn(move || {
            log::info!("Config watcher started for {watch_path:?}");

            let (tx, rx) = std::sync::mpsc::channel::<notify::Result<NotifyEvent>>();

            let mut watcher: RecommendedWatcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(e) => {
                    log::warn!("Failed to create config file watcher: {e}");
                    return;
                }
            };

            let watch_dir = watch_path.parent().unwrap_or(Path::new("/"));
            if let Err(e) = watcher.watch(watch_dir, RecursiveMode::NonRecursive) {
                log::warn!("Failed to watch config directory {watch_dir:?}: {e}");
                return;
            }

            let target_filename = watch_path.file_name().map(|f| f.to_os_string());

            while stop.load(Ordering::SeqCst) {
                let event = match rx.recv_timeout(std::time::Duration::from_secs(1)) {
                    Ok(event) => event,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let event = match event {
                    Ok(e) => e,
                    Err(e) => {
                        log::warn!("Config watcher error: {e}");
                        continue;
                    }
                };

                match event.kind {
                    EventKind::Access(notify::event::AccessKind::Close(
                        notify::event::AccessMode::Write,
                    )) => {}
                    _ => continue,
                }

                let is_target = event.paths.iter().any(|p| {
                    p.file_name().map(|f| f.to_os_string()) == target_filename
                });
                if !is_target {
                    continue;
                }

                let content = match std::fs::read_to_string(&watch_path) {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("Config watcher: failed to read {watch_path:?}: {e}");
                        continue;
                    }
                };

                match crate::config::parse_runtime_sls_path(&content) {
                    None => continue,
                    Some(None) => {
                        if sls_activated.swap(false, Ordering::SeqCst) {
                            crate::genai::logtail::set_dynamic_logtail_path("");
                            log::info!(
                                "Config watcher: SLS Logtail deactivated \
                                 (runtime.sls_logtail_path cleared)"
                            );
                        }
                    }
                    Some(Some(new_path)) => {
                        log::info!(
                            "Config watcher: detected runtime.sls_logtail_path = {new_path:?}"
                        );

                        let uid = crate::genai::instance_id::get_owner_account_id();
                        if uid.is_empty() {
                            log::error!(
                                "Config watcher: SLS activation requested but uid fetch failed. \
                                 Terminating process."
                            );
                            std::process::exit(1);
                        }

                        crate::genai::logtail::set_dynamic_logtail_path(&new_path);

                        if !sls_activated.swap(true, Ordering::SeqCst) {
                            let exporter = LogtailExporter::new_with_path(
                                &new_path,
                                encryption_pem.as_deref(),
                                trace_enabled,
                            );
                            log::info!(
                                "Config watcher: LogtailExporter created (path={new_path}, uid={uid})"
                            );
                            if let Ok(mut guard) = pending_logtail.lock() {
                                *guard = Some(Box::new(exporter));
                            }
                            log::info!("Config watcher: SLS Logtail activated dynamically");
                        } else {
                            log::info!(
                                "Config watcher: SLS Logtail re-activated with path={new_path}"
                            );
                        }
                    }
                }
            }

            log::info!("Config watcher exiting");
        })
        .ok();
}

pub(crate) fn start_token_collector_watcher(config_path: PathBuf, stop: Arc<AtomicBool>) {
    const ENABLE_FILE: &str = "/etc/anolisa/enable_token_collector";
    const LOGTAIL_CFG: &str = "/etc/anolisa/ilogtail.cfg";
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

    std::thread::Builder::new()
        .name("token-collector-watcher".to_string())
        .spawn(move || {
            log::info!(
                "Token-collector watcher started (enable_file={ENABLE_FILE}, logtail_cfg={LOGTAIL_CFG}, target={config_path:?})"
            );

            let mut last_state: Option<Option<String>> = None;

            while stop.load(Ordering::SeqCst) {
                std::thread::sleep(POLL_INTERVAL);

                let enabled = Path::new(ENABLE_FILE).exists();

                let desired: Option<String> = if enabled {
                    match read_logtail_sls_path(LOGTAIL_CFG) {
                        Some(p) => Some(p),
                        None => {
                            if last_state != Some(None) {
                                log::warn!(
                                    "token-collector enabled but SLS_LOG_PATH missing/empty in {LOGTAIL_CFG}"
                                );
                            }
                            continue;
                        }
                    }
                } else {
                    None
                };

                if last_state.as_ref() == Some(&desired) {
                    continue;
                }

                match write_runtime_sls_path(&config_path, desired.as_deref()) {
                    Ok(false) => {
                        last_state = Some(desired);
                    }
                    Ok(true) => {
                        match &desired {
                            Some(p) => log::info!(
                                "token-collector enabled: set runtime.sls_logtail_path={p:?}"
                            ),
                            None => log::info!(
                                "token-collector disabled: cleared runtime.sls_logtail_path"
                            ),
                        }
                        last_state = Some(desired);
                    }
                    Err(e) => {
                        log::warn!(
                            "token-collector failed to update {config_path:?}: {e}"
                        );
                    }
                }
            }
            log::info!("Token-collector watcher stopped");
        })
        .ok();
}

pub(crate) fn read_logtail_sls_path(cfg_path: &str) -> Option<String> {
    let content = match std::fs::read_to_string(cfg_path) {
        Ok(c) => c,
        Err(e) => {
            log::debug!("token-collector: failed to read {cfg_path}: {e}");
            return None;
        }
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let key = parts.next()?.trim();
        if key != "SLS_LOG_PATH" {
            continue;
        }
        let raw = parts.next()?.trim();
        let value = raw.trim_matches(|c| c == '"' || c == '\'').trim();
        if value.is_empty() {
            return None;
        }
        return Some(value.to_string());
    }
    None
}

pub(crate) fn write_runtime_sls_path(
    config_path: &Path,
    new_path: Option<&str>,
) -> anyhow::Result<bool> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("read config {config_path:?}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&content).with_context(|| format!("parse JSON {config_path:?}"))?;

    let root = value
        .as_object_mut()
        .context("agentsight config root must be a JSON object")?;
    let runtime_entry = root
        .entry("runtime".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let runtime = runtime_entry
        .as_object_mut()
        .context("runtime field must be a JSON object")?;

    let target = new_path.unwrap_or("");
    let current = runtime
        .get("sls_logtail_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current == target {
        return Ok(false);
    }
    runtime.insert(
        "sls_logtail_path".to_string(),
        serde_json::Value::String(target.to_string()),
    );

    let mut new_content =
        serde_json::to_string_pretty(&value).context("serialize updated config")?;
    new_content.push('\n');

    std::fs::write(config_path, new_content.as_bytes())
        .with_context(|| format!("write config {config_path:?}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn tmp_dir(tag: &str) -> PathBuf {
        static C: AtomicU32 = AtomicU32::new(0);
        let n = C.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("bg-test-{}-{n}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_read_logtail_sls_path_found() {
        let dir = tmp_dir("r1");
        let cfg = dir.join("ilogtail.cfg");
        std::fs::write(&cfg, "KEY=value1\nSLS_LOG_PATH=/var/log/sls\n").unwrap();
        assert_eq!(
            read_logtail_sls_path(cfg.to_str().unwrap()),
            Some("/var/log/sls".to_string())
        );
    }

    #[test]
    fn test_read_logtail_sls_path_quoted() {
        let dir = tmp_dir("r2");
        let cfg = dir.join("ilogtail.cfg");
        std::fs::write(&cfg, "SLS_LOG_PATH=\"/var/log/sls\"\n").unwrap();
        assert_eq!(
            read_logtail_sls_path(cfg.to_str().unwrap()),
            Some("/var/log/sls".to_string())
        );
    }

    #[test]
    fn test_read_logtail_sls_path_single_quoted() {
        let dir = tmp_dir("r2s");
        let cfg = dir.join("ilogtail.cfg");
        std::fs::write(&cfg, "SLS_LOG_PATH='/tmp/x.log'\n").unwrap();
        assert_eq!(
            read_logtail_sls_path(cfg.to_str().unwrap()),
            Some("/tmp/x.log".to_string())
        );
    }

    #[test]
    fn test_read_logtail_sls_path_skip_comments() {
        let dir = tmp_dir("r2c");
        let cfg = dir.join("ilogtail.cfg");
        std::fs::write(
            &cfg,
            "# comment line\nOTHER_KEY=value\nSLS_LOG_PATH=/data/agent.log\nEXTRA=foo\n",
        )
        .unwrap();
        assert_eq!(
            read_logtail_sls_path(cfg.to_str().unwrap()),
            Some("/data/agent.log".to_string())
        );
    }

    #[test]
    fn test_read_logtail_sls_path_missing() {
        let dir = tmp_dir("r3");
        let cfg = dir.join("ilogtail.cfg");
        std::fs::write(&cfg, "OTHER_KEY=value\n").unwrap();
        assert_eq!(read_logtail_sls_path(cfg.to_str().unwrap()), None);
    }

    #[test]
    fn test_read_logtail_sls_path_empty() {
        let dir = tmp_dir("r4");
        let cfg = dir.join("ilogtail.cfg");
        std::fs::write(&cfg, "SLS_LOG_PATH=\n").unwrap();
        assert_eq!(read_logtail_sls_path(cfg.to_str().unwrap()), None);
    }

    #[test]
    fn test_read_logtail_sls_path_quoted_empty() {
        let dir = tmp_dir("r4q");
        let cfg = dir.join("ilogtail.cfg");
        std::fs::write(&cfg, "SLS_LOG_PATH=\"\"\n").unwrap();
        // quotes strip to empty string -> None
        assert_eq!(read_logtail_sls_path(cfg.to_str().unwrap()), None);
    }

    #[test]
    fn test_read_logtail_sls_path_no_file() {
        assert_eq!(read_logtail_sls_path("/nonexistent/path"), None);
    }

    #[test]
    fn test_write_runtime_sls_path_set() {
        let dir = tmp_dir("w1");
        let cfg = dir.join("config.json");
        // Seed with sibling fields to verify they survive the surgical edit.
        std::fs::write(
            &cfg,
            r#"{"runtime":{"sls_logtail_path":""},"deadloop":{"enabled":false,"kill_after_count":3},"https":[{"rule":["dashscope.aliyuncs.com"]}]}"#,
        )
        .unwrap();
        assert!(write_runtime_sls_path(&cfg, Some("/var/log/sls")).unwrap());
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(
            v["runtime"]["sls_logtail_path"].as_str(),
            Some("/var/log/sls")
        );
        // Sibling fields preserved untouched.
        assert_eq!(v["deadloop"]["kill_after_count"].as_u64(), Some(3));
        assert_eq!(v["deadloop"]["enabled"].as_bool(), Some(false));
        assert_eq!(
            v["https"][0]["rule"][0].as_str(),
            Some("dashscope.aliyuncs.com")
        );
    }

    #[test]
    fn test_write_runtime_sls_path_noop() {
        let dir = tmp_dir("w2");
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, r#"{"runtime":{"sls_logtail_path":"/var/log/sls"}}"#).unwrap();
        assert!(!write_runtime_sls_path(&cfg, Some("/var/log/sls")).unwrap());
    }

    #[test]
    fn test_write_runtime_sls_path_clear() {
        let dir = tmp_dir("w3");
        let cfg = dir.join("config.json");
        std::fs::write(
            &cfg,
            r#"{"runtime":{"sls_logtail_path":"/var/log/sls"},"deadloop":{"enabled":true}}"#,
        )
        .unwrap();
        assert!(write_runtime_sls_path(&cfg, None).unwrap());
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(v["runtime"]["sls_logtail_path"].as_str(), Some(""));
        // Sibling field preserved.
        assert_eq!(v["deadloop"]["enabled"].as_bool(), Some(true));
    }

    #[test]
    fn test_write_runtime_sls_path_creates_runtime_section() {
        let dir = tmp_dir("w4");
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, r#"{"deadloop":{"enabled":false}}"#).unwrap();
        assert!(write_runtime_sls_path(&cfg, Some("/p.log")).unwrap());
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(v["runtime"]["sls_logtail_path"].as_str(), Some("/p.log"));
    }

    #[test]
    fn test_write_runtime_sls_path_invalid_root_errors() {
        let dir = tmp_dir("w5");
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, r#"[1,2,3]"#).unwrap();
        assert!(write_runtime_sls_path(&cfg, Some("/p.log")).is_err());
    }

    #[test]
    fn test_watcher_logic_e2e() {
        let dir = tmp_dir("e2e");
        let cfg = dir.join("agentsight.json");
        std::fs::write(&cfg, r#"{"runtime":{"sls_logtail_path":""}}"#).unwrap();
        let logtail_cfg = dir.join("ilogtail.cfg");
        std::fs::write(&logtail_cfg, "SLS_LOG_PATH=/var/log/sls/agent.log\n").unwrap();

        let desired = read_logtail_sls_path(logtail_cfg.to_str().unwrap());
        assert_eq!(desired, Some("/var/log/sls/agent.log".to_string()));
        assert!(write_runtime_sls_path(&cfg, desired.as_deref()).unwrap());

        assert!(write_runtime_sls_path(&cfg, None).unwrap());
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(v["runtime"]["sls_logtail_path"].as_str(), Some(""));

        assert!(!write_runtime_sls_path(&cfg, None).unwrap());
    }

    #[test]
    fn test_stale_scanner_loop_returns_when_stopped() {
        // stop already false -> loop must exit promptly without running the body.
        let stop = Arc::new(AtomicBool::new(false));
        let dir = tmp_dir("stale1");
        let store = Arc::new(GenAISqliteStore::new_with_path(&dir.join("test.db")).unwrap());
        let start = std::time::Instant::now();
        stale_scanner_loop(&store, &stop, 1);
        // First sleep_or_stop call sleeps ~1s then sees stop=false and returns.
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
    }

    #[test]
    fn test_stale_scanner_loop_runs_body_then_stops() {
        use crate::storage::sqlite::PendingCallInfo;

        let dir = tmp_dir("stale2");
        let store = Arc::new(GenAISqliteStore::new_with_path(&dir.join("test.db")).unwrap());

        // Seed a pending row with an old timestamp so it counts as stale.
        let old_ts_ns = 1_000_000_000u64; // ~1970, definitely older than 300s ago
        store
            .insert_pending(&PendingCallInfo {
                call_id: "stale-1".to_string(),
                trace_id: None,
                conversation_id: None,
                session_id: None,
                start_timestamp_ns: old_ts_ns,
                pid: 1234,
                process_name: "test".to_string(),
                agent_name: None,
                http_method: None,
                http_path: None,
                input_messages: None,
                system_instructions: None,
                user_query: None,
                is_sse: false,
                model: None,
                provider: None,
            })
            .unwrap();

        // Run the loop body via a 1s interval; stop after one iteration.
        let stop = Arc::new(AtomicBool::new(true));
        let stop_clone = Arc::clone(&stop);
        let store_clone = Arc::clone(&store);
        let handle = std::thread::spawn(move || {
            stale_scanner_loop(&store_clone, &stop_clone, 1);
        });
        std::thread::sleep(std::time::Duration::from_millis(2500));
        stop.store(false, Ordering::SeqCst);
        handle.join().unwrap();

        // Discriminating signal: the loop body must have marked the seeded row
        // interrupted. If the body never ran, the row is still pending and this
        // call would mark it now, returning 1. So it MUST return 0.
        assert_eq!(
            store.mark_interrupted_stale(0).unwrap(),
            0,
            "loop body should have already marked the stale pending row"
        );
    }
}
