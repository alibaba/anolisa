//! Debug logging for cosh-tui
//! Ctrl+O toggles debug mode, logs saved to ~/.copilot-shell/logs/

use std::path::PathBuf;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn toggle_debug() -> bool {
    let new_val = !DEBUG_ENABLED.load(Ordering::Relaxed);
    DEBUG_ENABLED.store(new_val, Ordering::Relaxed);
    new_val
}

#[allow(dead_code)]
pub fn is_debug() -> bool {
    DEBUG_ENABLED.load(Ordering::Relaxed)
}

pub fn log_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".copilot-shell")
        .join("logs")
}

#[allow(dead_code)]
pub fn log(level: &str, msg: &str) {
    if !is_debug() { return; }
    let dir = log_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("cosh-tui.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{}] [{}] {}", ts, level, msg);
    }
}

#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        $crate::logger::log("DEBUG", &format!($($arg)*));
    };
}

#[macro_export]
macro_rules! error_log {
    ($($arg:tt)*) => {
        $crate::logger::log("ERROR", &format!($($arg)*));
    };
}
