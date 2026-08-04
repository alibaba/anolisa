//! Exact cleanup helpers for settings entries written by legacy Qoder adapters.

use std::path::Path;

use serde_json::{Map, Value};

use crate::adapter::claim::QoderManagedHook;
use crate::adapter::driver::DriverCtx;

/// Rebuild exact hook entries written before receipts stored full hook specs.
pub(super) fn reconstruct_legacy_hook_specs(
    resource_root: &Path,
    managed_hook_names: &[String],
) -> Result<Vec<QoderManagedHook>, String> {
    if managed_hook_names.is_empty() {
        return Err("legacy Qoder receipt has no managed hook names or exact specs".to_string());
    }

    let hooks_root = resource_root
        .parent()
        .unwrap_or(resource_root)
        .join("common/hooks");
    let command = |interpreter: &str, script: &str| {
        format!("{interpreter} {}/{}", hooks_root.to_string_lossy(), script)
    };
    let agent_id_variant = |spec: &QoderManagedHook| -> Result<QoderManagedHook, String> {
        let mut variant = spec.clone();
        let command = variant
            .entry
            .pointer("/hooks/0/command")
            .and_then(Value::as_str)
            .ok_or_else(|| "generated legacy Qoder hook has no command".to_string())?
            .to_string();
        let slot = variant
            .entry
            .pointer_mut("/hooks/0/command")
            .ok_or_else(|| "generated legacy Qoder hook has no command".to_string())?;
        *slot = Value::String(format!("{command} --agent-id qoder-cli"));
        Ok(variant)
    };

    let mut specs = Vec::new();
    for name in managed_hook_names {
        let spec = match name.as_str() {
            "tokenless-tool-ready" => QoderManagedHook {
                event: "PreToolUse".to_string(),
                entry: serde_json::json!({
                    "matcher": "",
                    "sequential": true,
                    "hooks": [{
                        "type": "command",
                        "name": "tokenless-tool-ready",
                        "description": "Pre-checks tool environment readiness, auto-fixes, and provides skip-retry guidance",
                        "command": command("bash", "tool_ready_hook.sh"),
                        "timeout": 10000,
                        "env": {"TOKENLESS_AGENT_ID": "qoder-cli"}
                    }]
                }),
            },
            "tokenless-rewrite" => QoderManagedHook {
                event: "PreToolUse".to_string(),
                entry: serde_json::json!({
                    "matcher": "^(Bash|Shell|run_shell_command|terminal|execute_command)$",
                    "hooks": [{
                        "type": "command",
                        "name": "tokenless-rewrite",
                        "description": "Rewrites shell commands via rtk for token savings",
                        "command": command("python3", "rewrite_hook.py"),
                        "timeout": 5000,
                        "env": {"TOKENLESS_AGENT_ID": "qoder-cli"}
                    }]
                }),
            },
            "tokenless-compress-response" => QoderManagedHook {
                event: "PostToolUse".to_string(),
                entry: serde_json::json!({
                    "matcher": "",
                    "hooks": [{
                        "type": "command",
                        "name": "tokenless-compress-response",
                        "description": "Compresses tool responses and encodes to TOON format",
                        "command": command("python3", "compress_response_hook.py"),
                        "timeout": 10000,
                        "env": {"TOKENLESS_AGENT_ID": "qoder-cli"}
                    }]
                }),
            },
            other => {
                return Err(format!(
                    "legacy Qoder receipt contains unknown managed hook '{other}'"
                ));
            }
        };
        specs.push(spec.clone());
        if matches!(
            name.as_str(),
            "tokenless-rewrite" | "tokenless-compress-response"
        ) {
            specs.push(agent_id_variant(&spec)?);
        }
    }
    Ok(specs)
}

/// Prune receipt-owned legacy entries through the Manager's symlink-safe,
/// atomic filesystem operations.
pub(super) fn prune_settings_via_ops(
    ctx: &DriverCtx,
    settings: &Path,
    plugin_entry: &str,
    managed_hooks: &[QoderManagedHook],
    messages: &mut Vec<String>,
) -> bool {
    let bytes = match ctx.ops.read_file(settings) {
        Ok(None) => {
            messages.push("legacy ~/.qoder/settings.json absent; nothing to migrate".to_string());
            return true;
        }
        Ok(Some(bytes)) => bytes,
        Err(err) => {
            messages.push(format!("failed to safely read legacy settings.json: {err}"));
            return false;
        }
    };
    let mut root = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(root)) => root,
        Ok(_) => {
            messages.push("legacy settings.json root is not an object; left untouched".to_string());
            return false;
        }
        Err(err) => {
            messages.push(format!(
                "legacy settings.json is invalid JSON; left untouched: {err}"
            ));
            return false;
        }
    };
    if let Err(reason) = validate_legacy_shape(&root) {
        messages.push(format!(
            "legacy settings.json has an unsafe shape; left untouched: {reason}"
        ));
        return false;
    }
    if !prune_managed(&mut root, managed_hooks, plugin_entry) {
        messages.push("legacy settings already free of receipt-owned entries".to_string());
        return true;
    }
    let out = match serde_json::to_vec_pretty(&Value::Object(root)) {
        Ok(mut out) => {
            out.push(b'\n');
            out
        }
        Err(err) => {
            messages.push(format!("failed to render migrated settings JSON: {err}"));
            return false;
        }
    };
    match ctx.ops.write_file(settings, &out) {
        Ok(()) => {
            messages.push("migrated receipt-owned legacy Qoder settings entries".to_string());
            true
        }
        Err(err) => {
            messages.push(format!("failed to atomically migrate settings.json: {err}"));
            false
        }
    }
}

fn validate_legacy_shape(root: &Map<String, Value>) -> Result<(), String> {
    if let Some(hooks) = root.get("hooks") {
        let hooks = hooks
            .as_object()
            .ok_or_else(|| "'hooks' is not an object".to_string())?;
        for (event, entries) in hooks {
            if !entries.is_array() {
                return Err(format!("'hooks.{event}' is not an array"));
            }
        }
    }
    if let Some(plugins) = root.get("plugins") {
        let plugins = plugins
            .as_object()
            .ok_or_else(|| "'plugins' is not an object".to_string())?;
        if plugins
            .get("enabled")
            .is_some_and(|enabled| !enabled.is_array())
        {
            return Err("'plugins.enabled' is not an array".to_string());
        }
    }
    Ok(())
}

fn prune_managed(
    root: &mut Map<String, Value>,
    managed_hooks: &[QoderManagedHook],
    plugin_entry: &str,
) -> bool {
    let mut removed = false;

    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            let Some(entries) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
                continue;
            };
            let before = entries.len();
            entries.retain(|entry| {
                !managed_hooks
                    .iter()
                    .any(|managed| managed.event == event && &managed.entry == entry)
            });
            removed |= entries.len() != before;
            if entries.is_empty() {
                hooks.remove(&event);
            }
        }
        if hooks.is_empty() {
            root.remove("hooks");
        }
    }

    if let Some(plugins) = root.get_mut("plugins").and_then(Value::as_object_mut) {
        if let Some(enabled) = plugins.get_mut("enabled").and_then(Value::as_array_mut) {
            let before = enabled.len();
            enabled.retain(|value| value.as_str() != Some(plugin_entry));
            removed |= enabled.len() != before;
            if enabled.is_empty() {
                plugins.remove("enabled");
            }
        }
        if plugins.is_empty() {
            root.remove("plugins");
        }
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn managed() -> Vec<QoderManagedHook> {
        vec![QoderManagedHook {
            event: "PreToolUse".to_string(),
            entry: serde_json::json!({
                "matcher": "Bash",
                "hooks": [{"name": "tokenless-rewrite", "command": "python3 /owned/rewrite.py"}]
            }),
        }]
    }

    #[test]
    fn cleanup_is_exact_and_preserves_official_activation() {
        let mut root = serde_json::json!({
            "theme": "dark",
            "enabledPlugins": {"tokenless@local": true},
            "hooks": {"PreToolUse": [
                {"matcher": "Bash", "hooks": [{"name": "tokenless-custom", "command": "audit"}]},
                {"matcher": "Bash", "hooks": [{"name": "tokenless-rewrite", "command": "python3 /owned/rewrite.py"}]}
            ]},
            "plugins": {"enabled": ["other@local", "tokenless@local"]}
        })
        .as_object()
        .expect("object")
        .clone();

        assert!(prune_managed(&mut root, &managed(), "tokenless@local"));
        let value = Value::Object(root);
        assert_eq!(value["theme"], "dark");
        assert_eq!(value["enabledPlugins"]["tokenless@local"], true);
        assert_eq!(value["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["plugins"]["enabled"],
            serde_json::json!(["other@local"])
        );
    }

    #[test]
    fn cleanup_is_idempotent() {
        let mut root = Map::new();
        assert!(!prune_managed(&mut root, &managed(), "tokenless@local"));
    }

    #[test]
    fn reconstructs_earliest_receipt_hook_fingerprint() {
        let specs = reconstruct_legacy_hook_specs(
            Path::new("/opt/anolisa/adapters/tokenless/qoder"),
            &["tokenless-rewrite".to_string()],
        )
        .expect("known legacy hook");
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].event, "PreToolUse");
        assert_eq!(
            specs[0].entry["hooks"][0]["command"],
            "python3 /opt/anolisa/adapters/tokenless/common/hooks/rewrite_hook.py"
        );
        assert_eq!(
            specs[1].entry["hooks"][0]["command"],
            "python3 /opt/anolisa/adapters/tokenless/common/hooks/rewrite_hook.py --agent-id qoder-cli"
        );
        assert!(
            reconstruct_legacy_hook_specs(
                Path::new("/opt/anolisa/adapters/tokenless/qoder"),
                &["tokenless-custom".to_string()],
            )
            .is_err()
        );
    }

    #[test]
    fn unsafe_nested_shapes_are_rejected() {
        for value in [
            serde_json::json!({"hooks": "disabled"}),
            serde_json::json!({"hooks": {"PreToolUse": "disabled"}}),
            serde_json::json!({"plugins": {"enabled": "tokenless@local"}}),
        ] {
            assert!(validate_legacy_shape(value.as_object().unwrap()).is_err());
        }
    }
}
