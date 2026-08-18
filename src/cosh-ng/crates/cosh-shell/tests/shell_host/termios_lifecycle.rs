use std::fs;
use std::io::{self, Write as _};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use nix::libc;
use wait_timeout::ChildExt as _;

// Minimal self-contained session env: this shell_host-layer test spawns
// the real binary directly instead of borrowing the raw_cli harness, so
// it pins only what the termios roundtrip needs (an isolated HOME and a
// quiet, deterministic session).
fn lifecycle_command(binary: &str) -> Command {
    let home = std::env::temp_dir().join(format!(
        "cosh-termios-home-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&home).expect("lifecycle home");
    let mut command = Command::new(binary);
    command
        .env("COSH_SHELL_ISOLATED", "1")
        .env("COSH_SHELL_RAW_SHELL", "bash")
        .env("COSH_SHELL_DEFAULT_SHELL", "bash")
        .env("COSH_SHELL_LANG", "en-US")
        .env("COSH_SHELL_BOOTSTRAP_PATH", "0")
        .env("COSH_SHELL_HEALTH_SCAN", "disabled")
        .env("COSH_RECOMMENDATIONS_ENABLED", "0")
        .env("TERM", "xterm-256color")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("HOME", home);
    command
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

// M5 termios lifecycle anchors (#2598 / #2537-RC4): the raw relay holds the
// outer terminal in raw mode for the whole session, so every interceptable
// exit path must hand the terminal back with the exact termios it started
// with. Recovery surface audited on the fix base:
//   - normal exit / EOF: RawModeGuard::drop (shell_host/raw_runner.rs)
//   - SIGTERM/SIGHUP/SIGQUIT + panic: install_terminal_recovery
//     (runtime/terminal.rs)
// SIGKILL is uninterceptable and stays an explicit non-goal (NG-1).

// Raw-mode engagement can legitimately take a while on a loaded host
// (binary startup + inner shell spawn); restoration is synchronous in
// RawModeGuard::drop / the signal handlers and normally lands within
// milliseconds, so its window only needs to absorb pty scheduling noise.
// Both stay hard deadlines: a slow-but-eventual restore is still a
// contract regression worth failing on, not a log line.
const RAW_ENGAGE_DEADLINE: Duration = Duration::from_secs(20);
const RESTORE_DEADLINE: Duration = Duration::from_secs(10);

enum ExitPath {
    ExitCommand,
    Eof,
    Sigterm,
}

// tcflag_t is u32 on Linux and u64 on macOS; the widening cast is required
// on the CI target even though clippy flags it as redundant on macOS.
#[allow(clippy::unnecessary_cast)]
fn termios_flags(fd: i32) -> (u64, u64, u64, u64) {
    let mut attrs = std::mem::MaybeUninit::<libc::termios>::zeroed();
    let rc = unsafe { libc::tcgetattr(fd, attrs.as_mut_ptr()) };
    assert_eq!(
        rc,
        0,
        "tcgetattr({fd}) failed: {}",
        io::Error::last_os_error()
    );
    let attrs = unsafe { attrs.assume_init() };
    (
        attrs.c_iflag as u64,
        attrs.c_oflag as u64,
        attrs.c_cflag as u64,
        attrs.c_lflag as u64,
    )
}

fn assert_termios_roundtrip(path: ExitPath) {
    if Command::new("bash").arg("--version").output().is_err() {
        return;
    }

    let pty = nix::pty::openpty(None, None).expect("openpty");
    let master: OwnedFd = pty.master;
    let slave: OwnedFd = pty.slave;
    let before = termios_flags(slave.as_raw_fd());

    let binary = env!("CARGO_BIN_EXE_cosh-shell");
    let mut command = lifecycle_command(binary);
    command
        .args(["raw", "fake"])
        .stdin(Stdio::from(slave.try_clone().expect("slave stdin")))
        .stdout(Stdio::from(slave.try_clone().expect("slave stdout")))
        .stderr(Stdio::from(slave.try_clone().expect("slave stderr")));
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
    let mut child = command.spawn().expect("spawn cosh-shell in pty");

    // Drain the master so the child never blocks on a full pty buffer.
    // Deliberately detached: joining would deadlock — the read only sees
    // EOF once every slave fd is closed, and a grandchild of the shell
    // (the inner bash) may legitimately outlive the SIGTERM path while
    // still holding its inherited slave. The thread exits with the test
    // process (probed on Linux CI where the join hung all three tests).
    let drain_fd = master.try_clone().expect("drain fd");
    thread::spawn(move || {
        let mut file = fs::File::from(drain_fd);
        let mut sink = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut file, &mut sink) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    // Deterministic gate: wait until the relay actually engaged raw mode on
    // the outer terminal before driving the exit path.
    let engage_deadline = std::time::Instant::now() + RAW_ENGAGE_DEADLINE;
    while termios_flags(slave.as_raw_fd()).3 == before.3 {
        if std::time::Instant::now() > engage_deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("raw mode never engaged on the outer pty");
        }
        thread::sleep(Duration::from_millis(50));
    }
    // Let the inner shell reach its prompt before typing at it.
    thread::sleep(Duration::from_millis(700));

    match path {
        ExitPath::ExitCommand => {
            let mut writer = fs::File::from(master.try_clone().expect("writer fd"));
            writer.write_all(b"exit\n").expect("write exit");
        }
        ExitPath::Eof => {
            let mut writer = fs::File::from(master.try_clone().expect("writer fd"));
            writer.write_all(&[0x04]).expect("write eof");
        }
        ExitPath::Sigterm => {
            let rc = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            assert_eq!(rc, 0, "kill(SIGTERM): {}", io::Error::last_os_error());
        }
    }

    let status = child
        .wait_timeout(Duration::from_secs(30))
        .expect("wait for cosh-shell");
    let status = match status {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cosh-shell did not exit within the timeout");
        }
    };
    match path {
        ExitPath::Sigterm => assert!(
            !status.success(),
            "SIGTERM path must not report success: {status:?}"
        ),
        _ => assert!(status.success(), "clean exit path failed: {status:?}"),
    }

    // The recovery handlers restore termios before the process is reaped;
    // poll briefly to absorb pty scheduling noise, then assert field by
    // field so a partial restore cannot pass.
    let restore_deadline = std::time::Instant::now() + RESTORE_DEADLINE;
    let mut after = termios_flags(slave.as_raw_fd());
    while after != before && std::time::Instant::now() < restore_deadline {
        thread::sleep(Duration::from_millis(50));
        after = termios_flags(slave.as_raw_fd());
    }
    assert_eq!(after.0, before.0, "c_iflag not restored");
    assert_eq!(after.1, before.1, "c_oflag not restored");
    assert_eq!(after.2, before.2, "c_cflag not restored");
    assert_eq!(after.3, before.3, "c_lflag not restored");

    drop(slave);
    drop(master);
}

#[test]
fn termios_restored_after_exit_command() {
    assert_termios_roundtrip(ExitPath::ExitCommand);
}

#[test]
fn termios_restored_after_eof() {
    assert_termios_roundtrip(ExitPath::Eof);
}

#[test]
fn termios_restored_after_sigterm() {
    assert_termios_roundtrip(ExitPath::Sigterm);
}
