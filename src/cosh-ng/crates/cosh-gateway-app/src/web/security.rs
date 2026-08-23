//! Descriptor-first credential validation for the loopback Web adapter.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use super::{CliError, MAX_TOKEN_BYTES};

#[derive(Debug)]
pub(super) struct LoadedToken {
    pub(super) bytes: Vec<u8>,
    pub(super) path: PathBuf,
}

pub(super) fn canonical_workspace(path: &Path) -> Result<PathBuf, CliError> {
    if !path.is_absolute() {
        return Err(CliError::Web("workspace path must be absolute".to_owned()));
    }
    let canonical =
        std::fs::canonicalize(path).map_err(|error| CliError::Web(error.to_string()))?;
    if !canonical.is_dir() {
        return Err(CliError::Web(
            "workspace must be an existing directory".to_owned(),
        ));
    }
    Ok(canonical)
}

pub(super) fn read_token(path: &Path) -> Result<LoadedToken, CliError> {
    if !path.is_absolute() {
        return Err(CliError::Web(
            "Bearer token path must be absolute".to_owned(),
        ));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| CliError::Web(error.to_string()))?;
    let opened = file
        .metadata()
        .map_err(|error| CliError::Web(error.to_string()))?;
    if !opened.file_type().is_file() || opened.mode() & 0o777 != 0o600 || opened.nlink() != 1 {
        return Err(CliError::Web(
            "Bearer token must be a single-link regular file with mode 0600".to_owned(),
        ));
    }
    let owner = opened.uid();
    let effective = nix::unistd::Uid::effective().as_raw();
    if owner != 0 && owner != effective {
        return Err(CliError::Web(
            "Bearer token must be owned by root or the current user".to_owned(),
        ));
    }
    let opened_path = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|error| CliError::Web(format!("cannot resolve opened Bearer token: {error}")))?;
    if !opened_path.is_absolute()
        || opened_path
            .as_os_str()
            .to_string_lossy()
            .ends_with(" (deleted)")
    {
        return Err(CliError::Web(
            "opened Bearer token has no stable absolute path".to_owned(),
        ));
    }
    validate_token_ancestors(&opened_path, effective)?;
    let mut token = Vec::new();
    (&mut file)
        .take((MAX_TOKEN_BYTES + 1) as u64)
        .read_to_end(&mut token)
        .map_err(|error| CliError::Web(error.to_string()))?;
    while token.last().is_some_and(u8::is_ascii_whitespace) {
        token.pop();
    }
    if token.len() < 32 || token.len() > MAX_TOKEN_BYTES || !token.iter().all(u8::is_ascii_graphic)
    {
        return Err(CliError::Web(
            "Bearer token must contain 32 to 256 printable ASCII bytes".to_owned(),
        ));
    }
    Ok(LoadedToken {
        bytes: token,
        path: opened_path,
    })
}

fn validate_token_ancestors(path: &Path, effective_uid: u32) -> Result<(), CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::Web("Bearer token has no parent directory".to_owned()))?;
    for directory in parent.ancestors() {
        let metadata = std::fs::symlink_metadata(directory)
            .map_err(|error| CliError::Web(error.to_string()))?;
        let mode = metadata.mode();
        let sticky = mode & 0o1000 != 0;
        if !metadata.file_type().is_dir()
            || (metadata.uid() != 0 && metadata.uid() != effective_uid)
            || (mode & 0o022 != 0 && !sticky)
        {
            return Err(CliError::Web(format!(
                "Bearer token ancestor {} is not trusted and private",
                directory.display()
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_token_scope(token_path: &Path, workspace: &Path) -> Result<(), CliError> {
    if token_path.starts_with(workspace) {
        return Err(CliError::Web(
            "Bearer token must be outside the operator-declared workspace".to_owned(),
        ));
    }
    Ok(())
}
