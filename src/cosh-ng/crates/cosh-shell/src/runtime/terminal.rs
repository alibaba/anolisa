use std::io::{self, Write};
use std::sync::atomic::{AtomicI32, Ordering};

use nix::libc;

static mut ORIGINAL_TERMIOS: Option<libc::termios> = None;
static ORIGINAL_FILE_STATUS_FLAGS: AtomicI32 = AtomicI32::new(-1);

pub(crate) struct CrLfWriter<'a, W: Write> {
    inner: &'a mut W,
}

pub(crate) fn install_terminal_recovery() {
    let fd = libc::STDIN_FILENO;
    if unsafe { libc::isatty(fd) } != 1 {
        return;
    }
    let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut original) } < 0 {
        return;
    }
    let mut original_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if original_flags < 0 {
        return;
    }

    // SIGKILL cannot be caught, so a previous cosh-shell session may have
    // left the outer tty in raw mode. Heal that residue before saving the
    // "original" state used by panic/signal recovery paths.
    if termios_looks_like_raw_mode(&original) {
        tracing::warn!("detected stale raw mode from a previous session, self-healing terminal");
        restore_minimal_sane_terminal_modes(fd);
        clear_nonblock(fd);
        unsafe {
            libc::write(
                libc::STDOUT_FILENO,
                crate::shell_host::MODIFY_OTHER_KEYS_DISABLE.as_ptr().cast(),
                crate::shell_host::MODIFY_OTHER_KEYS_DISABLE.len(),
            );
        }
        // Re-sample the terminal so the recovery snapshot is sane, not raw.
        if unsafe { libc::tcgetattr(fd, &mut original) } < 0 {
            return;
        }
        let flags_after_heal = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags_after_heal >= 0 {
            original_flags = flags_after_heal;
        }
    }

    unsafe { ORIGINAL_TERMIOS = Some(original) };
    ORIGINAL_FILE_STATUS_FLAGS.store(original_flags, Ordering::Release);

    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));

    unsafe {
        libc::signal(
            libc::SIGINT,
            restore_and_exit as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            restore_and_exit as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGHUP,
            restore_and_exit as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGQUIT,
            restore_and_exit as *const () as libc::sighandler_t,
        );
    }
}

fn termios_looks_like_raw_mode(termios: &libc::termios) -> bool {
    let required_off = libc::ECHO | libc::ICANON | libc::ISIG;
    termios.c_lflag & required_off == 0
}

fn restore_minimal_sane_terminal_modes(fd: i32) {
    let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut termios) } < 0 {
        return;
    }
    termios.c_lflag |= libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN;
    termios.c_iflag |= libc::ICRNL | libc::IXON;
    termios.c_oflag |= libc::OPOST;
    unsafe {
        // Best-effort: if the tty cannot be configured, the subsequent
        // cosh-shell session will still attempt to enter raw mode and the
        // user will see the failure surface.
        let _ = libc::tcsetattr(fd, libc::TCSANOW, &termios);
    }
}

fn clear_nonblock(fd: i32) {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags >= 0 && flags & libc::O_NONBLOCK != 0 {
        unsafe {
            let _ = libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
        }
    }
}

fn restore_terminal() {
    unsafe {
        let original_flags = ORIGINAL_FILE_STATUS_FLAGS.load(Ordering::Acquire);
        if original_flags >= 0 {
            // RawModeGuard temporarily forces blocking I/O for its cleanup so
            // the withdrawal cannot be lost to EAGAIN, then restores the
            // exact inherited flags after the terminal state is safe.
            libc::fcntl(
                libc::STDIN_FILENO,
                libc::F_SETFL,
                original_flags & !libc::O_NONBLOCK,
            );
        }
        crate::shell_host::restore_raw_mode_signal_state();
        if let Some(ref original) = ORIGINAL_TERMIOS {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, original);
        }
        if original_flags >= 0 {
            libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, original_flags);
        }
    }
}

extern "C" fn restore_and_exit(sig: libc::c_int) {
    restore_terminal();
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

impl<'a, W: Write> CrLfWriter<'a, W> {
    pub(crate) fn new(inner: &'a mut W) -> Self {
        Self { inner }
    }
}

impl<W: Write> Write for CrLfWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for byte in buf {
            if *byte == b'\n' {
                self.inner.write_all(b"\r\n")?;
            } else {
                self.inner.write_all(&[*byte])?;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termios_looks_like_raw_mode_detects_cfmakeraw_signature() {
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        assert!(termios_looks_like_raw_mode(&termios));
        termios.c_lflag = libc::ECHO | libc::ICANON | libc::ISIG;
        assert!(!termios_looks_like_raw_mode(&termios));
    }

    #[test]
    fn termios_looks_like_raw_mode_does_not_flag_partial_noncanonical() {
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        termios.c_lflag = libc::ECHO | libc::ISIG;
        assert!(!termios_looks_like_raw_mode(&termios));
        termios.c_lflag = libc::ICANON;
        assert!(!termios_looks_like_raw_mode(&termios));
    }
}
