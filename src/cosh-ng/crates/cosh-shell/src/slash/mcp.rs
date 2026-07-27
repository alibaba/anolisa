use std::io::Write;
use std::process::{Command, Stdio};

use crate::runtime::prelude::*;
use crate::slash::panel::render_notice_panel;

pub(super) fn render_mcp_command<W: Write>(
    sub: Option<&str>,
    arg: Option<&str>,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let i18n = state.i18n();
    let title = i18n.t(MessageId::SlashMcpTitle);

    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        return render_notice_panel(
            output,
            title,
            vec![i18n.t(MessageId::SlashRegistryUnavailable).to_string()],
            None,
        );
    };

    let action = sub.unwrap_or("list");

    match action {
        "list" | "connect" | "inspect" | "refresh" | "disconnect" | "login" | "logout" => {
            let mut cmd_args = vec!["mcp", action];
            if let Some(server) = arg {
                cmd_args.push(server);
            }
            run_mcp_subprocess(&cosh_core.program, &cmd_args, title, output)
        }
        "help" | "--help" | "-h" => render_notice_panel(
            output,
            title,
            vec![
                "Usage: /mcp <command> [server]".to_string(),
                String::new(),
                "Commands:".to_string(),
                "  list         List configured MCP servers".to_string(),
                "  connect      Connect to an MCP server".to_string(),
                "  inspect      Inspect an MCP server's tools".to_string(),
                "  refresh      Refresh an MCP server's tool list".to_string(),
                "  disconnect   Disconnect an MCP server".to_string(),
                "  login        Authorize an MCP server (OAuth)".to_string(),
                "  logout       Remove saved OAuth credentials".to_string(),
            ],
            None,
        ),
        _ => render_notice_panel(
            output,
            title,
            vec![
                format!("Unknown subcommand: {action}"),
                "Run /mcp help for usage information.".to_string(),
            ],
            None,
        ),
    }
}

fn run_mcp_subprocess<W: Write>(
    program: &str,
    args: &[&str],
    title: &str,
    output: &mut W,
) -> std::io::Result<()> {
    let result = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match result {
        Ok(output_result) => {
            let stdout = String::from_utf8_lossy(&output_result.stdout);
            let stderr = String::from_utf8_lossy(&output_result.stderr);

            if output_result.status.success() {
                let body = format_mcp_output(&stdout);
                render_notice_panel(output, title, body, None)
            } else {
                let error_msg = if stderr.is_empty() {
                    format!("Command failed with exit code: {}", output_result.status)
                } else {
                    stderr.trim().to_string()
                };
                render_notice_panel(output, title, vec![error_msg], None)
            }
        }
        Err(error) => render_notice_panel(
            output,
            title,
            vec![format!("Failed to run cosh-core: {error}")],
            None,
        ),
    }
}

fn format_mcp_output(stdout: &str) -> Vec<String> {
    // Try to parse as JSON and format nicely
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        format_mcp_json(&json)
    } else {
        // Fall back to raw output lines
        stdout
            .lines()
            .map(|line| line.to_string())
            .filter(|line| !line.is_empty())
            .collect()
    }
}

fn format_mcp_json(json: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(servers) = json.get("servers").and_then(|v| v.as_array()) {
        if servers.is_empty() {
            lines.push("No MCP servers configured.".to_string());
            return lines;
        }
        for server in servers {
            let name = server
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let status = server
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let transport = server
                .get("transport")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            lines.push(format!("  {name}  [{status}]  {transport}"));

            if let Some(tools) = server.get("tools").and_then(|v| v.as_array()) {
                if !tools.is_empty() {
                    lines.push(format!("    Tools ({}):", tools.len()));
                    for tool in tools.iter().take(20) {
                        let tool_name = tool
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        lines.push(format!("      - {tool_name}"));
                    }
                    if tools.len() > 20 {
                        lines.push(format!("      ... and {} more", tools.len() - 20));
                    }
                }
            }
        }
    } else if let Some(action) = json.get("action").and_then(|v| v.as_str()) {
        lines.push(format!("Action: {action}"));
        if let Some(server) = json.get("server").and_then(|v| v.as_str()) {
            lines.push(format!("Server: {server}"));
        }
        if let Some(tools) = json.get("tools").and_then(|v| v.as_array()) {
            lines.push(format!("Tools ({}):", tools.len()));
            for tool in tools.iter().take(20) {
                let name = tool
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                lines.push(format!("  - {name}"));
            }
        }
        // Include any message field
        if let Some(message) = json.get("message").and_then(|v| v.as_str()) {
            lines.push(message.to_string());
        }
    } else {
        // Generic JSON formatting
        lines.push(serde_json::to_string_pretty(json).unwrap_or_else(|_| stdout_to_string(json)));
    }

    lines
}

fn stdout_to_string(json: &serde_json::Value) -> String {
    json.to_string()
}
