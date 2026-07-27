use serde_json::Value;

use crate::runtime::prelude::*;
use crate::slash::panel::render_notice_panel;

pub(super) fn render_mcp_command<W: Write>(
    sub: Option<&str>,
    arg: Option<&str>,
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        let i18n = state.i18n();
        return render_notice_panel(
            output,
            i18n.t(MessageId::SlashMcpTitle),
            vec![i18n.t(MessageId::SlashRegistryUnavailable).to_string()],
            None,
        );
    };

    let action = sub.unwrap_or("list");
    let i18n = state.i18n();

    match action {
        "list" => {
            let params = Value::Null;
            match cosh_core.registry_query("mcp", "list", params) {
                Ok(data) => {
                    let body = format_mcp_list(&data, &i18n);
                    render_notice_panel(output, i18n.t(MessageId::SlashMcpTitle), body, None)
                }
                Err(e) => render_notice_panel(
                    output,
                    i18n.t(MessageId::SlashMcpTitle),
                    vec![format!("Error: {e}")],
                    None,
                ),
            }
        }
        "connect" | "inspect" | "refresh" | "disconnect" | "login" | "logout" => {
            let server = arg.unwrap_or("");
            if server.is_empty() {
                return render_notice_panel(
                    output,
                    i18n.t(MessageId::SlashMcpTitle),
                    vec![format!("Usage: /mcp {action} <name>")],
                    None,
                );
            }
            let params = serde_json::json!({ "server": server });
            match cosh_core.registry_query("mcp", action, params) {
                Ok(data) => {
                    let body = format_mcp_action_result(action, server, &data);
                    render_notice_panel(output, i18n.t(MessageId::SlashMcpTitle), body, None)
                }
                Err(e) => render_notice_panel(
                    output,
                    i18n.t(MessageId::SlashMcpTitle),
                    vec![format!("Error: {e}")],
                    None,
                ),
            }
        }
        _ => render_notice_panel(
            output,
            i18n.t(MessageId::SlashMcpTitle),
            vec![
                "Usage: /mcp <subcommand>".to_string(),
                "  list                    List configured MCP servers".to_string(),
                "  connect <name>          Connect to an MCP server".to_string(),
                "  inspect <name>          Show server tools".to_string(),
                "  refresh <name>          Refresh server tools".to_string(),
                "  disconnect <name>       Disconnect an MCP server".to_string(),
                "  login <name>            OAuth login".to_string(),
                "  logout <name>           OAuth logout".to_string(),
            ],
            None,
        ),
    }
}

fn format_mcp_list(data: &Value, i18n: &I18n) -> Vec<String> {
    let Some(arr) = data.as_array() else {
        return vec![i18n.t(MessageId::SlashMcpEmptyBody).to_string()];
    };
    if arr.is_empty() {
        return vec![i18n.t(MessageId::SlashMcpEmptyBody).to_string()];
    }
    arr.iter()
        .filter_map(|server| {
            let name = server.get("server")?.as_str()?;
            let transport = server
                .get("transport")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let enabled = server
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let has_creds = server
                .get("has_credentials")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let status = if !enabled {
                " [disabled]"
            } else if has_creds {
                " [authenticated]"
            } else {
                ""
            };
            Some(format!("  • {name} ({transport}){status}"))
        })
        .collect()
}

fn format_mcp_action_result(action: &str, server: &str, data: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    match action {
        "connect" | "inspect" | "refresh" => {
            let action_label = match action {
                "connect" => "Connected",
                "inspect" => "Inspected",
                "refresh" => "Refreshed",
                _ => unreachable!(),
            };
            lines.push(format!("  {action_label}: {server}"));
            if let Some(transport) = data.get("transport").and_then(|v| v.as_str()) {
                lines.push(format!("  Transport: {transport}"));
            }
            if let Some(tools) = data.get("tools").and_then(|v| v.as_array()) {
                if tools.is_empty() {
                    lines.push("  Tools: none".to_string());
                } else {
                    lines.push(format!("  Tools ({}):", tools.len()));
                    for tool in tools {
                        let name = tool
                            .get("exposed_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        let desc = tool
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let desc_short = if desc.len() > 60 {
                            format!("{}...", &desc[..57])
                        } else {
                            desc.to_string()
                        };
                        lines.push(format!("    - {name}: {desc_short}"));
                    }
                }
            }
        }
        "disconnect" => {
            lines.push(format!("  Disconnected: {server}"));
            let disabled = data
                .get("disabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let creds_removed = data
                .get("credentials_removed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            lines.push(format!("  Disabled: {disabled}"));
            if creds_removed {
                lines.push("  Credentials removed: true".to_string());
            }
        }
        "login" => {
            lines.push(format!("  Logged in: {server}"));
        }
        "logout" => {
            lines.push(format!("  Logged out: {server}"));
        }
        _ => {
            lines.push(format!("  {action}: {server}"));
        }
    }
    lines
}
