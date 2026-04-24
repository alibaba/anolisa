//! OOM recovery module
//!
//! On AgentSight startup, scans `dmesg` for OOM kill events that occurred
//! after the last known AgentSight shutdown timestamp. For each killed process
//! that matches a known agent name, an `agent_crash` InterruptionEvent is
//! written to the interruption store with `oom: true` in its detail JSON.
//!
//! This handles the case where AgentSight itself was killed by OOM and
//! therefore could not record the crash in real-time.

use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::interruption::types::{InterruptionEvent, InterruptionType};
use crate::storage::sqlite::InterruptionStore;
use crate::storage::sqlite::GenAISqliteStore;

/// A parsed OOM kill event from dmesg
#[derive(Debug)]
struct OomKillEvent {
    /// Approximate timestamp in nanoseconds since Unix epoch
    pub timestamp_ns: i64,
    /// Killed process PID
    pub pid: i32,
    /// Killed process name (comm, may be truncated to 15 chars)
    pub process_name: String,
}

/// Run OOM recovery on startup.
///
/// Reads `dmesg -T`, parses OOM kill events, and writes `agent_crash`
/// interruption events for any killed process whose name matches a known agent.
///
/// Uses the latest existing OOM event timestamp in the DB as `since_ns` to
/// avoid re-writing events from previous runs. Each event is also checked
/// individually by (pid, occurred_at_ns) before insertion.
pub fn recover_oom_events(
    interruption_store: &Arc<InterruptionStore>,
    genai_store: Option<&Arc<GenAISqliteStore>>,
    _since_ns: i64,
) {
    // Use the latest OOM event timestamp already in DB as the dedup cutoff
    let since_ns = interruption_store.latest_oom_event_ns();
    log::info!("OOM recovery: scanning dmesg, since_ns={}", since_ns);

    let events = match parse_dmesg_oom_events() {
        Ok(e) => e,
        Err(err) => {
            log::warn!("OOM recovery: failed to read dmesg: {}", err);
            return;
        }
    };

    if events.is_empty() {
        log::info!("OOM recovery: no OOM kill events found in dmesg");
        return;
    }

    let mut written = 0usize;
    for ev in &events {
        // Skip events at or before the last recorded OOM timestamp
        if since_ns > 0 && ev.timestamp_ns <= since_ns {
            continue;
        }

        // Per-event dedup: skip if already recorded with same (pid, timestamp)
        if interruption_store.oom_event_exists(ev.pid, ev.timestamp_ns) {
            log::debug!("OOM recovery: skip duplicate pid={} ts={}", ev.pid, ev.timestamp_ns);
            continue;
        }

        // Match against known agent process name prefixes
        let agent_name = match_agent_name(&ev.process_name);

        // Try to correlate with genai_events to find active session/trace
        // Use 5-minute lookback window to avoid false positives from old data
        let since_ns = ev.timestamp_ns.saturating_sub(300_000_000_000) as i64;
        let (session_id, trace_id, active_traces): (Option<String>, Option<String>, Vec<String>) =
            if let Some(gstore) = genai_store {
                match gstore.list_incomplete_agentic_sessions_for_pid(ev.pid, since_ns) {
                    Ok(pairs) => {
                        // Use the first pair as the primary session/trace;
                        // collect all trace_ids for the detail field.
                        let primary = pairs.first();
                        let traces: Vec<String> = pairs.iter()
                            .filter_map(|(_, _, tid)| tid.clone())
                            .collect();
                        (
                            primary.and_then(|(_, sid, _)| sid.clone()),
                            primary.and_then(|(_, _, tid)| tid.clone()),
                            traces,
                        )
                    }
                    Err(e) => {
                        log::debug!("OOM recovery: failed to query genai for pid={}: {}", ev.pid, e);
                        (None, None, Vec::new())
                    }
                }
            } else {
                (None, None, Vec::new())
            };

        let mut detail = serde_json::json!({
            "pid": ev.pid,
            "process_name": ev.process_name,
            "agent_name": agent_name,
            "oom": true,
            "source": "dmesg",
        });
        if !active_traces.is_empty() {
            detail["active_traces"] = serde_json::json!(active_traces);
        }

        let interruption = InterruptionEvent::new(
            InterruptionType::AgentCrash,
            session_id,
            trace_id,
            None,
            Some(ev.pid),
            agent_name.map(|s| s.to_string()),
            ev.timestamp_ns,
            Some(detail),
        );

        match interruption_store.insert(&interruption) {
            Ok(_) => {
                log::info!(
                    "OOM recovery: wrote agent_crash for pid={} name={} at {}",
                    ev.pid, ev.process_name, ev.timestamp_ns,
                );
                written += 1;
            }
            Err(e) => {
                log::warn!("OOM recovery: failed to insert event for pid={}: {}", ev.pid, e);
            }
        }
    }

    log::info!(
        "OOM recovery: scanned {} OOM events, wrote {} new interruption records",
        events.len(), written,
    );
}

/// Parse OOM kill events from `dmesg -T` output.
///
/// Looks for lines like:
///   [Fri Apr 17 10:00:00 2026] Out of memory: Killed process 12345 (openclaw-gatewa) ...
fn parse_dmesg_oom_events() -> Result<Vec<OomKillEvent>, Box<dyn std::error::Error>> {
    let output = Command::new("dmesg")
        .arg("-T")
        .output()
        .map_err(|e| format!("failed to run dmesg: {}", e))?;

    if !output.status.success() {
        // Some systems require privileges; fall back to dmesg without -T
        let output2 = Command::new("dmesg").output()?;
        return parse_dmesg_lines(&String::from_utf8_lossy(&output2.stdout));
    }

    parse_dmesg_lines(&String::from_utf8_lossy(&output.stdout))
}

fn parse_dmesg_lines(content: &str) -> Result<Vec<OomKillEvent>, Box<dyn std::error::Error>> {
    let mut events = Vec::new();
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);

    for line in content.lines() {
        // Match: "Killed process <pid> (<name>)"
        // Example: [Fri Apr 17 10:00:00 2026] Out of memory: Killed process 12345 (openclaw-gatewa) ...
        if !line.contains("Killed process") {
            continue;
        }

        let (pid, process_name) = match parse_killed_process(line) {
            Some(v) => v,
            None => continue,
        };

        // Try to extract timestamp from dmesg -T format: [Fri Apr 17 10:00:00 2026]
        let timestamp_ns = parse_dmesg_timestamp(line).unwrap_or(now_ns);

        events.push(OomKillEvent {
            timestamp_ns,
            pid,
            process_name,
        });
    }

    Ok(events)
}

/// Extract (pid, process_name) from a dmesg line containing "Killed process".
///
/// Handles: "Killed process 12345 (openclaw-gatewa)"
fn parse_killed_process(line: &str) -> Option<(i32, String)> {
    // Find "Killed process " then parse pid and (name)
    let after = line.split("Killed process ").nth(1)?;
    // after = "12345 (openclaw-gatewa) ..."
    let mut parts = after.splitn(2, ' ');
    let pid_str = parts.next()?;
    let rest = parts.next().unwrap_or("");

    let pid: i32 = pid_str.trim().parse().ok()?;

    // Extract name between first '(' and ')'
    let name = rest
        .split('(').nth(1)
        .and_then(|s| s.split(')').next())
        .unwrap_or("")
        .to_string();

    if name.is_empty() {
        return None;
    }

    Some((pid, name))
}

/// Parse timestamp from dmesg -T format: "[Fri Apr 17 15:58:28 2026]"
/// Returns nanoseconds since Unix epoch, or None if parsing fails.
fn parse_dmesg_timestamp(line: &str) -> Option<i64> {
    // Format: [Fri Apr 17 15:58:28 2026]
    let start = line.find('[')?;
    let end = line.find(']')?;
    if end <= start {
        return None;
    }
    let ts_str = line[start + 1..end].trim();
    // ts_str = "Fri Apr 17 15:58:28 2026"
    // Fields: weekday(0) month(1) day(2) time(3) year(4)
    let parts: Vec<&str> = ts_str.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    // Reconstruct as "17 Apr 2026 15:58:28" for a stable parse
    let normalised = format!("{} {} {} {}", parts[2], parts[1], parts[4], parts[3]);
    let dt = chrono::NaiveDateTime::parse_from_str(&normalised, "%d %b %Y %T").ok()?;
    let ns = dt.and_utc().timestamp_nanos_opt()?;
    Some(ns)
}

/// Match a process comm name to a known agent name.
/// Returns Some(agent_name) if matched, None otherwise.
fn match_agent_name(comm: &str) -> Option<&'static str> {
    let comm_lower = comm.to_lowercase();
    if comm_lower.starts_with("openclaw-gatewa") || comm_lower.starts_with("openclaw") {
        Some("OpenClaw")
    } else if comm_lower == "co" || comm_lower == "cosh" || comm_lower.starts_with("copilot") {
        Some("Cosh")
    } else if comm_lower.starts_with("node") {
        // Node processes could be either; record with unknown agent but still track
        Some("node(unknown-agent)")
    } else {
        // Non-agent processes — skip
        None
    }
}
