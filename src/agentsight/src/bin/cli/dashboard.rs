//! Dashboard subcommand — display dashboard URL, auth status, and ECS access guide

use std::net::TcpStream;
use std::time::Duration;

use agentsight::ecs_metadata::{EcsMetadata, probe_ecs_metadata};
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

        let output = self.build_output();

        println!();
        for line in &output.lines {
            println!("{line}");
        }

        // ECS security group guide
        if let Some(ref msg) = output.sg_message {
            println!("{msg}");
        }

        // Try to open browser
        if !self.no_open {
            try_open_browser(&output.display_url);
        }
    }

    /// Compute all display information without performing I/O.
    fn build_output(&self) -> DashboardOutput {
        let local_url = format!("http://127.0.0.1:{}", self.port);

        let ecs = if self.skip_sg_guide {
            None
        } else {
            probe_ecs_metadata()
        };

        let display_url = resolve_display_url(&ecs, &local_url, self.port);

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

        let mut lines = Vec::new();
        lines.push(format!("AgentSight Dashboard: {display_url}"));
        if display_url != local_url {
            lines.push(format!("  Local:   {local_url} (no auth required)"));
        }
        lines.push(String::new());

        if auth.enabled
            && let Some(token) = auth.read_token_from_file()
        {
            lines.push("  Auth:      enabled".to_string());
            lines.push(format!("  URL with token: {display_url}/?token={token}"));
            lines.push(String::new());
        }

        let sg_message = build_sg_message(&ecs, self.skip_sg_guide, self.port);

        DashboardOutput {
            display_url,
            lines,
            sg_message,
        }
    }
}

/// Pre-computed display data for the dashboard command.
#[derive(Debug)]
struct DashboardOutput {
    display_url: String,
    lines: Vec<String>,
    sg_message: Option<String>,
}

/// Determine the display URL: ECS public IP > non-loopback local IP > localhost.
fn resolve_display_url(ecs: &Option<EcsMetadata>, local_url: &str, port: u16) -> String {
    if let Some(meta) = ecs {
        meta.public_ip()
            .map(|ip| format!("http://{ip}:{port}"))
            .unwrap_or_else(|| local_url.to_string())
    } else {
        local_addresses()
            .into_iter()
            .next()
            .map(|ip| format!("http://{ip}:{port}"))
            .unwrap_or_else(|| local_url.to_string())
    }
}

/// Build the security group guide message, if applicable.
fn build_sg_message(ecs: &Option<EcsMetadata>, skip: bool, port: u16) -> Option<String> {
    match ecs {
        Some(meta) => {
            let mut msg = String::new();
            msg.push_str(&format!(
                "远程打不开？请前往实例控制台配置安全组，放行 TCP {port}：\n"
            ));
            msg.push_str(&format!("  {}", meta.instance_url()));
            msg.push('\n');
            Some(msg)
        }
        None if !skip => Some(format!(
            "未检测到 ECS 环境，请手动确保防火墙/安全组已放行 {port} 端口。\n"
        )),
        None => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn find_executable_returns_path_for_known_command() {
        // "ls" should exist on any Linux system
        let result = find_executable("ls");
        assert!(result.is_some(), "ls should be found in PATH");
        let path = result.unwrap();
        assert!(
            std::path::Path::new(&path).is_file(),
            "returned path should be a file: {path}"
        );
    }

    #[test]
    fn find_executable_returns_none_for_nonexistent_command() {
        let result = find_executable("__nonexistent_binary_xyz__");
        assert!(result.is_none(), "nonexistent command should return None");
    }

    #[test]
    fn local_addresses_returns_valid_ipv4_or_empty() {
        let addrs = local_addresses();
        for addr in &addrs {
            let parsed: Result<std::net::Ipv4Addr, _> = addr.parse();
            assert!(parsed.is_ok(), "each address should be valid IPv4: {addr}");
            let ip = parsed.unwrap();
            assert!(!ip.is_loopback(), "should exclude loopback: {ip}");
            assert!(!ip.is_unspecified(), "should exclude unspecified: {ip}");
        }
    }

    #[test]
    fn check_server_running_returns_false_when_no_listener() {
        // Pick a port that is almost certainly not in use
        let result = check_server_running(1);
        assert!(
            !result,
            "port 1 should not have a listener, so check should return false"
        );
    }

    #[test]
    fn check_server_running_returns_true_with_active_listener() {
        // Bind a TCP listener on a random port
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind");
        let port = listener.local_addr().unwrap().port();
        let result = check_server_running(port);
        assert!(result, "should detect the active listener on port {port}");
    }

    #[test]
    fn resolve_display_url_with_ecs_public_ip() {
        let ecs = Some(EcsMetadata {
            instance_id: "i-test".to_string(),
            region_id: "cn-hangzhou".to_string(),
            eip: "1.2.3.4".to_string(),
            public_ipv4: String::new(),
        });
        let url = resolve_display_url(&ecs, "http://127.0.0.1:7396", 7396);
        assert_eq!(url, "http://1.2.3.4:7396");
    }

    #[test]
    fn resolve_display_url_falls_back_to_local_when_no_public_ip() {
        let ecs = Some(EcsMetadata {
            instance_id: "i-test".to_string(),
            region_id: "cn-hangzhou".to_string(),
            eip: String::new(),
            public_ipv4: String::new(),
        });
        let url = resolve_display_url(&ecs, "http://127.0.0.1:7396", 7396);
        assert_eq!(url, "http://127.0.0.1:7396");
    }

    #[test]
    fn resolve_display_url_without_ecs() {
        let url = resolve_display_url(&None, "http://127.0.0.1:7396", 7396);
        // Should use local_addresses or fall back to localhost
        assert!(url.starts_with("http://"));
        assert!(url.ends_with(":7396"));
    }

    #[test]
    fn build_sg_message_with_ecs_metadata() {
        let ecs = Some(EcsMetadata {
            instance_id: "i-abc123".to_string(),
            region_id: "cn-hangzhou".to_string(),
            eip: "1.2.3.4".to_string(),
            public_ipv4: String::new(),
        });
        let msg = build_sg_message(&ecs, false, 7396);
        let msg = msg.expect("should produce a message");
        assert!(msg.contains("7396"));
        assert!(msg.contains("i-abc123"));
        assert!(msg.contains("安全组"));
    }

    #[test]
    fn build_sg_message_none_when_skipped() {
        let msg = build_sg_message(&None, true, 7396);
        assert!(msg.is_none(), "skip=true should produce None");
    }

    #[test]
    fn build_sg_message_without_ecs() {
        let msg = build_sg_message(&None, false, 7396);
        let msg = msg.expect("should produce a message when not skipped");
        assert!(msg.contains("7396"));
        assert!(msg.contains("未检测到"));
    }
}
