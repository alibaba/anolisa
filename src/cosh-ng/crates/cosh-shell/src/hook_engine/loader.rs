use std::fs;
use std::path::{Path, PathBuf};

use crate::hook_types::{HookMatcher, HookTrigger};

use super::{ExternalHookConfig, ExternalHookSource};

pub(super) fn load_external_hook_configs(
    dir: &Path,
    source: ExternalHookSource,
    project_root: Option<PathBuf>,
    trusted: bool,
) -> Vec<ExternalHookConfig> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut configs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = path.metadata() {
                if meta.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
        }
        if let Some(mut config) = parse_hook_header(&path) {
            config.source = source.clone();
            config.project_root = project_root.clone();
            config.trusted = trusted;
            configs.push(config);
        }
    }
    configs
}

pub(super) fn parse_hook_header(path: &Path) -> Option<ExternalHookConfig> {
    let content = fs::read_to_string(path).ok()?;
    let mut hook_id: Option<String> = None;
    let mut match_commands: Vec<String> = Vec::new();
    let mut trigger = HookTrigger::OnComplete;
    let mut timeout_ms: u64 = 5000;

    for line in content.lines().take(10) {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("# cosh-hook:") {
            hook_id = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("# match-commands:") {
            match_commands = val.split(',').map(|s| s.trim().to_string()).collect();
        } else if let Some(val) = line.strip_prefix("# trigger:") {
            trigger = match val.trim() {
                "on_fail" => HookTrigger::OnFail,
                "on_success" => HookTrigger::OnSuccess,
                _ => HookTrigger::OnComplete,
            };
        } else if let Some(val) = line.strip_prefix("# timeout:") {
            timeout_ms = parse_timeout(val.trim());
        }
    }

    let id = hook_id?;
    Some(ExternalHookConfig {
        path: path.to_path_buf(),
        matcher: HookMatcher {
            id,
            commands: match_commands,
            command_patterns: Vec::new(),
            command_regex: None,
            min_output_bytes: None,
            exit_codes: None,
            trigger,
        },
        timeout_ms,
        source: ExternalHookSource::User,
        project_root: None,
        trusted: true,
    })
}

pub(super) fn parse_timeout(s: &str) -> u64 {
    if let Some(ms) = s.strip_suffix("ms") {
        ms.trim().parse::<u64>().unwrap_or(5000)
    } else if let Some(secs) = s.strip_suffix('s') {
        secs.trim().parse::<u64>().unwrap_or(5) * 1000
    } else {
        s.parse::<u64>().unwrap_or(5000)
    }
}
