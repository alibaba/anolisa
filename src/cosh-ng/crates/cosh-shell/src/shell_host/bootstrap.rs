use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
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
    pub(super) recovery_request_file: PathBuf,
    pub(super) handoff_request_file: PathBuf,
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
    let recovery_request_file = config.work_dir.join("terminal-recovery-request");
    let handoff_request_file = config.work_dir.join("shell-handoff-request");
    let marker_token = generate_marker_token();
    let recovery_request_file_str = recovery_request_file.to_string_lossy().to_string();
    let handoff_request_file_str = handoff_request_file.to_string_lossy().to_string();
    fs::write(
        &rcfile,
        marker_script_with_token(
            adapter.marker_script(),
            &marker_token,
            &recovery_request_file_str,
            &handoff_request_file_str,
        ),
    )?;
    fs::set_permissions(&rcfile, fs::Permissions::from_mode(0o600))?;

    let pty = openpty(Some(&config.winsize), None).map_err(nix_to_io)?;
    let master = unsafe { File::from_raw_fd(pty.master.into_raw_fd()) };
    set_close_on_exec(master.as_raw_fd())?;
    set_nonblocking(master.as_raw_fd())?;

    let slave = unsafe { File::from_raw_fd(pty.slave.into_raw_fd()) };
    set_interactive_terminal_baseline(slave.as_raw_fd())?;
    let terminal = slave.try_clone()?;
    let stdin = slave.try_clone()?;
    let stdout = slave.try_clone()?;
    set_close_on_exec(slave.as_raw_fd())?;
    set_close_on_exec(terminal.as_raw_fd())?;
    set_close_on_exec(stdin.as_raw_fd())?;
    set_close_on_exec(stdout.as_raw_fd())?;

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
    for (key, value) in &config.env_overrides {
        command.env(key, value);
    }

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            set_interactive_terminal_baseline(0)?;
            if libc::tcsetpgrp(0, libc::getpgrp()) < 0 {
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
        recovery_request_file,
        handoff_request_file,
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

fn set_close_on_exec(fd: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_interactive_terminal_baseline(fd: i32) -> io::Result<()> {
    let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut termios) } < 0 {
        return Err(io::Error::last_os_error());
    }

    termios.c_lflag |= libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ICANON | libc::ISIG;
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    {
        termios.c_lflag |= libc::IEXTEN;
    }
    termios.c_iflag |= libc::ICRNL | libc::IXON | libc::BRKINT;
    termios.c_iflag &= !(libc::IGNBRK | libc::INLCR | libc::IGNCR | libc::ISTRIP);
    termios.c_oflag |= libc::OPOST;
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    {
        termios.c_oflag |= libc::ONLCR;
    }
    set_control_char(&mut termios, libc::VINTR, 0x03);
    set_control_char(&mut termios, libc::VQUIT, 0x1c);
    set_control_char(&mut termios, libc::VERASE, 0x7f);
    set_control_char(&mut termios, libc::VKILL, 0x15);
    set_control_char(&mut termios, libc::VEOF, 0x04);
    set_control_char(&mut termios, libc::VEOL, 0x00);
    set_control_char(&mut termios, libc::VMIN, 0x01);
    set_control_char(&mut termios, libc::VTIME, 0x00);
    set_control_char(&mut termios, libc::VSUSP, 0x1a);
    set_control_char(&mut termios, libc::VSTART, 0x11);
    set_control_char(&mut termios, libc::VSTOP, 0x13);

    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_control_char(termios: &mut libc::termios, index: usize, value: u8) {
    if index < termios.c_cc.len() {
        termios.c_cc[index] = value as libc::cc_t;
    }
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

    #[test]
    fn close_on_exec_sets_fd_flag() {
        let pty = openpty(None, None).expect("open pty");
        let fd = pty.master.as_raw_fd();

        set_close_on_exec(fd).expect("set close on exec");

        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }
}
