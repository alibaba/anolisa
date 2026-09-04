//! Watchdog subprocess for terminal state recovery after parent SIGKILL.
//!
//! Forks a lightweight child before entering raw mode. If the parent is
//! killed (SIGKILL is uncatchable), the watchdog detects parent death and
//! restores the original termios settings and file status flags.
//!
//! - Linux: `prctl(PR_SET_PDEATHSIG, SIGTERM)` delivers SIGTERM when the
//!   parent dies; the child blocks in `sigwait`.
//! - macOS: the child polls `kill(getppid(), 0)` every 200ms.
//!
//! Normal exit: `RawModeGuard::drop` dismisses the watchdog via SIGTERM.

use std::io;
use std::os::fd::RawFd;

use nix::libc;

/// Keyboard protocol withdrawal written to the output path during
/// terminal restoration (paired with the relay's enable sequence).
const MODIFY_OTHER_KEYS_DISABLE: &[u8] = b"\x1b[>4;0m";

/// Spawn a terminal watchdog child process that restores `original_termios`
/// and `original_flags` if the current process is killed without running
/// cleanup (e.g. SIGKILL).
///
/// Returns the child PID on success, or `None` if fork failed.
pub(super) fn spawn_terminal_watchdog(
    original_termios: Option<libc::termios>,
    original_flags: i32,
    fd: RawFd,
    output_fd: RawFd,
) -> Option<libc::pid_t> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return None;
    }
    if pid == 0 {
        // Child process — async-signal-safe calls only (no malloc, no Rust
        // allocator, no println).
        run_watchdog_child(original_termios, original_flags, fd, output_fd);
    }
    // Parent: return child PID.
    Some(pid)
}

/// Dismiss a previously spawned watchdog by sending SIGTERM and reaping it.
///
/// Called from `RawModeGuard::drop` after the parent has already restored
/// terminal state, so the watchdog exits without performing its own restore.
pub(super) fn dismiss_watchdog(pid: libc::pid_t) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    let mut status: libc::c_int = 0;
    let ret = unsafe { libc::waitpid(pid, &mut status, 0) };
    if ret < 0 {
        // Force-kill if the child is somehow stuck.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            libc::waitpid(pid, &mut status, 0);
        }
    }
}

#[allow(clippy::needless_return)]
fn run_watchdog_child(
    original_termios: Option<libc::termios>,
    original_flags: i32,
    fd: RawFd,
    output_fd: RawFd,
) -> ! {
    #[cfg(target_os = "linux")]
    {
        // prctl(PR_SET_PDEATHSIG) sends SIGTERM when the parent dies.
        unsafe {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
        }
        // Race guard: if the parent died between fork() and prctl(), getppid
        // no longer matches and we restore immediately.
        if unsafe { libc::getppid() } <= 1 {
            restore_terminal(original_termios.as_ref(), original_flags, fd, output_fd);
            unsafe { libc::_exit(0) };
        }
        // Block all signals; sigwait will deliver SIGTERM.
        unsafe {
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            libc::sigprocmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
        }
        let mut received: libc::c_int = 0;
        let mut waitset: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut waitset);
            libc::sigaddset(&mut waitset, libc::SIGTERM);
            libc::sigwait(&waitset, &mut received);
        }
        // SIGTERM means parent died (prctl) or parent dismissed us; either
        // way, restore terminal state (idempotent if already restored).
        restore_terminal(original_termios.as_ref(), original_flags, fd, output_fd);
        unsafe { libc::_exit(0) };
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Unblock SIGTERM so it can interrupt nanosleep; set disposition to
        // SIG_DFL so delivery terminates this child (dismiss from parent).
        unsafe {
            let mut mask: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGTERM);
            libc::sigprocmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut());
            let mut sa: libc::sigaction = std::mem::zeroed();
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = 0;
            sa.sa_handler = Some(sigterm_handler);
            libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        }

        loop {
            let mut ts = libc::timespec {
                tv_sec: 0,
                tv_nsec: 200_000_000,
            };
            let interrupted = unsafe {
                libc::nanosleep(&ts, &mut ts) < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
            };
            if unsafe { libc::kill(unsafe { libc::getppid() }, 0) } < 0 {
                restore_terminal(original_termios.as_ref(), original_flags, fd, output_fd);
                unsafe { libc::_exit(0) };
            }
            if interrupted {
                // SIGTERM from dismiss interrupted nanosleep.
                unsafe { libc::_exit(0) };
            }
        }
    }
}

/// Minimal SIGTERM handler for macOS polling path. Returns to the caller
/// so that `nanosleep` reports EINTR; the loop then exits.
#[cfg(not(target_os = "linux"))]
extern "C" fn sigterm_handler(_: libc::c_int) {
    // Intentionally empty: the interrupt itself is the signal.
}

/// Restore original termios, clear `O_NONBLOCK`, and write the keyboard
/// protocol withdrawal to the output path. All calls are async-signal-safe.
fn restore_terminal(
    original_termios: Option<&libc::termios>,
    original_flags: i32,
    fd: RawFd,
    output_fd: RawFd,
) {
    if let Some(termios) = original_termios {
        // Clear O_NONBLOCK so the cleanup write cannot be lost to EAGAIN.
        unsafe {
            libc::fcntl(fd, libc::F_SETFL, original_flags);
        }

        // Write the keyboard protocol withdrawal to the output path.
        let output_flags = unsafe { libc::fcntl(output_fd, libc::F_GETFL) };
        if output_flags >= 0 {
            let write_flags = if unsafe { libc::isatty(output_fd) } == 1 {
                output_flags & !libc::O_NONBLOCK
            } else {
                output_flags | libc::O_NONBLOCK
            };
            unsafe {
                libc::fcntl(output_fd, libc::F_SETFL, write_flags);
                libc::write(
                    output_fd,
                    MODIFY_OTHER_KEYS_DISABLE.as_ptr().cast(),
                    MODIFY_OTHER_KEYS_DISABLE.len(),
                );
                libc::fcntl(output_fd, libc::F_SETFL, output_flags);
            }
        }

        // Restore original termios.
        unsafe {
            libc::tcsetattr(fd, libc::TCSANOW, termios);
        }
    }

    // Restore original file status flags (clears O_NONBLOCK if it was not
    // originally set).
    unsafe {
        libc::fcntl(fd, libc::F_SETFL, original_flags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn watchdog_spawn_returns_valid_pid_and_dismiss_terminates() {
        let pty = nix::pty::openpty(None, None).expect("open pty");
        let fd = pty.slave.as_raw_fd();
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(fd, &mut termios) }, 0);
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(flags >= 0);

        let pid = spawn_terminal_watchdog(Some(termios), flags, fd, fd).expect("spawn watchdog");
        assert!(pid > 0, "watchdog pid must be positive");
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "watchdog must be alive");

        dismiss_watchdog(pid);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "watchdog must be dead after dismiss"
        );
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn watchdog_double_dismiss_is_safe() {
        let pty = nix::pty::openpty(None, None).expect("open pty");
        let fd = pty.slave.as_raw_fd();
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(fd, &mut termios) }, 0);
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };

        let pid = spawn_terminal_watchdog(Some(termios), flags, fd, fd).expect("spawn watchdog");

        dismiss_watchdog(pid);
        // Second dismiss: waitpid returns -1 (ECHILD), falls through to
        // SIGKILL + waitpid which also returns -1 — both are benign.
        dismiss_watchdog(pid);
    }

    #[test]
    fn watchdog_without_termios_cleans_up_on_dismiss() {
        let pipe = nix::unistd::pipe().expect("open pipe");
        let fd = pipe.0.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };

        let pid = spawn_terminal_watchdog(None, flags, fd, fd).expect("spawn watchdog");
        dismiss_watchdog(pid);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    #[test]
    fn watchdog_parent_death_restores_terminal() {
        let pty = nix::pty::openpty(None, None).expect("open pty");
        let fd = pty.slave.as_raw_fd();

        // Put the slave into raw mode so the watchdog has something to
        // restore.
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(fd, &mut original) }, 0);
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        assert_eq!(unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) }, 0);

        // Capture the watchdog's output via a pipe.
        let (pipe_read, pipe_write) = nix::unistd::pipe().expect("open pipe");
        let output_fd = pipe_write.as_raw_fd();

        let pid =
            spawn_terminal_watchdog(Some(original), 0, fd, output_fd).expect("spawn watchdog");

        // Close our copy of the write end; the watchdog's inherited copy
        // is the only writer remaining.
        drop(pipe_write);

        let reader = thread::spawn(move || {
            let mut all = Vec::new();
            let mut buf = [0_u8; 64];
            loop {
                let n = unsafe {
                    libc::read(pipe_read.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len())
                };
                if n <= 0 {
                    break;
                }
                all.extend_from_slice(&buf[..n as usize]);
            }
            all
        });

        // Simulate parent death by sending SIGTERM to the watchdog. On
        // Linux, prctl(PR_SET_PDEATHSIG) delivers SIGTERM when the parent
        // dies; this test triggers the same signal path directly.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }

        let output = reader.join().expect("reader thread");
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("\x1b[>4;0m"),
            "watchdog must write disable sequence on parent death, got: {text:?}"
        );

        // Verify termios was restored on the slave fd (shared with the
        // watchdog via the open file description).
        let mut restored = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(fd, &mut restored) }, 0);
        assert_eq!(
            restored.c_lflag & libc::ECHO,
            original.c_lflag & libc::ECHO,
            "ECHO must be restored"
        );
        assert_eq!(
            restored.c_lflag & libc::ICANON,
            original.c_lflag & libc::ICANON,
            "ICANON must be restored"
        );

        // Reap the watchdog.
        let mut status: libc::c_int = 0;
        unsafe {
            libc::waitpid(pid, &mut status, 0);
        }
    }
}
