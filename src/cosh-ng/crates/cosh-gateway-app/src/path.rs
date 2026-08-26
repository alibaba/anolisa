//! Resolves absolute filesystem paths for the local Gateway daemon.

use std::path::{Path, PathBuf};

use super::CliError;

pub(super) fn daemon_socket_path(explicit: Option<&PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(path) = explicit {
        return require_absolute(path, "daemon socket");
    }
    if let Some(path) = std::env::var_os("COSH_GATEWAY_SOCKET") {
        return require_absolute(&PathBuf::from(path), "COSH_GATEWAY_SOCKET");
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return require_absolute(&PathBuf::from(runtime), "XDG_RUNTIME_DIR")
            .map(|path| path.join("cosh/gateway.sock"));
    }
    Ok(PathBuf::from(format!(
        "/run/user/{}/cosh/gateway.sock",
        nix::unistd::Uid::effective().as_raw()
    )))
}

pub(super) fn daemon_database_path(explicit: Option<&PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(path) = explicit {
        return require_absolute(path, "daemon database");
    }
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return require_absolute(&PathBuf::from(state), "XDG_STATE_HOME")
            .map(|path| path.join("cosh/gateway/state.db"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CliError::Daemon("absolute HOME is required".to_owned()))?;
    require_absolute(&home, "HOME").map(|path| path.join(".local/state/cosh/gateway/state.db"))
}

pub(super) fn require_absolute(path: &Path, label: &str) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Err(CliError::Daemon(format!("{label} path must be absolute")))
    }
}
