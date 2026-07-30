//! Serve subcommand — start the API server
//!
//! Linux: full eBPF server with SQLite, auth, health routes.
//! macOS: delegates to agentsight::local::server (trajectory viewer).

use structopt::StructOpt;

/// Start the AgentSight API server
#[derive(Debug, StructOpt, Clone)]
pub struct ServeCommand {
    /// Host to bind to
    #[structopt(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Port to bind to
    #[structopt(long, default_value = "7396")]
    pub port: u16,

    /// Custom database path (Linux only)
    #[cfg(target_os = "linux")]
    #[structopt(long)]
    pub db: Option<String>,

    /// Path to JSON configuration file (Linux only)
    #[cfg(target_os = "linux")]
    #[structopt(long, default_value = super::DEFAULT_CONFIG_PATH)]
    pub config: String,
}

impl ServeCommand {
    pub fn execute(&self) {
        let host = self.host.clone();
        let port = self.port;

        #[cfg(target_os = "linux")]
        {
            use agentsight::server::run_server;
            use agentsight::storage::sqlite::GenAISqliteStore;

            let db_path = self
                .db
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(GenAISqliteStore::default_path);

            let auth_config = super::load_server_auth_config(&self.config);

            actix_web::rt::System::new().block_on(async move {
                if let Err(e) = run_server(&host, port, db_path, auth_config).await {
                    eprintln!("Server error: {e}");
                    std::process::exit(1);
                }
            });
        }

        #[cfg(not(target_os = "linux"))]
        {
            actix_web::rt::System::new().block_on(async move {
                if let Err(e) = agentsight::local::server::run_server(&host, port).await {
                    eprintln!("Server error: {e}");
                    std::process::exit(1);
                }
            });
        }
    }
}
