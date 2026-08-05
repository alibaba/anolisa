//! Validates process and file targets before building enforcement requests.

use std::fs;
use std::path::{Path, PathBuf};

/// Errors that prevent a process or file from being used in an enforcement policy.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TargetValidationError {
    #[error("PID {0} is not an eligible Agent process")]
    ProtectedProcess(i32),
    #[error("cannot inspect PID {pid}: {source}")]
    ProcessIo { pid: i32, source: std::io::Error },
    #[error("invalid /proc/{0}/stat start time")]
    InvalidStat(i32),
    #[error("invalid policy file {path}: {message}")]
    InvalidPath { path: PathBuf, message: String },
}

/// Reads the Linux process start time after excluding protected service processes.
pub(crate) fn read_process_start_time(pid: i32) -> Result<u64, TargetValidationError> {
    if pid <= 1 || pid == std::process::id() as i32 {
        return Err(TargetValidationError::ProtectedProcess(pid));
    }

    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|source| TargetValidationError::ProcessIo { pid, source })?;
    let open = stat
        .find('(')
        .ok_or(TargetValidationError::InvalidStat(pid))?;
    let close = stat
        .rfind(')')
        .filter(|close| *close > open)
        .ok_or(TargetValidationError::InvalidStat(pid))?;
    let process_name = &stat[open + 1..close];
    if matches!(
        process_name,
        "agentsight" | "agentsight-enfo" | "agentsight-enforcer"
    ) {
        return Err(TargetValidationError::ProtectedProcess(pid));
    }

    stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or(TargetValidationError::InvalidStat(pid))?
        .parse()
        .map_err(|_| TargetValidationError::InvalidStat(pid))
}

/// Resolves an existing regular file that is safe to embed in the policy lexer.
pub(crate) fn canonical_policy_file(path: &Path) -> Result<PathBuf, TargetValidationError> {
    if !path.is_absolute() {
        return Err(invalid_path(path, "path must be absolute"));
    }
    validate_policy_path_text(path)?;
    let canonical = path.canonicalize().map_err(|error| {
        invalid_path(
            path,
            format!("cannot canonicalize path {}: {error}", path.display()),
        )
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        invalid_path(
            &canonical,
            format!("cannot inspect path {}: {error}", canonical.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err(invalid_path(
            &canonical,
            "path must identify an existing regular file",
        ));
    }
    validate_policy_path_text(&canonical)?;
    Ok(canonical)
}

fn validate_policy_path_text(path: &Path) -> Result<(), TargetValidationError> {
    let value = path
        .to_str()
        .ok_or_else(|| invalid_path(path, "path must be valid UTF-8"))?;
    if value.contains(['\0', '"', '\r', '\n']) {
        return Err(invalid_path(
            path,
            "path contains characters unsupported by the policy lexer",
        ));
    }
    Ok(())
}

fn invalid_path(path: &Path, message: impl Into<String>) -> TargetValidationError {
    TargetValidationError::InvalidPath {
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TemporaryRegularFile {
        path: PathBuf,
    }

    impl TemporaryRegularFile {
        fn create() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);

            let pid = std::process::id();
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();

            for _ in 0..100 {
                let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "agentsight-target-{pid}-{timestamp}-{counter}.policy"
                ));
                match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(_) => return Self { path },
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("failed to create temporary policy file: {error}"),
                }
            }

            panic!("could not create a unique temporary policy file")
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TemporaryRegularFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn rejects_init_and_agentsight_processes() {
        assert!(matches!(
            read_process_start_time(1),
            Err(TargetValidationError::ProtectedProcess(_))
        ));
        assert!(matches!(
            read_process_start_time(std::process::id() as i32),
            Err(TargetValidationError::ProtectedProcess(_))
        ));
    }

    #[test]
    fn canonicalizes_an_existing_regular_file() {
        let file = TemporaryRegularFile::create();
        assert_eq!(
            canonical_policy_file(file.path()).unwrap(),
            file.path().canonicalize().unwrap()
        );
    }
}
