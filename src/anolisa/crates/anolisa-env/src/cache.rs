//! Environment facts caching to avoid repeated detection.

use crate::EnvFacts;
use std::path::PathBuf;

/// Default cache path for environment facts.
pub fn cache_path(system_mode: bool) -> PathBuf {
    if system_mode {
        PathBuf::from("/var/lib/anolisa/env-facts.json")
    } else {
        // user state root override, else the $HOME-based default
        std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".local/state"))
                    .unwrap_or_else(|_| PathBuf::from("/tmp"))
            })
            .join("anolisa/env-facts.json")
    }
}

/// Load cached facts if they exist and are not stale.
pub fn load_cached(_path: &PathBuf) -> Option<EnvFacts> {
    // TODO(owner: env-detection, when: repeated probes become measurable):
    // load JSON only when the recorded TTL is still valid.
    None
}

/// Write facts to cache.
pub fn save_cache(_path: &PathBuf, _facts: &EnvFacts) -> std::io::Result<()> {
    // TODO(owner: env-detection, when: `load_cached` is wired): persist
    // facts atomically so partial writes cannot poison future detection.
    Ok(())
}
