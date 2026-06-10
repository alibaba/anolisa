use crate::cli::CliArgs;
use crate::config::CoreConfig;

pub async fn run(_args: &CliArgs, _config: CoreConfig) {
    eprintln!("[cosh-tui] Interactive TUI mode is not yet implemented.");
    eprintln!("[cosh-tui] Use --headless or pipe input via stdin for JSONL mode.");
    std::process::exit(1);
}
