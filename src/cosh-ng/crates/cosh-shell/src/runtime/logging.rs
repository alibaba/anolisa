use tracing_subscriber::EnvFilter;

pub(crate) fn init_logging(config_log_level: &str) {
    let log_dir = log_directory();

    let filter = if let Ok(cosh_log) = std::env::var("COSH_LOG") {
        EnvFilter::try_new(&cosh_log).unwrap_or_else(|_| EnvFilter::new("warn"))
    } else if let Ok(rust_log) = std::env::var("RUST_LOG") {
        EnvFilter::try_new(&rust_log).unwrap_or_else(|_| EnvFilter::new("warn"))
    } else {
        EnvFilter::try_new(config_log_level).unwrap_or_else(|_| EnvFilter::new("warn"))
    };

    if let Some(dir) = &log_dir {
        let _ = std::fs::create_dir_all(dir);
        cleanup_old_logs(dir, 7);
        let file_appender = tracing_appender::rolling::daily(dir, "cosh-shell.log");
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(file_appender)
            .with_ansi(false)
            .with_target(true)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_target(true)
            .init();
    }
}

fn log_directory() -> Option<std::path::PathBuf> {
    dirs_next_or_home().map(|h| h.join(".copilot-shell/logs"))
}

fn dirs_next_or_home() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

fn cleanup_old_logs(dir: &std::path::Path, keep_days: u64) {
    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(keep_days * 24 * 3600);
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().map_or(false, |e| e.len() == 10) {
            continue;
        }
        if let Ok(meta) = path.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < cutoff {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}
