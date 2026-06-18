//! Trace subcommand - eBPF-based agent activity tracing

use agentsight::{AgentSight, AgentsightConfig};
use daemonize::Daemonize;
use structopt::StructOpt;

/// Trace subcommand
#[derive(Debug, StructOpt, Clone)]
pub struct TraceCommand {
    /// Enable verbose/debug output.
    #[structopt(short, long)]
    pub verbose: bool,

    /// Run as daemon in background
    #[structopt(long)]
    pub daemon: bool,
    /// PID file path for daemon mode
    #[structopt(long, default_value = "/tmp/agentsight.pid")]
    pub pid_file: String,

    /// Enable file watch probe (monitors .jsonl file opens from traced processes)
    #[structopt(long)]
    pub enable_filewatch: bool,

    /// Path to JSON configuration file
    #[structopt(short, long, default_value = "/etc/agentsight/config.json")]
    pub config: String,
}

impl TraceCommand {
    pub fn execute(&self) {
        // Daemonize if requested
        if self.daemon {
            self.run_as_daemon();
            return;
        }

        self.run_tracing();
    }

    /// Run as daemon process
    fn run_as_daemon(&self) {
        println!("Starting agentsight in daemon mode...");
        println!("PID file: {}", self.pid_file);

        let daemonize = Daemonize::new()
            .pid_file(&self.pid_file)
            .chown_pid_file(true)
            .working_directory("/tmp");

        match daemonize.start() {
            Ok(_) => {
                // We're now in the daemon process
                self.run_tracing();
            }
            Err(e) => {
                eprintln!("Failed to daemonize: {e}");
                std::process::exit(1);
            }
        }
    }

    /// Run the actual tracing logic using AgentSight
    fn run_tracing(&self) {
        // Build AgentSight config (empty target_pids means trace all processes).
        // Note: `traceEnabled=false` from agentsight.json does NOT stop the agent
        // — token consumption (LLM call) data must always be collected by default.
        // The toggle only affects the SLS upload layer (LogtailExporter): when
        // traceEnabled=false, conversation content fields (gen_ai.input.messages /
        // gen_ai.output.messages) are dropped from uploaded records, but token
        // metadata (model, provider, token counts, etc.) is still uploaded.
        let config = AgentsightConfig::new()
            .set_verbose(self.verbose)
            .set_enable_filewatch(self.enable_filewatch)
            .set_config_path(std::path::PathBuf::from(&self.config));

        // Create AgentSight (auto-attaches probes and starts polling)
        let mut sight = match AgentSight::new(config) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to create AgentSight: {e}");
                std::process::exit(1);
            }
        };

        // Register Ctrl+C handler for graceful shutdown.
        // This ensures AgentSight is dropped normally, which triggers
        // Storage::drop → WAL checkpoint, flushing data to the main db file.
        let running = sight.running_flag();
        ctrlc::set_handler(move || {
            log::info!("Ctrl+C received, shutting down gracefully...");
            running.store(false, std::sync::atomic::Ordering::SeqCst);
        })
        .expect("Failed to set Ctrl+C handler");

        // Run event loop (blocks until running flag is set to false)
        match sight.run() {
            Ok(count) => {
                println!("\nReceived {count} events total");
                println!("Token usage data saved. Use 'agentsight token' to query.");
            }
            Err(e) => {
                eprintln!("Error during tracing: {e}");
                std::process::exit(1);
            }
        }
        // `sight` drops here → Storage::drop → checkpoint
    }
}
