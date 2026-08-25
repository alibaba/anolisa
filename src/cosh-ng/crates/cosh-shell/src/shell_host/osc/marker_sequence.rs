//! Compact and legacy OSC marker envelopes plus history-file observation.

use std::path::PathBuf;

use serde::Deserialize;

use super::super::model::ShellHistoryFileObserver;

const SHELL_HISTORY_FILE_MAX_BYTES: usize = 4 * 1024;

pub(super) fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub(super) fn osc_prefix_suffix_len(pending: &[u8]) -> usize {
    let max_keep = pending.len().min(super::OSC_PREFIX.len().saturating_sub(1));
    for size in (1..=max_keep).rev() {
        if super::OSC_PREFIX.starts_with(&pending[pending.len() - size..]) {
            return size;
        }
    }
    0
}

#[derive(Debug, Deserialize)]
pub(super) struct Marker {
    #[serde(alias = "e")]
    pub(super) event: String,
    #[serde(alias = "t")]
    pub(super) token: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) timestamp_ms: Option<u64>,
    #[serde(alias = "c")]
    pub(super) cwd: Option<String>,
    pub(super) command: Option<String>,
    pub(super) reason: Option<String>,
    #[serde(alias = "s")]
    pub(super) status: Option<i32>,
    pub(super) path: Option<String>,
    pub(super) path_trusted: Option<bool>,
    #[serde(alias = "h")]
    pub(super) history_file: Option<String>,
    pub(super) prompt_ready: Option<bool>,
    pub(super) generation: Option<u64>,
    pub(super) top_level_missing: Option<bool>,
    pub(super) proven: Option<bool>,
    pub(super) intent: Option<String>,
    pub(super) sensitive: Option<bool>,
    #[serde(rename = "unsafe")]
    pub(super) unsafe_input: Option<bool>,
    /// Handoff claim token echoed by the approved handoff marker pair.
    #[serde(alias = "x")]
    pub(super) handoff: Option<String>,
}

impl Marker {
    /// Expands the compact prompt boundary into the legacy parser event shape.
    pub(super) fn normalize_compact_prompt(&mut self) -> bool {
        if self.event != "p" {
            return false;
        }
        self.event = "precmd".to_string();
        self.prompt_ready = Some(true);
        true
    }

    pub(super) fn has_trusted_history_context(
        &self,
        session_id: &str,
        compact_prompt_marker: bool,
    ) -> bool {
        self.session_id.as_deref() == Some(session_id)
            || (compact_prompt_marker && self.session_id.is_none())
    }
}

#[derive(Debug, Default)]
pub(super) struct HistoryFileTracker {
    observer: Option<ShellHistoryFileObserver>,
    last_path: Option<PathBuf>,
}

impl HistoryFileTracker {
    pub(super) fn set_observer(&mut self, observer: ShellHistoryFileObserver) {
        self.observer = Some(observer);
    }

    pub(super) fn observe(&mut self, history_file: Option<&str>) {
        let Some(path) = history_file.filter(|value| {
            value.len() <= SHELL_HISTORY_FILE_MAX_BYTES
                && !value.chars().any(|character| character.is_control())
        }) else {
            return;
        };
        let path = PathBuf::from(path);
        if !path.is_absolute() || self.last_path.as_ref() == Some(&path) {
            return;
        }
        self.last_path = Some(path.clone());
        if let Some(observer) = &self.observer {
            observer.observe(path);
        }
    }
}
