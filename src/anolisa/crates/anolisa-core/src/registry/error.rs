//! Shared error type for the registry submodule (config + client + cache).

use std::path::PathBuf;

/// Errors raised while resolving registry settings or fetching the index.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The config file exists but its `[registry]` table is malformed, or the
    /// file could not be read for a reason other than absence.
    #[error("invalid registry config '{path}': {reason}")]
    Config {
        /// Config file that failed to load.
        path: PathBuf,
        /// Parser or I/O diagnostic.
        reason: String,
    },

    /// A registry cache file could not be read or written.
    #[error("registry cache io error at '{path}': {source}")]
    Io {
        /// Cache path involved in the failed filesystem operation.
        path: PathBuf,
        /// Original I/O error from the OS.
        #[source]
        source: std::io::Error,
    },

    /// The fetched (or cached) index TOML could not be parsed.
    #[error("cannot parse distribution index: {reason}")]
    Parse {
        /// Raw `toml` parser message.
        reason: String,
    },

    /// The cached index is stale, a fresh copy could not be fetched, and
    /// offline fallback is disabled (or there is no cache to fall back to).
    #[error("registry offline: cannot refresh index from {url} ({reason})")]
    Offline {
        /// Index URL that could not be refreshed.
        url: String,
        /// Why the refresh attempt failed.
        reason: String,
    },
}
