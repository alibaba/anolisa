//! Serve subcommand — start the API server

use agentsight::server::run_server;
use agentsight::storage::sqlite::GenAISqliteStore;
use structopt::StructOpt;

use super::{DEFAULT_CONFIG_PATH, load_server_auth_config};

/// Start the AgentSight API server
#[derive(Debug, StructOpt, Clone)]
pub struct ServeCommand {
    /// Host to bind to
    #[structopt(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Port to bind to
    #[structopt(long, default_value = "7396")]
    pub port: u16,

    /// Custom database path
    #[structopt(long)]
    pub db: Option<String>,

    /// Path to JSON configuration file
    #[structopt(long, default_value = DEFAULT_CONFIG_PATH)]
    pub config: String,
}

impl ServeCommand {
    pub fn execute(&self) {
        let db_path = self
            .db
            .as_ref()
            .map(std::path::PathBuf::from)
            // Default to genai_events.db — the same file the tracer writes to
            .unwrap_or_else(GenAISqliteStore::default_path);

        let host = self.host.clone();
        let port = self.port;

        // Load server.auth.enabled from config file (same source as `trace`)
        let auth_config = load_server_auth_config(&self.config);

        actix_web::rt::System::new().block_on(async move {
            if let Err(e) = run_server(&host, port, db_path, auth_config).await {
                eprintln!("Server error: {e}");
                std::process::exit(1);
            }
        });
    }
}
