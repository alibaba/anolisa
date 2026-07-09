//! Dashboard subcommand — display dashboard authentication status

use agentsight::server::auth::DashboardAuth;
use structopt::StructOpt;

use super::{DEFAULT_CONFIG_PATH, load_server_auth_config};

/// Display the current AgentSight dashboard status (auth, token, URL)
#[derive(Debug, StructOpt, Clone)]
pub struct DashboardCommand {
    /// Custom database path (used to locate the token file)
    #[structopt(long)]
    pub db: Option<String>,

    /// Host the server is bound to
    #[structopt(long, default_value = "0.0.0.0")]
    pub host: String,

    /// Port the server is listening on
    #[structopt(long, default_value = "7396")]
    pub port: u16,

    /// Path to JSON configuration file
    #[structopt(long, default_value = DEFAULT_CONFIG_PATH)]
    pub config: String,
}

impl DashboardCommand {
    pub fn execute(&self) {
        let storage_base = self
            .db
            .as_ref()
            .map(std::path::PathBuf::from)
            .and_then(|p| p.parent().map(|pp| pp.to_path_buf()))
            .unwrap_or_else(|| {
                agentsight::storage::sqlite::GenAISqliteStore::default_path()
                    .parent()
                    .unwrap_or(std::path::Path::new("/var/log/sysak/.agentsight"))
                    .to_path_buf()
            });

        // Load server.auth.enabled from config file (same source as `serve`)
        let auth_config = load_server_auth_config(&self.config);
        let auth = DashboardAuth::init(&auth_config, &storage_base);

        println!("AgentSight Dashboard Status");
        println!("===========================");
        println!();

        if auth.enabled {
            println!("  Auth:      enabled");
        } else {
            println!("  Auth:      disabled");
        }

        // Display URLs
        println!(
            "  Local:     http://127.0.0.1:{} (no auth required)",
            self.port
        );
        let net_ip = local_addresses().into_iter().next();
        match (net_ip, auth.read_token_from_file()) {
            (Some(ip), Some(token)) => {
                println!("  Network:   http://{}:{}/?token={}", ip, self.port, token);
            }
            (Some(ip), None) => {
                println!("  Network:   http://{}:{}", ip, self.port);
            }
            _ => println!("  Network:   (no non-loopback interface found)"),
        }
        println!();
    }
}

/// Get non-loopback local IP addresses for URL display.
fn local_addresses() -> Vec<String> {
    let Ok(output) = std::process::Command::new("ip")
        .args(["-4", "addr", "show"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("inet ")
                .and_then(|r| r.split('/').next())
        })
        .filter_map(|ip_str| ip_str.trim().parse::<std::net::Ipv4Addr>().ok())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .map(|ip| ip.to_string())
        .collect()
}
