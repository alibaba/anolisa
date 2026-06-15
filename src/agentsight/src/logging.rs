//! Process-global logger with swappable output for repeated `init_logging` calls.
//!
//! `env_logger::try_init()` only succeeds once per process. AgentSight may call
//! `init_logging` on every `AgentSight::new` (FFI new+start cycles), so we
//! install a custom `log::Log` once and reconfigure its filter + writer on later calls.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

use env_filter::Filter;
use log::{LevelFilter, Log, Metadata, Record};

static LOGGER: OnceLock<AgentsightLogger> = OnceLock::new();

/// Initialize or reconfigure process logging.
///
/// * `verbose` — true = debug, false = warn (unless `RUST_LOG` is set)
/// * `log_path` — append to this file when set; otherwise stderr
pub fn init(verbose: bool, log_path: Option<&str>) {
    let filter = build_filter(verbose);
    let writer = open_log_writer(log_path);

    if let Some(logger) = LOGGER.get() {
        logger.reconfigure(filter, writer);
        return;
    }

    let max_level = filter.filter();
    let logger = AgentsightLogger::new(filter, writer);
    if LOGGER.set(logger).is_ok() {
        log::set_max_level(max_level);
        if let Err(err) = log::set_logger(LOGGER.get().expect("logger just set")) {
            eprintln!("agentsight: failed to install logger: {err}");
        }
    } else if let Some(logger) = LOGGER.get() {
        // Lost a concurrent first-time install race; apply this caller's config.
        logger.reconfigure(build_filter(verbose), open_log_writer(log_path));
    }
}

fn build_filter(verbose: bool) -> Filter {
    if let Ok(rust_log) = std::env::var("RUST_LOG") {
        let mut builder = env_filter::Builder::new();
        match builder.try_parse(&rust_log) {
            Ok(_) => builder.build(),
            Err(e) => {
                eprintln!("agentsight: invalid RUST_LOG={rust_log:?}: {e}");
                default_filter(verbose)
            }
        }
    } else {
        default_filter(verbose)
    }
}

fn default_filter(verbose: bool) -> Filter {
    let level = if verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Warn
    };
    env_filter::Builder::new().filter_level(level).build()
}

fn open_log_writer(log_path: Option<&str>) -> LogWriter {
    let Some(path) = log_path else {
        return LogWriter::Stderr;
    };

    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => LogWriter::File(file),
        Err(e) => {
            eprintln!("agentsight: failed to open log file {path:?}: {e}");
            LogWriter::Stderr
        }
    }
}

enum LogWriter {
    Stderr,
    File(File),
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            LogWriter::Stderr => io::stderr().write(buf),
            LogWriter::File(file) => file.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            LogWriter::Stderr => io::stderr().flush(),
            LogWriter::File(file) => file.flush(),
        }
    }
}

struct AgentsightLogger {
    filter: Mutex<Filter>,
    writer: Mutex<LogWriter>,
}

impl AgentsightLogger {
    fn new(filter: Filter, writer: LogWriter) -> Self {
        Self {
            filter: Mutex::new(filter),
            writer: Mutex::new(writer),
        }
    }

    fn reconfigure(&self, filter: Filter, writer: LogWriter) {
        log::set_max_level(filter.filter());
        *self.filter.lock().expect("log filter lock poisoned") = filter;
        *self.writer.lock().expect("log writer lock poisoned") = writer;
    }
}

impl Log for AgentsightLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.filter
            .lock()
            .expect("log filter lock poisoned")
            .enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let line = format_record(record);
        let mut writer = self.writer.lock().expect("log writer lock poisoned");
        let _ = writeln!(writer, "{line}");
    }

    fn flush(&self) {
        let mut writer = self.writer.lock().expect("log writer lock poisoned");
        let _ = writer.flush();
    }
}

fn format_record(record: &Record) -> String {
    use std::fmt::Write as _;

    let mut line = String::new();
    let _ = write!(
        line,
        "[{} {:5}",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ"),
        record.level(),
    );
    if let Some(path) = record.module_path() {
        let _ = write!(line, " {path}");
    }
    if let Some(line_no) = record.line() {
        let _ = write!(line, ":{line_no}");
    }
    let _ = write!(line, "] {}", record.args());
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn build_filter_respects_verbose_default() {
        let filter = default_filter(false);
        assert_eq!(filter.filter(), LevelFilter::Warn);

        let filter = default_filter(true);
        assert_eq!(filter.filter(), LevelFilter::Debug);
    }

    #[test]
    fn log_writer_file_append_and_write() {
        let dir = std::env::temp_dir().join(format!("agentsight-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path = dir.join("test.log");

        {
            let mut writer = open_log_writer(Some(path.to_str().unwrap()));
            writeln!(writer, "first").expect("write");
        }
        {
            let mut writer = open_log_writer(Some(path.to_str().unwrap()));
            writeln!(writer, "second").expect("write");
        }

        let mut contents = String::new();
        File::open(&path)
            .expect("open log")
            .read_to_string(&mut contents)
            .expect("read log");
        assert!(contents.contains("first"));
        assert!(contents.contains("second"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logger_reconfigure_swaps_writer() {
        let dir =
            std::env::temp_dir().join(format!("agentsight-log-reconfig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path_a = dir.join("a.log");
        let path_b = dir.join("b.log");

        let logger = AgentsightLogger::new(default_filter(true), open_log_writer(None));
        logger.log(
            &Record::builder()
                .args(format_args!("to stderr"))
                .level(log::Level::Info)
                .target("test")
                .build(),
        );

        logger.reconfigure(
            default_filter(true),
            open_log_writer(Some(path_a.to_str().unwrap())),
        );
        logger.log(
            &Record::builder()
                .args(format_args!("to a"))
                .level(log::Level::Info)
                .target("test")
                .build(),
        );

        logger.reconfigure(
            default_filter(true),
            open_log_writer(Some(path_b.to_str().unwrap())),
        );
        logger.log(
            &Record::builder()
                .args(format_args!("to b"))
                .level(log::Level::Info)
                .target("test")
                .build(),
        );

        let a = std::fs::read_to_string(&path_a).expect("read a");
        assert!(a.contains("to a"));
        assert!(!a.contains("to b"));

        let b = std::fs::read_to_string(&path_b).expect("read b");
        assert!(b.contains("to b"));
        assert!(!b.contains("to a"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
