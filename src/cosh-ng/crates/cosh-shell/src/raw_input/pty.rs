use std::io;

use nix::libc;
use nix::pty::Winsize;

pub(crate) fn set_pty_winsize(fd: i32, winsize: Winsize) -> io::Result<()> {
    let result = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as libc::c_ulong, &winsize) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn signal_process_group(child_pid: u32, signal: i32) -> io::Result<()> {
    let result = unsafe { libc::kill(-(child_pid as i32), signal) };
    if result < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}
