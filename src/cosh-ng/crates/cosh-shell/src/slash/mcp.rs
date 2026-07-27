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
    let title = i18n.t(MessageId::SlashMcpTitle);

    match action {
        "list" => match cosh_core.registry_query("mcp", "list", Value::Null) {
            Ok(data) => {
                let body = format_mcp_list(&data, &i18n);
                render_notice_panel(output, title, body, None)
            }
            Err(error) => render_notice_panel(
                output,
                title,
                vec![format_error(&i18n, &error)],
                None,
            ),
        },
        "connect" | "inspect" | "refresh" => {
            let server = arg.unwrap_or("");
            if server.is_empty() {
                return render_notice_panel(
                    output,
                    title,
                    vec![format!("Usage: /mcp {action} <name>")],
                    None,
                );
            }
            let params = serde_json::json!({ "server": server });
            match cosh_core.registry_query("mcp", action, params) {
                Ok(data) => {
                    let body = format_inspection(&data, action);
                    render_notice_panel(output, title, body, None)
                }
                Err(error) => render_notice_panel(
                    output,
                    title,
                    vec![format_error(&i18n, &error)],
                    None,
                ),
            }
        }
        "disconnect" => {
            let server = arg.unwrap_or("");
            if server.is_empty() {
                return render_notice_panel(
                    output,
                    title,
                    vec!["Usage: /mcp disconnect <name>".to_string()],
                    None,
                );
            }
            let params = serde_json::json!({ "server": server });
            match cosh_core.registry_query("mcp", "disconnect", params) {
                Ok(data) => {
                    let body = format_disconnect(&data, server);
                    render_notice_panel(output, title, body, None)
                }
                Err(error) => render_notice_panel(
                    output,
                    title,
                    vec![format_error(&i18n, &error)],
                    None,
                ),
            }
        }
        "login" => {
            let server = arg.unwrap_or("");
            if server.is_empty() {
                return render_notice_panel(
                    output,
                    title,
                    vec!["Usage: /mcp login <name>".to_string()],
                    None,
                );
            }
            render_notice_panel(
                output,
                title,
                vec![format!(
                    "OAuth login for MCP server '{server}' is not supported in TUI.\n  Use: cosh-core mcp login {server}"
                )],
                None,
            )
        }
        "logout" => {
            let server = arg.unwrap_or("");
            if server.is_empty() {
                return render_notice_panel(
                    output,
                    title,
                    vec!["Usage: /mcp logout <name>".to_string()],
                    None,
                );
            }
            let params = serde_json::json!({ "server": server });
            match cosh_core.registry_query("mcp", "logout", params) {
                Ok(data) => {
                    let removed = data
                        .get("credentials_removed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let message = if removed {
                        format!("  OAuth credentials for '{server}' removed.")
                    } else {
                        format!("  No OAuth credentials found for '{server}'.")
                    };
                    render_notice_panel(output, title, vec![message], None)
                }
                Err(error) => render_notice_panel(
                    output,
                    title,
                    vec![format_error(&i18n, &error)],
                    None,
                ),
            }
        }
        _ => render_notice_panel(
            output,
            title,
            vec![
                "Usage: /mcp <subcommand>".to_string(),
                "  list                         List configured MCP servers".to_string(),
                "  connect <name>               Connect to an MCP server".to_string(),
                "  inspect <name>               Show server tools".to_string(),
                "  refresh <name>               Refresh server tools".to_string(),
                "  disconnect <name>            Disconnect an MCP server".to_string(),
                "  login <name>                 OAuth login (CLI only)".to_string(),
                "  logout <name>                Remove OAuth credentials".to_string(),
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
            if enabled {
                let creds = if has_creds { " [auth]" } else { "" };
                Some(format!("  • {name} [{transport}]{creds}"))
            } else {
                Some(format!("  ○ {name} [{transport}] [disabled]"))
            }
        })
        .collect()
}

fn format_inspection(data: &Value, action: &str) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(server) = data.get("server").and_then(Value::as_str) {
        lines.push(format!("  Server: {server}"));
    }
    lines.push(format!("  Action: {action}"));
    if let Some(transport) = data.get("transport").and_then(Value::as_str) {
        lines.push(format!("  Transport: {transport}"));
    }
    if let Some(tools) = data.get("tools").and_then(Value::as_array) {
        if tools.is_empty() {
            lines.push("  Tools: none".to_string());
        } else {
            lines.push(format!("  Tools ({}):", tools.len()));
            for tool in tools {
                let name = tool.get("name").and_then(Value::as_str).unwrap_or("?");
                let exposed = tool
                    .get("exposed_name")
                    .and_then(Value::as_str)
                    .unwrap_or(name);
                let desc = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if desc.is_empty() {
                    lines.push(format!("    • {exposed}"));
                } else {
                    let short_desc: String = desc.chars().take(60).collect();
                    lines.push(format!("    • {exposed} — {short_desc}"));
                }
            }
        }
    }
    lines
}

fn format_disconnect(data: &Value, server: &str) -> Vec<String> {
    let disabled = data
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let creds_removed = data
        .get("credentials_removed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut lines = vec![format!("  Server: {server}")];
    if disabled {
        lines.push("  Status: disconnected".to_string());
    }
    if creds_removed {
        lines.push("  OAuth credentials: removed".to_string());
    }
    lines
}

fn format_error(i18n: &I18n, error: &str) -> String {
    match i18n.language() {
        Language::EnUs => format!("Error: {error}"),
        Language::ZhCn => format!("错误：{error}"),
    }
}
