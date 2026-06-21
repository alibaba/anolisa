use serde_json::Value;

use crate::runtime::prelude::*;
use crate::slash::panel::render_notice_panel;

pub(super) fn render_extensions_command<W: Write>(
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
            i18n.t(MessageId::SlashExtensionsTitle),
            vec![i18n.t(MessageId::SlashRegistryUnavailable).to_string()],
            None,
        );
    };

    let action = sub.unwrap_or("list");
    let i18n = state.i18n();

    match action {
        "list" => {
            let params = Value::Null;
            match cosh_core.registry_query("extensions", "list", params) {
                Ok(data) => {
                    let body = format_extensions_list(&data, &i18n);
                    render_notice_panel(
                        output,
                        i18n.t(MessageId::SlashExtensionsTitle),
                        body,
                        None,
                    )
                }
                Err(e) => render_notice_panel(
                    output,
                    i18n.t(MessageId::SlashExtensionsTitle),
                    vec![format!("Error: {e}")],
                    None,
                ),
            }
        }
        "detail" => {
            let name = arg.unwrap_or("");
            let params = serde_json::json!({ "name": name });
            match cosh_core.registry_query("extensions", "detail", params) {
                Ok(data) => {
                    let body = format_extension_detail(&data);
                    render_notice_panel(
                        output,
                        i18n.t(MessageId::SlashExtensionsTitle),
                        body,
                        None,
                    )
                }
                Err(e) => render_notice_panel(
                    output,
                    i18n.t(MessageId::SlashExtensionsTitle),
                    vec![format!("Error: {e}")],
                    None,
                ),
            }
        }
        _ => render_notice_panel(
            output,
            i18n.t(MessageId::SlashExtensionsTitle),
            vec![format!("Unknown subcommand: {action}")],
            None,
        ),
    }
}

fn format_extensions_list(data: &Value, i18n: &I18n) -> Vec<String> {
    let Some(arr) = data.as_array() else {
        return vec![i18n.t(MessageId::SlashExtensionsEmptyBody).to_string()];
    };
    if arr.is_empty() {
        return vec![i18n.t(MessageId::SlashExtensionsEmptyBody).to_string()];
    }
    arr.iter()
        .filter_map(|ext| {
            let name = ext.get("name")?.as_str()?;
            let version = ext.get("version").and_then(|v| v.as_str()).unwrap_or("?");
            let active = ext.get("is_active").and_then(|v| v.as_bool()).unwrap_or(false);
            let status = if active { "active" } else { "inactive" };
            Some(format!("  {name} v{version} ({status})"))
        })
        .collect()
}

fn format_extension_detail(data: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
        lines.push(format!("  Name: {name}"));
    }
    if let Some(version) = data.get("version").and_then(|v| v.as_str()) {
        lines.push(format!("  Version: {version}"));
    }
    if let Some(active) = data.get("is_active").and_then(|v| v.as_bool()) {
        lines.push(format!("  Active: {active}"));
    }
    if let Some(path) = data.get("path").and_then(|v| v.as_str()) {
        lines.push(format!("  Path: {path}"));
    }
    if let Some(has_hooks) = data.get("has_hooks").and_then(|v| v.as_bool()) {
        lines.push(format!("  Has Hooks: {has_hooks}"));
    }
    if let Some(skill_dirs) = data.get("skill_dirs").and_then(|v| v.as_array()) {
        let dirs: Vec<&str> = skill_dirs.iter().filter_map(|v| v.as_str()).collect();
        lines.push(format!("  Skill Dirs: {}", dirs.join(", ")));
    }
    lines
}
