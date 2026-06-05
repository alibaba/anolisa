use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime};

use nix::libc;
use nix::pty::openpty;

use super::adapter::{BashAdapter, ShellAdapter, ZshAdapter};
use super::auth::{generate_marker_token, marker_script_with_token};
use super::lifecycle::push_shell_started_event;
use super::model::ShellHostConfig;
use super::osc::OscParser;

const OUTPUT_REF_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

pub(super) struct PtySession {
    pub(super) master: File,
    pub(super) terminal: File,
    pub(super) child: Child,
    pub(super) parser: OscParser,
}

pub(super) fn start_bash_session(config: &ShellHostConfig) -> io::Result<PtySession> {
    start_shell_session(config, &BashAdapter)
}

pub(super) fn start_zsh_session(config: &ShellHostConfig) -> io::Result<PtySession> {
    start_shell_session(config, &ZshAdapter)
}

fn start_shell_session(
    config: &ShellHostConfig,
    adapter: &dyn ShellAdapter,
) -> io::Result<PtySession> {
    fs::create_dir_all(&config.work_dir)?;
    fs::set_permissions(&config.work_dir, fs::Permissions::from_mode(0o700))?;
    let output_ref_dir = config.work_dir.join("output-refs");
    fs::create_dir_all(&output_ref_dir)?;
    fs::set_permissions(&output_ref_dir, fs::Permissions::from_mode(0o700))?;
    cleanup_expired_output_refs(&output_ref_dir, OUTPUT_REF_RETENTION)?;
    let rcfile = config.work_dir.join(adapter.marker_filename());
    let marker_token = generate_marker_token();
    fs::write(
        &rcfile,
        marker_script_with_token(adapter.marker_script(), &marker_token),
    )?;
    fs::set_permissions(&rcfile, fs::Permissions::from_mode(0o600))?;

    let pty = openpty(Some(&config.winsize), None).map_err(nix_to_io)?;
    let master = unsafe { File::from_raw_fd(pty.master.into_raw_fd()) };
    set_nonblocking(master.as_raw_fd())?;

    let slave = unsafe { File::from_raw_fd(pty.slave.into_raw_fd()) };
    let terminal = slave.try_clone()?;
    let stdin = slave.try_clone()?;
    let stdout = slave.try_clone()?;

    let mut command = Command::new(adapter.executable(config));
    adapter.configure_command(&mut command, &rcfile, config);
    command
        .env("COSH_SESSION_ID", &config.session_id)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(slave));
    if !config.native_mode {
        command
            .env("COSH_HISTFILE", config.work_dir.join("history"))
            .env("COSH_POC_PS1", &config.prompt)
            .env("BASH_SILENCE_DEPRECATION_WARNING", "1")
            .env("COSH_SHELL_ISOLATED", "1");
    }

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command.spawn()?;
    let mut parser = OscParser::new(config.session_id.clone(), output_ref_dir, marker_token);
    push_shell_started_event(&mut parser, config);

    Ok(PtySession {
        master,
        terminal,
        child,
        parser,
    })
}

fn set_nonblocking(fd: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn nix_to_io(err: nix::Error) -> io::Error {
    io::Error::other(err)
}

fn cleanup_expired_output_refs(dir: &Path, retention: Duration) -> io::Result<()> {
    cleanup_expired_output_refs_at(dir, retention, SystemTime::now())
}

fn cleanup_expired_output_refs_at(
    dir: &Path,
    retention: Duration,
    now: SystemTime,
) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        let expired = match now.duration_since(modified) {
            Ok(age) => age > retention,
            Err(_) => false,
        };
        if expired {
            let _ = fs::remove_file(entry.path());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_expired_output_refs_removes_old_files_only() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-output-ref-cleanup-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create dir");
        let stale = dir.join("cmd-old.txt");
        let subdir = dir.join("nested");
        fs::write(&stale, "old\n").expect("write stale");
        fs::create_dir_all(&subdir).expect("create subdir");

        cleanup_expired_output_refs_at(
            &dir,
            Duration::ZERO,
            SystemTime::now() + Duration::from_secs(1),
        )
        .expect("cleanup");

        assert!(!stale.exists(), "stale output ref should be removed");
        assert!(subdir.exists(), "cleanup must not remove directories");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_expired_output_refs_keeps_recent_files() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-output-ref-retain-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create dir");
        let recent = dir.join("cmd-recent.txt");
        fs::write(&recent, "recent\n").expect("write recent");

        cleanup_expired_output_refs_at(&dir, Duration::from_secs(60 * 60), SystemTime::now())
            .expect("cleanup");

        assert!(recent.exists(), "recent output ref should be retained");
        let _ = fs::remove_dir_all(&dir);
    }
}
