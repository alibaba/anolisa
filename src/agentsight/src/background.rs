use crate::genai::GenAIExporter;
use crate::genai::logtail::LogtailExporter;
use crate::storage::sqlite::GenAISqliteStore;
use anyhow::Context;
use std::ffi::OsString;
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

/// True if a filesystem event is a "file fully written" (CloseWrite) event.
///
/// Pure helper extracted so the config-watcher's event filtering is unit-testable
/// without spawning a real inotify watcher.
pub(crate) fn is_close_write(kind: &notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Access(notify::event::AccessKind::Close(
            notify::event::AccessMode::Write
        ))
    )
}

/// True if any of `paths` has a file name equal to `target`.
///
/// Pure helper extracted from the config-watcher's path filtering.
pub(crate) fn path_matches_target(paths: &[PathBuf], target: &Option<OsString>) -> bool {
    paths
        .iter()
        .any(|p| p.file_name().map(|f| f.to_os_string()) == *target)
}

/// Decision produced by [`decide_sls_config_change`]: what the config watcher
/// should do in response to a parsed `runtime.sls_logtail_path` value.
///
/// Side effects (process exit, exporter construction, mailbox write, dynamic
/// logtail-path update) are carried out by the thread shell so the decision
/// logic stays pure and testable.
#[derive(Debug, PartialEq)]
pub(crate) enum SlsConfigAction {
    /// Field missing / parse error, or empty path while already inactive: no-op.
    NoChange,
    /// Empty path while active: SLS was just deactivated (dynamic path cleared).
    Deactivated,
    /// Non-empty path but uid fetch failed: the shell must abort the process.
    AbortUidMissing,
    /// Non-empty path, first activation: shell should build a LogtailExporter for
    /// `path` and deposit it into the mailbox.
    Activate { path: String },
    /// Non-empty path, already active: dynamic path swapped, no new exporter.
    Reactivated { path: String },
}

/// Decide how to react to a parsed `runtime.sls_logtail_path`. Mutates only the
/// caller-owned `sls_activated` flag (the test-and-set that distinguishes first
/// activation from reactivation); the process-global dynamic logtail path is
/// updated by the caller (`handle_config_event`) from the returned action, so
/// this function touches no cross-module global state.
///
/// `uid` is passed in (not fetched here) because `get_owner_account_id` blocks on
/// ECS metadata and `process::exit`s the test harness; the shell fetches it and
/// this function only inspects whether it is empty.
pub(crate) fn decide_sls_config_change(
    parsed: Option<Option<String>>,
    sls_activated: &AtomicBool,
    uid: &str,
) -> SlsConfigAction {
    match parsed {
        None => SlsConfigAction::NoChange,
        Some(None) => {
            if sls_activated.swap(false, Ordering::SeqCst) {
                SlsConfigAction::Deactivated
            } else {
                SlsConfigAction::NoChange
            }
        }
        Some(Some(new_path)) => {
            if uid.is_empty() {
                return SlsConfigAction::AbortUidMissing;
            }
            if !sls_activated.swap(true, Ordering::SeqCst) {
                SlsConfigAction::Activate { path: new_path }
            } else {
                SlsConfigAction::Reactivated { path: new_path }
            }
        }
    }
}

/// Handle one config-file change: parse `runtime.sls_logtail_path`, decide the
/// SLS reaction, and carry out the in-process side effects (update the
/// process-global dynamic logtail path, and on first activation build a
/// LogtailExporter and deposit it into the mailbox).
///
/// Returns the [`SlsConfigAction`] so the thread shell can perform the only
/// non-testable action (`process::exit` on uid failure). `fetch_uid` is injected
/// so tests can supply a uid without invoking `get_owner_account_id`, which
/// blocks on ECS metadata and would `process::exit` the test harness.
pub(crate) fn handle_config_event(
    content: &str,
    sls_activated: &AtomicBool,
    fetch_uid: impl Fn() -> String,
    encryption_pem: Option<&str>,
    trace_enabled: bool,
    pending_logtail: &Mutex<Option<Box<dyn GenAIExporter>>>,
) -> SlsConfigAction {
    let parsed = crate::config::parse_runtime_sls_path(content);
    let uid: String = match &parsed {
        Some(Some(_)) => fetch_uid(),
        _ => String::new(),
    };
    let action = decide_sls_config_change(parsed, sls_activated, &uid);
    match &action {
        SlsConfigAction::NoChange => {}
        SlsConfigAction::Deactivated => {
            crate::genai::logtail::set_dynamic_logtail_path("");
            log::info!(
                "Config watcher: SLS Logtail deactivated \
                 (runtime.sls_logtail_path cleared)"
            );
        }
        SlsConfigAction::AbortUidMissing => {
            log::error!(
                "Config watcher: SLS activation requested but uid fetch failed. \
                 Terminating process."
            );
        }
        SlsConfigAction::Activate { path } => {
            crate::genai::logtail::set_dynamic_logtail_path(path);
            let exporter = LogtailExporter::new_with_path(path, encryption_pem, trace_enabled);
            log::info!("Config watcher: LogtailExporter created (path={path}, uid={uid})");
            if let Ok(mut guard) = pending_logtail.lock() {
                *guard = Some(Box::new(exporter));
            }
            log::info!("Config watcher: SLS Logtail activated dynamically");
        }
        SlsConfigAction::Reactivated { path } => {
            crate::genai::logtail::set_dynamic_logtail_path(path);
            log::info!("Config watcher: SLS Logtail re-activated with path={path}");
        }
    }
    action
}

pub(crate) fn start_config_watcher(
    config_path: PathBuf,
    sls_activated: Arc<AtomicBool>,
    pending_logtail: Arc<Mutex<Option<Box<dyn GenAIExporter>>>>,
    encryption_pem: Option<String>,
    trace_enabled: bool,
    stop: Arc<AtomicBool>,
) {
    use notify::{Event as NotifyEvent, RecommendedWatcher, RecursiveMode, Watcher};

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

                if !is_close_write(&event.kind) {
                    continue;
                }
                if !path_matches_target(&event.paths, &target_filename) {
                    continue;
                }

                let content = match std::fs::read_to_string(&watch_path) {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("Config watcher: failed to read {watch_path:?}: {e}");
                        continue;
                    }
                };

                let action = handle_config_event(
                    &content,
                    &sls_activated,
                    crate::genai::instance_id::get_owner_account_id,
                    encryption_pem.as_deref(),
                    trace_enabled,
                    &pending_logtail,
                );
                if action == SlsConfigAction::AbortUidMissing {
                    std::process::exit(1);
                }
            }

            log::info!("Config watcher exiting");
        })
        .ok();
}

/// Decision produced by [`decide_token_collector_action`].
#[derive(Debug, PartialEq)]
pub(crate) enum TokenCollectorAction {
    /// Desired state equals last applied state (or enabled-but-path-missing): skip.
    Skip,
    /// Enabled but SLS_LOG_PATH is missing/empty in ilogtail.cfg; warn once.
    WarnMissingPath,
    /// Apply `desired` to runtime.sls_logtail_path (Some=set, None=clear).
    Apply { desired: Option<String> },
}

/// Decide what the token-collector watcher should do for one poll tick.
///
/// Pure decision over the trigger-file state and the resolved logtail path; the
/// shell performs the actual `write_runtime_sls_path` and updates `last_state`.
/// `enabled` is whether the trigger file exists; `logtail_path` is the parsed
/// SLS_LOG_PATH (None when the file is absent/empty); `last_state` is the
/// previously-applied desired value.
pub(crate) fn decide_token_collector_action(
    enabled: bool,
    logtail_path: Option<String>,
    last_state: &Option<Option<String>>,
) -> TokenCollectorAction {
    let desired: Option<String> = if enabled {
        match logtail_path {
            Some(p) => Some(p),
            None => {
                // Enabled but no usable path: warn (once) unless we already
                // recorded the disabled state.
                if *last_state != Some(None) {
                    return TokenCollectorAction::WarnMissingPath;
                }
                return TokenCollectorAction::Skip;
            }
        }
    } else {
        None
    };

    if last_state.as_ref() == Some(&desired) {
        TokenCollectorAction::Skip
    } else {
        TokenCollectorAction::Apply { desired }
    }
}

/// Run one poll tick of the token-collector watcher: resolve the desired SLS
/// path from the trigger file + ilogtail.cfg, then apply it to the agentsight
/// config (updating `last_state`). Extracted from the thread loop so the full
/// decision + write + state-update path is unit-testable with real temp files;
/// the thread shell only drives the sleep loop.
pub(crate) fn run_token_collector_tick(
    config_path: &Path,
    enable_file: &str,
    logtail_cfg: &str,
    last_state: &mut Option<Option<String>>,
) {
    let enabled = Path::new(enable_file).exists();
    let logtail_path = if enabled {
        read_logtail_sls_path(logtail_cfg)
    } else {
        None
    };

    match decide_token_collector_action(enabled, logtail_path, last_state) {
        TokenCollectorAction::Skip => {}
        TokenCollectorAction::WarnMissingPath => {
            log::warn!("token-collector enabled but SLS_LOG_PATH missing/empty in {logtail_cfg}");
        }
        TokenCollectorAction::Apply { desired } => {
            match write_runtime_sls_path(config_path, desired.as_deref()) {
                Ok(false) => {
                    *last_state = Some(desired);
                }
                Ok(true) => {
                    match &desired {
                        Some(p) => log::info!(
                            "token-collector enabled: set runtime.sls_logtail_path={p:?}"
                        ),
                        None => {
                            log::info!("token-collector disabled: cleared runtime.sls_logtail_path")
                        }
                    }
                    *last_state = Some(desired);
                }
                Err(e) => {
                    log::warn!("token-collector failed to update {config_path:?}: {e}");
                }
            }
        }
    }
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
                run_token_collector_tick(&config_path, ENABLE_FILE, LOGTAIL_CFG, &mut last_state);
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
        let dir = std::env::temp_dir().join(format!("bg-test-{tag}-{n}"));
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
                call_kind: "main".to_string(),
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

    // ── is_close_write ──────────────────────────────────────────────
    #[test]
    fn test_is_close_write() {
        use notify::EventKind;
        use notify::event::{AccessKind, AccessMode};
        assert!(is_close_write(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        // Other access modes / kinds are not "fully written".
        assert!(!is_close_write(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(!is_close_write(&EventKind::Access(AccessKind::Open(
            AccessMode::Write
        ))));
        assert!(!is_close_write(&EventKind::Modify(
            notify::event::ModifyKind::Any
        )));
    }

    // ── path_matches_target ─────────────────────────────────────────
    #[test]
    fn test_path_matches_target() {
        let target = Some(OsString::from("agentsight.json"));
        assert!(path_matches_target(
            &[PathBuf::from("/etc/anolisa/agentsight.json")],
            &target
        ));
        // Non-matching file name.
        assert!(!path_matches_target(
            &[PathBuf::from("/etc/anolisa/other.json")],
            &target
        ));
        // Matches when any path in the list matches.
        assert!(path_matches_target(
            &[
                PathBuf::from("/etc/anolisa/other.json"),
                PathBuf::from("/etc/anolisa/agentsight.json"),
            ],
            &target
        ));
        // Empty list never matches.
        assert!(!path_matches_target(&[], &target));
        // None target never matches a named file.
        assert!(!path_matches_target(
            &[PathBuf::from("/etc/anolisa/agentsight.json")],
            &None
        ));
    }

    // ── decide_sls_config_change ────────────────────────────────────
    #[test]
    fn test_decide_sls_none_is_nochange() {
        let flag = AtomicBool::new(false);
        assert_eq!(
            decide_sls_config_change(None, &flag, "uid"),
            SlsConfigAction::NoChange
        );
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_decide_sls_empty_while_inactive_is_nochange() {
        let flag = AtomicBool::new(false);
        assert_eq!(
            decide_sls_config_change(Some(None), &flag, "uid"),
            SlsConfigAction::NoChange
        );
    }

    #[test]
    fn test_decide_sls_empty_while_active_deactivates() {
        let flag = AtomicBool::new(true);
        assert_eq!(
            decide_sls_config_change(Some(None), &flag, "uid"),
            SlsConfigAction::Deactivated
        );
        assert!(!flag.load(Ordering::SeqCst), "flag cleared on deactivation");
    }

    #[test]
    fn test_decide_sls_path_but_no_uid_aborts() {
        let flag = AtomicBool::new(false);
        assert_eq!(
            decide_sls_config_change(Some(Some("/p.log".into())), &flag, ""),
            SlsConfigAction::AbortUidMissing
        );
        // Flag must NOT be set when uid is missing.
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_decide_sls_first_activation() {
        let flag = AtomicBool::new(false);
        let action = decide_sls_config_change(Some(Some("/p.log".into())), &flag, "ecs-uid");
        assert_eq!(
            action,
            SlsConfigAction::Activate {
                path: "/p.log".to_string()
            }
        );
        assert!(flag.load(Ordering::SeqCst), "flag set on activation");
    }

    #[test]
    fn test_decide_sls_reactivation() {
        let flag = AtomicBool::new(true); // already active
        let action = decide_sls_config_change(Some(Some("/p2.log".into())), &flag, "ecs-uid");
        assert_eq!(
            action,
            SlsConfigAction::Reactivated {
                path: "/p2.log".to_string()
            }
        );
        assert!(flag.load(Ordering::SeqCst));
    }

    // ── decide_token_collector_action ───────────────────────────────
    #[test]
    fn test_decide_tc_disabled_first_time_applies_clear() {
        // Not enabled, no prior state -> apply None (clear).
        assert_eq!(
            decide_token_collector_action(false, None, &None),
            TokenCollectorAction::Apply { desired: None }
        );
    }

    #[test]
    fn test_decide_tc_enabled_with_path_applies() {
        assert_eq!(
            decide_token_collector_action(true, Some("/p.log".into()), &None),
            TokenCollectorAction::Apply {
                desired: Some("/p.log".to_string())
            }
        );
    }

    #[test]
    fn test_decide_tc_enabled_missing_path_warns_once() {
        // Enabled but no path, and not yet recorded disabled -> warn.
        assert_eq!(
            decide_token_collector_action(true, None, &None),
            TokenCollectorAction::WarnMissingPath
        );
        // Already recorded disabled -> skip (no repeated warning).
        assert_eq!(
            decide_token_collector_action(true, None, &Some(None)),
            TokenCollectorAction::Skip
        );
    }

    #[test]
    fn test_decide_tc_skip_when_unchanged() {
        // Desired equals last applied -> skip.
        let last = Some(Some("/p.log".to_string()));
        assert_eq!(
            decide_token_collector_action(true, Some("/p.log".into()), &last),
            TokenCollectorAction::Skip
        );
        // Disabled and last already None -> skip.
        assert_eq!(
            decide_token_collector_action(false, None, &Some(None)),
            TokenCollectorAction::Skip
        );
    }

    #[test]
    fn test_decide_tc_apply_when_path_changes() {
        let last = Some(Some("/old.log".to_string()));
        assert_eq!(
            decide_token_collector_action(true, Some("/new.log".into()), &last),
            TokenCollectorAction::Apply {
                desired: Some("/new.log".to_string())
            }
        );
    }

    // ── run_token_collector_tick (full tick over real temp files) ───
    #[test]
    fn test_tick_enabled_sets_path() {
        let dir = tmp_dir("tick1");
        let cfg = dir.join("agentsight.json");
        std::fs::write(&cfg, r#"{"runtime":{"sls_logtail_path":""}}"#).unwrap();
        let enable = dir.join("enable");
        let logtail = dir.join("ilogtail.cfg");
        std::fs::write(&enable, b"").unwrap();
        std::fs::write(&logtail, "SLS_LOG_PATH=/var/log/sls/a.log\n").unwrap();

        let mut last_state = None;
        run_token_collector_tick(
            &cfg,
            enable.to_str().unwrap(),
            logtail.to_str().unwrap(),
            &mut last_state,
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(
            v["runtime"]["sls_logtail_path"].as_str(),
            Some("/var/log/sls/a.log")
        );
        assert_eq!(last_state, Some(Some("/var/log/sls/a.log".to_string())));
    }

    #[test]
    fn test_tick_disabled_clears_path() {
        let dir = tmp_dir("tick2");
        let cfg = dir.join("agentsight.json");
        std::fs::write(&cfg, r#"{"runtime":{"sls_logtail_path":"/old.log"}}"#).unwrap();
        let enable = dir.join("enable"); // does NOT exist
        let logtail = dir.join("ilogtail.cfg");

        let mut last_state = Some(Some("/old.log".to_string()));
        run_token_collector_tick(
            &cfg,
            enable.to_str().unwrap(),
            logtail.to_str().unwrap(),
            &mut last_state,
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(v["runtime"]["sls_logtail_path"].as_str(), Some(""));
        assert_eq!(last_state, Some(None));
    }

    #[test]
    fn test_tick_skip_when_unchanged() {
        let dir = tmp_dir("tick3");
        let cfg = dir.join("agentsight.json");
        // Seed the on-disk path DIFFERENT from desired so a wrongly-chosen Apply
        // would rewrite the file (current != target), making Skip observable via
        // file content — mtime alone can't tell Skip from Apply+Ok(false).
        std::fs::write(&cfg, r#"{"runtime":{"sls_logtail_path":"/stale.log"}}"#).unwrap();
        let enable = dir.join("enable");
        let logtail = dir.join("ilogtail.cfg");
        std::fs::write(&enable, b"").unwrap();
        std::fs::write(&logtail, "SLS_LOG_PATH=/a.log\n").unwrap();

        // decide compares last_state vs desired (both /a.log) -> Skip, never writes.
        let mut last_state = Some(Some("/a.log".to_string()));
        run_token_collector_tick(
            &cfg,
            enable.to_str().unwrap(),
            logtail.to_str().unwrap(),
            &mut last_state,
        );
        // If Skip were wrongly Apply, the file would be rewritten to /a.log.
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(
            v["runtime"]["sls_logtail_path"].as_str(),
            Some("/stale.log")
        );
        assert_eq!(last_state, Some(Some("/a.log".to_string())));
    }

    #[test]
    fn test_tick_apply_ok_false_advances_state() {
        // Restart scenario: last_state=None but config already holds the desired
        // path -> decide returns Apply, write returns Ok(false) (no change), and
        // the Ok(false) arm must still advance last_state so the watcher converges.
        let dir = tmp_dir("tick6");
        let cfg = dir.join("agentsight.json");
        std::fs::write(&cfg, r#"{"runtime":{"sls_logtail_path":"/a.log"}}"#).unwrap();
        let enable = dir.join("enable");
        let logtail = dir.join("ilogtail.cfg");
        std::fs::write(&enable, b"").unwrap();
        std::fs::write(&logtail, "SLS_LOG_PATH=/a.log\n").unwrap();
        let mtime_before = std::fs::metadata(&cfg).unwrap().modified().unwrap();

        let mut last_state = None;
        run_token_collector_tick(
            &cfg,
            enable.to_str().unwrap(),
            logtail.to_str().unwrap(),
            &mut last_state,
        );
        // Ok(false) arm advances last_state even though the file is untouched.
        assert_eq!(last_state, Some(Some("/a.log".to_string())));
        let mtime_after = std::fs::metadata(&cfg).unwrap().modified().unwrap();
        assert_eq!(mtime_before, mtime_after, "file must not be rewritten");
    }

    #[test]
    fn test_tick_enabled_missing_path_is_noop() {
        let dir = tmp_dir("tick4");
        let cfg = dir.join("agentsight.json");
        std::fs::write(&cfg, r#"{"runtime":{"sls_logtail_path":""}}"#).unwrap();
        let enable = dir.join("enable");
        let logtail = dir.join("ilogtail.cfg"); // missing file -> no path
        std::fs::write(&enable, b"").unwrap();

        let mut last_state = None;
        run_token_collector_tick(
            &cfg,
            enable.to_str().unwrap(),
            logtail.to_str().unwrap(),
            &mut last_state,
        );
        // WarnMissingPath -> no write, last_state unchanged.
        assert_eq!(last_state, None);
    }

    #[test]
    fn test_tick_write_error_keeps_state() {
        let dir = tmp_dir("tick5");
        let cfg = dir.join("agentsight.json");
        std::fs::write(&cfg, r#"[1,2,3]"#).unwrap(); // invalid root -> write Err
        let enable = dir.join("enable");
        let logtail = dir.join("ilogtail.cfg");
        std::fs::write(&enable, b"").unwrap();
        std::fs::write(&logtail, "SLS_LOG_PATH=/a.log\n").unwrap();

        let mut last_state = None;
        run_token_collector_tick(
            &cfg,
            enable.to_str().unwrap(),
            logtail.to_str().unwrap(),
            &mut last_state,
        );
        // Err arm: last_state must NOT advance (so the next tick retries).
        assert_eq!(last_state, None);
    }

    // ── handle_config_event (dispatch + exporter construction) ───────
    fn empty_mailbox() -> Mutex<Option<Box<dyn GenAIExporter>>> {
        Mutex::new(None)
    }

    // Serializes tests that read or write the process-global dynamic logtail
    // path (`genai::logtail::DYNAMIC_LOGTAIL_PATH`); cargo runs tests in parallel
    // and would otherwise let them clobber each other's path assertions.
    static SLS_PATH_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_sls_path() -> std::sync::MutexGuard<'static, ()> {
        // Recover from poisoning so one failing test does not cascade-panic the rest.
        SLS_PATH_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_dynamic_path_side_effect_is_in_handler_not_decider() {
        let _guard = lock_sls_path();
        let reset = || crate::genai::logtail::set_dynamic_logtail_path("");
        reset();
        assert_eq!(
            crate::genai::logtail::logtail_path(),
            None,
            "precondition: no SLS_LOGTAIL_FILE env in the test process"
        );

        // (1) decide_sls_config_change must be PURE w.r.t. the global dynamic
        //     path. Reverting the fix (set_dynamic_logtail_path back inside
        //     decide) makes this assertion fail.
        let flag = AtomicBool::new(false);
        let action =
            decide_sls_config_change(Some(Some("/decide-only.log".into())), &flag, "ecs-uid");
        assert_eq!(
            action,
            SlsConfigAction::Activate {
                path: "/decide-only.log".to_string()
            }
        );
        assert_eq!(
            crate::genai::logtail::logtail_path(),
            None,
            "decide must NOT touch the global dynamic path"
        );

        // (2) handle_config_event MUST set the global dynamic path on
        //     activation. Forgetting to move the side effect into the handler
        //     makes this assertion fail.
        reset();
        let flag = AtomicBool::new(false);
        let mailbox = empty_mailbox();
        handle_config_event(
            r#"{"runtime":{"sls_logtail_path":"/handler-set.log"}}"#,
            &flag,
            || "ecs-uid".to_string(),
            None,
            false,
            &mailbox,
        );
        assert_eq!(
            crate::genai::logtail::logtail_path(),
            Some("/handler-set.log".to_string()),
            "handler must set the global dynamic path on activation"
        );

        // (3) handle_config_event MUST clear it on deactivation.
        let flag = AtomicBool::new(true);
        let mailbox = empty_mailbox();
        handle_config_event(
            r#"{"runtime":{"sls_logtail_path":""}}"#,
            &flag,
            || "uid".to_string(),
            None,
            false,
            &mailbox,
        );
        assert_eq!(
            crate::genai::logtail::logtail_path(),
            None,
            "handler must clear the global dynamic path on deactivation"
        );

        reset();
    }

    #[test]
    fn test_handle_event_none_is_nochange() {
        let flag = AtomicBool::new(false);
        let mailbox = empty_mailbox();
        // Content without runtime.sls_logtail_path -> parse None -> NoChange.
        let action = handle_config_event(
            r#"{"deadloop":{"enabled":false}}"#,
            &flag,
            || "uid".to_string(),
            None,
            false,
            &mailbox,
        );
        assert_eq!(action, SlsConfigAction::NoChange);
        assert!(mailbox.lock().unwrap().is_none());
    }

    #[test]
    fn test_handle_event_deactivate() {
        let _guard = lock_sls_path();
        let flag = AtomicBool::new(true); // currently active
        let mailbox = empty_mailbox();
        let action = handle_config_event(
            r#"{"runtime":{"sls_logtail_path":""}}"#,
            &flag,
            || "uid".to_string(),
            None,
            false,
            &mailbox,
        );
        assert_eq!(action, SlsConfigAction::Deactivated);
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_handle_event_abort_when_uid_missing() {
        let flag = AtomicBool::new(false);
        let mailbox = empty_mailbox();
        // Non-empty path but uid fetch returns empty -> AbortUidMissing.
        let action = handle_config_event(
            r#"{"runtime":{"sls_logtail_path":"/p.log"}}"#,
            &flag,
            String::new, // empty uid
            None,
            false,
            &mailbox,
        );
        assert_eq!(action, SlsConfigAction::AbortUidMissing);
        // No exporter built, flag not set.
        assert!(mailbox.lock().unwrap().is_none());
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_handle_event_activate_builds_exporter() {
        let _guard = lock_sls_path();
        let flag = AtomicBool::new(false);
        let mailbox = empty_mailbox();
        let action = handle_config_event(
            r#"{"runtime":{"sls_logtail_path":"/var/log/sls/a.log"}}"#,
            &flag,
            || "ecs-uid".to_string(),
            None,
            false,
            &mailbox,
        );
        assert_eq!(
            action,
            SlsConfigAction::Activate {
                path: "/var/log/sls/a.log".to_string()
            }
        );
        assert!(flag.load(Ordering::SeqCst));
        // Exporter was built and deposited into the mailbox.
        assert!(mailbox.lock().unwrap().is_some());
        crate::genai::logtail::set_dynamic_logtail_path("");
    }

    #[test]
    fn test_handle_event_reactivate_no_new_exporter() {
        let _guard = lock_sls_path();
        let flag = AtomicBool::new(true); // already active
        let mailbox = empty_mailbox();
        // Seed a different active path: an active->active change (the production
        // token-collector flow) must OVERWRITE it, not leave the stale value.
        crate::genai::logtail::set_dynamic_logtail_path("/stale.log");
        let action = handle_config_event(
            r#"{"runtime":{"sls_logtail_path":"/p2.log"}}"#,
            &flag,
            || "ecs-uid".to_string(),
            None,
            false,
            &mailbox,
        );
        assert_eq!(
            action,
            SlsConfigAction::Reactivated {
                path: "/p2.log".to_string()
            }
        );
        // Reactivation does NOT build a new exporter.
        assert!(mailbox.lock().unwrap().is_none());
        // ...but the Reactivated arm MUST overwrite the global dynamic path to
        // the new value (load-bearing: dropping its set_dynamic_logtail_path
        // would silently keep writing GenAI events to the stale path).
        assert_eq!(
            crate::genai::logtail::logtail_path(),
            Some("/p2.log".to_string()),
            "reactivation must overwrite the dynamic path to the new value"
        );
        crate::genai::logtail::set_dynamic_logtail_path("");
    }
}
