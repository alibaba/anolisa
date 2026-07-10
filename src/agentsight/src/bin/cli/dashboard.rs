//! Dashboard subcommand — display dashboard URL, auth status, and ECS access guide

use std::net::TcpStream;
use std::time::Duration;

use agentsight::ecs_metadata::probe_ecs_metadata;
use agentsight::server::auth::DashboardAuth;
use structopt::StructOpt;

use super::{DEFAULT_CONFIG_PATH, load_server_auth_config};

/// Display the AgentSight dashboard URL and ECS access guide
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

    /// Do not attempt to open a browser
    #[structopt(long)]
    pub no_open: bool,

    /// Skip ECS security group guide output
    #[structopt(long)]
    pub skip_sg_guide: bool,

    /// Path to JSON configuration file
    #[structopt(long, default_value = DEFAULT_CONFIG_PATH)]
    pub config: String,
}

impl DashboardCommand {
    pub fn execute(&self) {
        // Check if the server is running
        if !check_server_running(self.port) {
            eprintln!("AgentSight 服务未启动。请先运行 `agentsight serve`。");
            std::process::exit(1);
        }

        // Build dashboard URL
        let local_url = format!("http://127.0.0.1:{}", self.port);

        // Probe ECS metadata (2s timeout)
        let ecs = if self.skip_sg_guide {
            None
        } else {
            probe_ecs_metadata()
        };

        // Determine the display URL: ECS public IP > non-loopback local IP > localhost
        let display_url = if let Some(ref meta) = ecs {
            meta.public_ip()
                .map(|ip| format!("http://{ip}:{}", self.port))
                .unwrap_or_else(|| local_url.clone())
        } else {
            local_addresses()
                .into_iter()
                .next()
                .map(|ip| format!("http://{ip}:{}", self.port))
                .unwrap_or_else(|| local_url.clone())
        };

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

        let auth_config = load_server_auth_config(&self.config);
        let auth = DashboardAuth::init(&auth_config, &storage_base);

        println!();
        println!("AgentSight Dashboard: {display_url}");
        if display_url != local_url {
            println!("  Local:   {local_url} (no auth required)");
        }
        println!();

        if auth.enabled
            && let Some(token) = auth.read_token_from_file()
        {
            println!("  Auth:      enabled");
            println!("  URL with token: {display_url}/?token={token}");
            println!();
        }

        // ECS security group guide
        match ecs {
            Some(meta) => {
                println!(
                    "远程打不开？请前往实例控制台配置安全组，放行 TCP {}：",
                    self.port
                );
                println!("  {}", meta.instance_url());
                println!();
            }
            None => {
                if !self.skip_sg_guide {
                    println!(
                        "未检测到 ECS 环境，请手动确保防火墙/安全组已放行 {} 端口。",
                        self.port
                    );
                    println!();
                }
            }
        }

        // Try to open browser
        if !self.no_open {
            try_open_browser(&display_url);
        }
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

/// Quick TCP connect to check whether the server is listening.
fn check_server_running(port: u16) -> bool {
    TcpStream::connect_timeout(
        &std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port),
        Duration::from_millis(500),
    )
    .is_ok()
}

/// Try to open a URL in the default browser.
fn try_open_browser(url: &str) {
    let opener = find_executable("xdg-open");
    if let Some(bin) = opener {
        let _ = std::process::Command::new(bin)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Check whether an executable exists in `$PATH`.
fn find_executable(name: &str) -> Option<String> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    path_var
        .split(':')
        .map(|dir| format!("{dir}/{name}"))
        .find(|full| std::path::Path::new(full).is_file())
}
