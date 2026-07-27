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
            let output_text = run_cosh_core_mcp(&cosh_core.program, &["list"]);
            match output_text {
                Ok(json_str) => {
                    let body = format_mcp_list(&json_str, &i18n);
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
        "connect" | "inspect" | "refresh" | "disconnect" => {
            let name = arg.unwrap_or("");
            if name.is_empty() {
                return render_notice_panel(
                    output,
                    i18n.t(MessageId::SlashMcpTitle),
                    vec![format!("Usage: /mcp {action} <name>")],
                    None,
                );
            }
            let output_text = run_cosh_core_mcp(&cosh_core.program, &[action, name]);
            match output_text {
                Ok(json_str) => {
                    let body = format_mcp_inspection(&json_str, action, &i18n);
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
        "login" | "logout" => {
            let name = arg.unwrap_or("");
            if name.is_empty() {
                return render_notice_panel(
                    output,
                    i18n.t(MessageId::SlashMcpTitle),
                    vec![format!("Usage: /mcp {action} <name>")],
                    None,
                );
            }
            let output_text = run_cosh_core_mcp(&cosh_core.program, &[action, name]);
            match output_text {
                Ok(text) => {
                    let body = if text.trim().is_empty() {
                        vec![format!(
                            "  {} \"{name}\".",
                            tr(
                                &i18n,
                                &format!("{action} completed"),
                                &format!("{action} 完成")
                            )
                        )]
                    } else {
                        vec![format!("  {text}")]
                    };
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
        _ => render_notice_panel(output, i18n.t(MessageId::SlashMcpTitle), help(&i18n), None),
    }
}

fn run_cosh_core_mcp(program: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new(program);
    cmd.arg("mcp");
    for arg in args {
        cmd.arg(arg);
    }
    let result = cmd
        .output()
        .map_err(|e| format!("failed to run cosh-core: {e}"))?;
    if result.status.success() {
        Ok(String::from_utf8_lossy(&result.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let stdout = String::from_utf8_lossy(&result.stdout);
        let message = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("cosh-core mcp exited with status {}", result.status)
        };
        Err(message)
    }
}

fn format_mcp_list(json_str: &str, i18n: &I18n) -> Vec<String> {
    let parsed: Result<Value, _> = serde_json::from_str(json_str);
    let Ok(Value::Array(servers)) = parsed else {
        if json_str.trim().is_empty() {
            return vec![i18n.t(MessageId::SlashMcpEmptyBody).to_string()];
        }
        return vec![json_str.trim().to_string()];
    };
    if servers.is_empty() {
        return vec![i18n.t(MessageId::SlashMcpEmptyBody).to_string()];
    }
    servers
        .iter()
        .map(|server| {
            let name = server.get("server").and_then(|v| v.as_str()).unwrap_or("?");
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
                let creds = if has_creds { " [authenticated]" } else { "" };
                format!("  • {name} [{transport}]{creds}")
            } else {
                format!("  ○ {name} [{transport}] [disabled]")
            }
        })
        .collect()
}

fn format_mcp_inspection(json_str: &str, action: &str, i18n: &I18n) -> Vec<String> {
    let parsed: Result<Value, _> = serde_json::from_str(json_str);
    let Ok(data) = parsed else {
        return vec![json_str.trim().to_string()];
    };
    let mut lines = Vec::new();
    if let Some(server) = data.get("server").and_then(|v| v.as_str()) {
        lines.push(format!("  {}: {server}", tr(i18n, "Server", "服务器")));
    }
    if let Some(transport) = data.get("transport").and_then(|v| v.as_str()) {
        lines.push(format!("  {}: {transport}", tr(i18n, "Transport", "传输")));
    }
    if let Some(tools) = data.get("tools").and_then(|v| v.as_array()) {
        lines.push(format!("  {}: {}", tr(i18n, "Tools", "工具"), tools.len()));
        for tool in tools {
            let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let desc = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let short_desc = if desc.len() > 60 {
                format!("{}...", &desc[..57])
            } else {
                desc.to_string()
            };
            lines.push(format!("    - {name}: {short_desc}"));
        }
    }
    let action_label = tr(
        i18n,
        &format!("Action: {action}"),
        &format!("操作: {action}"),
    );
    lines.insert(0, format!("  {action_label}"));
    lines
}

fn help(i18n: &I18n) -> Vec<String> {
    let commands = [
        "list                                  — list configured MCP servers",
        "connect <name>                        — connect to an MCP server",
        "inspect <name>                        — view server tools",
        "refresh <name>                        — rediscover server tools",
        "disconnect <name>                     — disable an MCP server",
        "login <name>                          — authorize via OAuth",
        "logout <name>                         — remove OAuth credentials",
    ];
    let mut lines = vec![tr(i18n, "Available commands:", "可用命令：")];
    lines.extend(commands.into_iter().map(|cmd| format!("  {cmd}")));
    lines
}

fn tr(i18n: &I18n, en: &str, zh: &str) -> String {
    match i18n.language() {
        Language::EnUs => en.to_string(),
        Language::ZhCn => zh.to_string(),
    }
}
