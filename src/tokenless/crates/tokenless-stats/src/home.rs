//! Home-directory resolution rooted in the passwd database.
//!
//! Every tokenless crate that writes state under the user's home (config,
//! stats DB, log files) must agree on what "home" means. Reading `$HOME`
//! directly is unsafe — a parent process can set it to anything before
//! invoking the binary, redirecting state files into attacker-writable
//! paths. This module derives the home directory from `getpwuid_r(getuid())`
//! and falls back to `dirs::home_dir` only when the passwd lookup fails
//! (e.g. minimal containers without an `/etc/passwd` entry).

/// Resolve the current user's home directory.
///
/// Returns an empty string when no trusted home anchor is available; callers
/// must treat that as "no $HOME-relative writes" rather than using `.` as a
/// fallback (which would silently place state wherever the binary was
/// invoked from).
pub fn get_home_dir() -> String {
    #[cfg(unix)]
    if let Some(home) = home_dir_from_passwd() {
        return home;
    }
    dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

#[cfg(unix)]
fn home_dir_from_passwd() -> Option<String> {
    use std::ffi::CStr;
    // SAFETY: getuid is infallible and always safe. getpwuid_r is the
    // thread-safe variant: we hand it a stack-allocated passwd struct and
    // a 4 KiB heap buffer, and it never writes past the buffer length we
    // pass. result is left null when no entry is found, which we detect.
    let uid = unsafe { libc::getuid() };
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 4096];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into our buf and is NUL-terminated by the libc
    // contract. The CStr borrow is short-lived; we copy the bytes out before
    // pwd/buf are dropped.
    let home = unsafe { CStr::from_ptr(pwd.pw_dir) }.to_str().ok()?;
    (!home.is_empty()).then(|| home.to_string())
}
