//! Service management backend — routes operations via systemctl.

use std::process::Command;

use cosh_types::error::{CoshError, ErrorCode};
use cosh_types::svc::*;

use crate::{run_command, SVC_TIMEOUT};

/// Get structured status of a systemd service.
pub fn svc_status(name: &str) -> Result<SvcStatus, CoshError> {
    // Use systemctl show for machine-readable output
    let output = run_command(
        Command::new("systemctl").args(["show", name, "--no-pager"]),
        SVC_TIMEOUT,
        "svc",
    )?;

    if !output.status.success() {
        return Err(CoshError::new(
            ErrorCode::SvcNotFound,
            format!("Service '{}' not found", name),
            "svc",
        )
        .with_hint("Try 'cosh svc list' to see available services".to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let props = parse_systemctl_show(&stdout);

    // systemctl show returns exit 0 even for nonexistent units — detect via LoadState.
    let load_state = props.get("LoadState").map(|s| s.as_str()).unwrap_or("");
    if load_state == "not-found" {
        return Err(CoshError::new(
            ErrorCode::SvcNotFound,
            format!("Service '{}' not found (unit not loaded)", name),
            "svc",
        )
        .with_hint("Try 'cosh svc list' to see available services"));
    }

    let active_state = props.get("ActiveState").map(|s| s.as_str()).unwrap_or("unknown");
    let state = match active_state {
        "active" => SvcState::Running,
        "inactive" => SvcState::Stopped,
        "failed" => SvcState::Failed,
        "activating" => SvcState::Activating,
        "deactivating" => SvcState::Deactivating,
        other => SvcState::Unknown(other.to_string()),
    };

    let pid = props
        .get("MainPID")
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&p| p > 0);

    let enabled = props
        .get("UnitFileState")
        .map(|s| s == "enabled")
        .unwrap_or(false);

    let memory_bytes = props
        .get("MemoryCurrent")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&m| m < u64::MAX); // [not set] parses as max

    let description = props.get("Description").cloned();

    // Fetch recent journal lines
    let recent_logs = fetch_journal_lines(name, 5);

    Ok(SvcStatus {
        name: name.to_string(),
        active: active_state == "active",
        enabled,
        state,
        pid,
        uptime_secs: parse_uptime_secs(&props),
        memory_bytes,
        description,
        recent_logs,
    })
}

/// Perform a service action (start/stop/restart).
pub fn svc_action(name: &str, action: &str, dry_run: bool) -> Result<SvcActionResult, CoshError> {
    let valid_actions = ["start", "stop", "restart", "enable", "disable"];
    if !valid_actions.contains(&action) {
        return Err(CoshError::new(
            ErrorCode::InvalidInput,
            format!("Invalid action '{}'. Valid: start, stop, restart, enable, disable", action),
            "svc",
        ));
    }

    // Get current state before action
    let before = svc_status(name)?;
    let previous_state = before.state.clone();

    if dry_run {
        return Ok(SvcActionResult {
            name: name.to_string(),
            action: action.to_string(),
            success: true,
            previous_state,
            new_state: SvcState::Unknown("(dry-run)".to_string()),
        });
    }

    let output = run_command(
        Command::new("systemctl").args([action, name]),
        SVC_TIMEOUT,
        "svc",
    )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = match action {
            "start" | "restart" => ErrorCode::SvcStartFailed,
            "stop" => ErrorCode::SvcStopFailed,
            _ => ErrorCode::Unknown,
        };
        return Err(CoshError::new(code, stderr.trim().to_string(), "svc")
            .recoverable(true)
            .with_hint(format!("Check logs with 'cosh svc status {}'", name)));
    }

    // Get new state after action
    let after = svc_status(name).unwrap_or(SvcStatus {
        name: name.to_string(),
        active: false,
        enabled: false,
        state: SvcState::Unknown("query-failed".to_string()),
        pid: None,
        uptime_secs: None,
        memory_bytes: None,
        description: None,
        recent_logs: vec![],
    });

    Ok(SvcActionResult {
        name: name.to_string(),
        action: action.to_string(),
        success: true,
        previous_state,
        new_state: after.state,
    })
}

/// List services matching an optional state filter.
pub fn svc_list(state_filter: Option<&str>) -> Result<SvcListResult, CoshError> {
    let mut args = vec!["list-units", "--type=service", "--no-pager", "--no-legend"];
    if let Some(state) = state_filter {
        validate_state_filter(state)?;
        args.push("--state");
        args.push(state);
    }

    let output = run_command(
        Command::new("systemctl").args(&args),
        SVC_TIMEOUT,
        "svc",
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut services = Vec::new();

    for line in stdout.lines() {
        if let Some(svc) = parse_svc_list_line(line) {
            services.push(svc);
        }
    }

    let total = services.len();
    Ok(SvcListResult { services, total })
}

/// Parse a single line from `systemctl list-units --no-legend` output.
///
/// Expected columns: UNIT  LOAD  ACTIVE  SUB  DESCRIPTION...
fn parse_svc_list_line(line: &str) -> Option<SvcStatus> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let name = parts[0].trim_end_matches(".service");
    let active = parts[2] == "active";
    let state = match parts[3] {
        "running" => SvcState::Running,
        "exited" => SvcState::Stopped,
        "dead" => SvcState::Stopped,
        "failed" => SvcState::Failed,
        other => SvcState::Unknown(other.to_string()),
    };
    let description = if parts.len() > 4 {
        Some(parts[4..].join(" "))
    } else {
        None
    };

    Some(SvcStatus {
        name: name.to_string(),
        active,
        enabled: false, // list-units doesn't show this
        state,
        pid: None,
        uptime_secs: None,
        memory_bytes: None,
        description,
        recent_logs: vec![],
    })
}

/// Validate a `--state` filter value for `systemctl list-units`.
fn validate_state_filter(state: &str) -> Result<(), CoshError> {
    const VALID_STATES: &[&str] = &[
        "active", "inactive", "activating", "deactivating", "reloading",
        "loaded", "not-found", "masked",
        "running", "dead", "exited", "failed", "waiting", "listening",
        "mounted", "plugged",
        "enabled", "disabled", "static", "generated", "indirect",
    ];
    if VALID_STATES.contains(&state) {
        Ok(())
    } else {
        Err(CoshError::new(
            ErrorCode::InvalidInput,
            format!("Invalid state filter '{}'. Valid: {}", state, VALID_STATES.join(", ")),
            "svc",
        ))
    }
}

// --- Internal helpers ---

fn parse_systemctl_show(output: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for line in output.lines() {
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.to_string(), value.to_string());
        }
    }
    map
}

/// Compute service uptime from `ActiveEnterTimestampMonotonic` and `/proc/uptime`.
///
/// `ActiveEnterTimestampMonotonic` is microseconds since boot when the service
/// last entered the active state. We subtract it from the current system uptime
/// (also measured since boot) to get the service running duration.
fn parse_uptime_secs(props: &std::collections::HashMap<String, String>) -> Option<u64> {
    let monotonic_us = props
        .get("ActiveEnterTimestampMonotonic")?
        .parse::<u64>()
        .ok()
        .filter(|&v| v > 0)?;

    let system_uptime_secs = read_system_uptime_secs()?;
    let active_enter_secs = monotonic_us / 1_000_000;

    system_uptime_secs.checked_sub(active_enter_secs)
}

/// Read system uptime in whole seconds from `/proc/uptime`.
fn read_system_uptime_secs() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/uptime").ok()?;
    let secs_str = content.split_whitespace().next()?;
    secs_str.parse::<f64>().ok().map(|s| s as u64)
}

fn fetch_journal_lines(unit: &str, count: usize) -> Vec<String> {
    let result = run_command(
        Command::new("journalctl").args(["-u", unit, "-n", &count.to_string(), "--no-pager", "-q"]),
        SVC_TIMEOUT,
        "svc",
    );

    match result {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect()
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_systemctl_show tests ---

    #[test]
    fn test_parse_systemctl_show_active() {
        let output = "ActiveState=active\nMainPID=1234\nUnitFileState=enabled\nDescription=nginx - high performance web server\nMemoryCurrent=1048576";
        let props = parse_systemctl_show(output);

        assert_eq!(props.get("ActiveState").unwrap(), "active");
        assert_eq!(props.get("MainPID").unwrap(), "1234");
        assert_eq!(props.get("UnitFileState").unwrap(), "enabled");
        assert_eq!(props.get("Description").unwrap(), "nginx - high performance web server");
        assert_eq!(props.get("MemoryCurrent").unwrap(), "1048576");
    }

    #[test]
    fn test_parse_systemctl_show_inactive() {
        let output = "ActiveState=inactive\nMainPID=0\nUnitFileState=disabled";
        let props = parse_systemctl_show(output);

        assert_eq!(props.get("ActiveState").unwrap(), "inactive");
        assert_eq!(props.get("MainPID").unwrap(), "0");
        assert_eq!(props.get("UnitFileState").unwrap(), "disabled");
    }

    #[test]
    fn test_parse_systemctl_show_failed() {
        let output = "ActiveState=failed\nMainPID=0\nDescription=Postfix Mail Transport Agent";
        let props = parse_systemctl_show(output);

        assert_eq!(props.get("ActiveState").unwrap(), "failed");
    }

    #[test]
    fn test_parse_systemctl_show_empty() {
        let output = "";
        let props = parse_systemctl_show(output);
        assert!(props.is_empty());
    }

    #[test]
    fn test_parse_systemctl_show_line_without_equals() {
        let output = "ActiveState=active\nSome line without equals sign\nMainPID=99";
        let props = parse_systemctl_show(output);
        assert_eq!(props.len(), 2);
        assert_eq!(props.get("ActiveState").unwrap(), "active");
        assert_eq!(props.get("MainPID").unwrap(), "99");
    }

    // --- SvcState from ActiveState mapping ---

    #[test]
    fn test_svc_state_active() {
        let props = parse_systemctl_show("ActiveState=active");
        let active_state = props.get("ActiveState").map(|s| s.as_str()).unwrap_or("unknown");
        let state = match active_state {
            "active" => SvcState::Running,
            "inactive" => SvcState::Stopped,
            "failed" => SvcState::Failed,
            "activating" => SvcState::Activating,
            "deactivating" => SvcState::Deactivating,
            other => SvcState::Unknown(other.to_string()),
        };
        assert_eq!(state, SvcState::Running);
    }

    #[test]
    fn test_svc_state_inactive() {
        let props = parse_systemctl_show("ActiveState=inactive");
        let active_state = props.get("ActiveState").map(|s| s.as_str()).unwrap_or("unknown");
        let state = match active_state {
            "active" => SvcState::Running,
            "inactive" => SvcState::Stopped,
            "failed" => SvcState::Failed,
            "activating" => SvcState::Activating,
            "deactivating" => SvcState::Deactivating,
            other => SvcState::Unknown(other.to_string()),
        };
        assert_eq!(state, SvcState::Stopped);
    }

    #[test]
    fn test_svc_state_failed() {
        let props = parse_systemctl_show("ActiveState=failed");
        let active_state = props.get("ActiveState").map(|s| s.as_str()).unwrap_or("unknown");
        let state = match active_state {
            "active" => SvcState::Running,
            "inactive" => SvcState::Stopped,
            "failed" => SvcState::Failed,
            "activating" => SvcState::Activating,
            "deactivating" => SvcState::Deactivating,
            other => SvcState::Unknown(other.to_string()),
        };
        assert_eq!(state, SvcState::Failed);
    }

    #[test]
    fn test_svc_state_activating() {
        let props = parse_systemctl_show("ActiveState=activating");
        let active_state = props.get("ActiveState").map(|s| s.as_str()).unwrap_or("unknown");
        let state = match active_state {
            "active" => SvcState::Running,
            "inactive" => SvcState::Stopped,
            "failed" => SvcState::Failed,
            "activating" => SvcState::Activating,
            "deactivating" => SvcState::Deactivating,
            other => SvcState::Unknown(other.to_string()),
        };
        assert_eq!(state, SvcState::Activating);
    }

    #[test]
    fn test_svc_state_unknown() {
        let props = parse_systemctl_show("ActiveState=maintenance");
        let active_state = props.get("ActiveState").map(|s| s.as_str()).unwrap_or("unknown");
        let state = match active_state {
            "active" => SvcState::Running,
            "inactive" => SvcState::Stopped,
            "failed" => SvcState::Failed,
            "activating" => SvcState::Activating,
            "deactivating" => SvcState::Deactivating,
            other => SvcState::Unknown(other.to_string()),
        };
        assert_eq!(state, SvcState::Unknown("maintenance".to_string()));
    }

    #[test]
    fn test_svc_state_missing_defaults_to_unknown() {
        let props = parse_systemctl_show("MainPID=0");
        let active_state = props.get("ActiveState").map(|s| s.as_str()).unwrap_or("unknown");
        let state = match active_state {
            "active" => SvcState::Running,
            "inactive" => SvcState::Stopped,
            "failed" => SvcState::Failed,
            "activating" => SvcState::Activating,
            "deactivating" => SvcState::Deactivating,
            other => SvcState::Unknown(other.to_string()),
        };
        assert_eq!(state, SvcState::Unknown("unknown".to_string()));
    }

    // --- UTF-8 service names and descriptions ---

    #[test]
    fn test_parse_systemctl_show_utf8() {
        let output = "ActiveState=active\nDescription=数据库服务 (MySQL)\nMainPID=5678";
        let props = parse_systemctl_show(output);
        assert_eq!(props.get("Description").unwrap(), "数据库服务 (MySQL)");
    }

    #[test]
    fn test_parse_systemctl_show_utf8_description_with_special_chars() {
        let output = "ActiveState=active\nDescription=nginx — высокопроизводительный сервер\nMainPID=100";
        let props = parse_systemctl_show(output);
        assert!(props.get("Description").unwrap().contains("nginx"));
    }

    // --- svc_action validation ---

    #[test]
    fn test_svc_action_invalid_action() {
        let result = svc_action("nginx", "destroy", false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert!(err.message.contains("destroy"));
    }

    // --- PID parsing edge cases ---

    #[test]
    fn test_pid_zero_means_none() {
        let props = parse_systemctl_show("MainPID=0");
        let pid = props
            .get("MainPID")
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&p| p > 0);
        assert!(pid.is_none());
    }

    #[test]
    fn test_pid_positive_value() {
        let props = parse_systemctl_show("MainPID=1234");
        let pid = props
            .get("MainPID")
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&p| p > 0);
        assert_eq!(pid, Some(1234));
    }

    // --- parse_uptime_secs tests ---

    #[test]
    fn test_parse_uptime_active_service() {
        // Simulate: system uptime = 10000s, service entered active at 5000s since boot
        // We can't easily mock /proc/uptime, so test the logic with a known-good
        // monotonic value by reading actual /proc/uptime
        let sys_uptime = super::read_system_uptime_secs();
        if sys_uptime.is_none() {
            // Skip test if /proc/uptime is unavailable (e.g., non-Linux CI)
            return;
        }
        let sys_uptime = sys_uptime.unwrap();

        // Pretend service started 120 seconds ago
        let active_enter_mono = (sys_uptime.saturating_sub(120)) * 1_000_000;
        let mut props = std::collections::HashMap::new();
        props.insert(
            "ActiveEnterTimestampMonotonic".to_string(),
            active_enter_mono.to_string(),
        );

        let uptime = parse_uptime_secs(&props);
        assert!(uptime.is_some());
        // Allow 2 second tolerance for timing drift
        let val = uptime.unwrap();
        assert!((119..=122).contains(&val), "unexpected uptime: {}", val);
    }

    #[test]
    fn test_parse_uptime_inactive_service() {
        // ActiveEnterTimestampMonotonic=0 means never activated
        let mut props = std::collections::HashMap::new();
        props.insert("ActiveEnterTimestampMonotonic".to_string(), "0".to_string());

        let uptime = parse_uptime_secs(&props);
        assert!(uptime.is_none());
    }

    #[test]
    fn test_parse_uptime_na_value() {
        // Missing key entirely
        let props = std::collections::HashMap::new();
        let uptime = parse_uptime_secs(&props);
        assert!(uptime.is_none());

        // Non-numeric value
        let mut props2 = std::collections::HashMap::new();
        props2.insert("ActiveEnterTimestampMonotonic".to_string(), "n/a".to_string());
        let uptime2 = parse_uptime_secs(&props2);
        assert!(uptime2.is_none());
    }

    // --- MemoryCurrent parsing edge cases ---

    #[test]
    fn test_memory_current_not_set() {
        // systemd uses u64::MAX for [not set]
        let props = parse_systemctl_show("MemoryCurrent=18446744073709551615");
        let memory = props
            .get("MemoryCurrent")
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&m| m < u64::MAX);
        assert!(memory.is_none());
    }

    #[test]
    fn test_memory_current_valid() {
        let props = parse_systemctl_show("MemoryCurrent=5242880");
        let memory = props
            .get("MemoryCurrent")
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&m| m < u64::MAX);
        assert_eq!(memory, Some(5242880));
    }

    // --- parse_svc_list_line tests ---

    #[test]
    fn test_parse_svc_list_line_active_running() {
        let line = "cron.service loaded active running Regular background program processing daemon";
        let svc = parse_svc_list_line(line).unwrap();
        assert_eq!(svc.name, "cron");
        assert!(svc.active);
        assert_eq!(svc.state, SvcState::Running);
        assert_eq!(svc.description, Some("Regular background program processing daemon".to_string()));
    }

    #[test]
    fn test_parse_svc_list_line_inactive_exited() {
        let line = "cloud-init.service loaded inactive exited Initial cloud-init job";
        let svc = parse_svc_list_line(line).unwrap();
        assert_eq!(svc.name, "cloud-init");
        assert!(!svc.active);
        assert_eq!(svc.state, SvcState::Stopped);
    }

    #[test]
    fn test_parse_svc_list_line_sub_dead() {
        let line = "apt-daily.service loaded active dead Daily apt download activities";
        let svc = parse_svc_list_line(line).unwrap();
        assert_eq!(svc.name, "apt-daily");
        assert!(svc.active);
        assert_eq!(svc.state, SvcState::Stopped);
    }

    // --- validate_state_filter tests ---

    #[test]
    fn test_validate_state_filter_valid() {
        assert!(validate_state_filter("running").is_ok());
        assert!(validate_state_filter("active").is_ok());
        assert!(validate_state_filter("failed").is_ok());
        assert!(validate_state_filter("dead").is_ok());
        assert!(validate_state_filter("enabled").is_ok());
    }

    #[test]
    fn test_validate_state_filter_invalid() {
        let err = validate_state_filter("bogus").unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert!(err.message.contains("bogus"));
    }

    #[test]
    fn test_validate_state_filter_shell_metachar() {
        assert!(validate_state_filter("running;evil").is_err());
        assert!(validate_state_filter("").is_err());
    }
}
