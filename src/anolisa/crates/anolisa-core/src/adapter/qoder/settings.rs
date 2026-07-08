//! Qoder `settings.json` merge, prune, and verification helpers.

use std::collections::HashSet;
use std::path::Path;

use serde_json::{Map, Value};

use crate::adapter::AdapterError;
use crate::adapter::driver::DriverCtx;

use super::{HOOKS_PLACEHOLDER, QODER_HOOKS_FILE, common_hooks_dir, hook_prefix, plugin_entry};

/// Outcome of reading and inspecting `settings.json` for `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsProbe {
    /// The file exists and parses; whether our two managed entries are set.
    Present {
        hooks_present: bool,
        plugin_enabled: bool,
    },
    /// The file does not exist.
    Absent,
    /// The file cannot be read or parsed, so absence must not be inferred.
    Unverifiable,
}

/// Read `settings` through controlled ops and report managed-entry presence.
pub(super) fn probe_settings(
    ctx: &DriverCtx,
    settings: &Path,
    plugin: &str,
    expected_hooks: &[String],
) -> SettingsProbe {
    match ctx.ops.read_file(settings) {
        Ok(None) => SettingsProbe::Absent,
        Ok(Some(bytes)) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(root)) => {
                let (hooks_present, plugin_enabled) = settings_managed_present(
                    &root,
                    expected_hooks,
                    &hook_prefix(plugin),
                    &plugin_entry(plugin),
                );
                SettingsProbe::Present {
                    hooks_present,
                    plugin_enabled,
                }
            }
            Ok(_) => SettingsProbe::Present {
                hooks_present: false,
                plugin_enabled: false,
            },
            Err(_) => SettingsProbe::Unverifiable,
        },
        Err(_) => SettingsProbe::Unverifiable,
    }
}

/// Prune managed entries from `settings.json` through controlled ops.
pub(super) fn prune_settings_via_ops(
    ctx: &DriverCtx,
    settings: &Path,
    plugin: &str,
    messages: &mut Vec<String>,
) -> bool {
    let bytes = match ctx.ops.read_file(settings) {
        Ok(None) => {
            messages.push("~/.qoder/settings.json absent; nothing to prune".to_string());
            return true;
        }
        Ok(Some(bytes)) => bytes,
        Err(err) => {
            messages.push(format!("failed to read settings.json: {err}"));
            return false;
        }
    };
    let mut root = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(root)) => root,
        Ok(_) => {
            messages.push("settings.json is not a JSON object; left untouched".to_string());
            return true;
        }
        Err(err) => {
            messages.push(format!("settings.json unparseable; left untouched: {err}"));
            return false;
        }
    };
    if !prune_managed(&mut root, &hook_prefix(plugin), &plugin_entry(plugin)) {
        messages.push("settings.json already free of tokenless entries".to_string());
        return true;
    }
    let out = match serde_json::to_vec_pretty(&Value::Object(root)) {
        Ok(out) => out,
        Err(err) => {
            messages.push(format!("failed to render pruned settings JSON: {err}"));
            return false;
        }
    };
    match ctx.ops.write_file(settings, &out) {
        Ok(()) => {
            messages.push("pruned tokenless entries from settings.json".to_string());
            true
        }
        Err(err) => {
            messages.push(format!("failed to write settings.json: {err}"));
            false
        }
    }
}

/// Parse settings bytes for a merge.
pub(super) fn load_settings_for_merge(
    existing: Option<Vec<u8>>,
    path: &Path,
) -> Result<Map<String, Value>, AdapterError> {
    match existing {
        None => Ok(Map::new()),
        Some(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(root)) => Ok(root),
            Ok(_) => Ok(Map::new()),
            Err(source) => Err(AdapterError::SettingsUnparseable {
                path: path.to_path_buf(),
                reason: source.to_string(),
            }),
        },
    }
}

/// Read `hooks.json`, expand hook script placeholders, and parse it.
pub(super) fn load_resolved_hooks(resource_root: &Path) -> Result<Value, AdapterError> {
    let path = resource_root.join(QODER_HOOKS_FILE);
    let bytes = std::fs::read(&path).map_err(|source| AdapterError::Io {
        path: path.clone(),
        source,
    })?;
    let hooks_dir = common_hooks_dir(resource_root);
    let text = String::from_utf8_lossy(&bytes);
    let substituted = text.replace(HOOKS_PLACEHOLDER, &hooks_dir.to_string_lossy());
    serde_json::from_str(&substituted).map_err(|source| AdapterError::BundleInvalid {
        root: resource_root.to_path_buf(),
        reason: format!("failed to parse {QODER_HOOKS_FILE}: {source}"),
    })
}

/// Every hook name declared in the bundle's `hooks.json`.
pub(super) fn collect_expected_hook_names(
    resource_root: &Path,
) -> Result<Vec<String>, AdapterError> {
    let resolved = load_resolved_hooks(resource_root)?;
    let mut names = Vec::new();
    if let Some(hooks) = resolved.get("hooks").and_then(Value::as_object) {
        for entries in hooks.values() {
            if let Some(arr) = entries.as_array() {
                for entry in arr {
                    if let Some(hs) = entry.get("hooks").and_then(Value::as_array) {
                        for h in hs {
                            if let Some(name) = h.get("name").and_then(Value::as_str) {
                                names.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(names)
}

/// Add managed hooks and `<plugin>@local` to a settings object.
pub(super) fn merge_managed(root: &mut Map<String, Value>, resolved: &Value, plugin_entry: &str) {
    if let Some(resolved_hooks) = resolved.get("hooks").and_then(Value::as_object) {
        let hooks_slot = root
            .entry("hooks")
            .or_insert_with(|| Value::Object(Map::new()));
        let hooks = ensure_object(hooks_slot);
        for (event, entries) in resolved_hooks {
            let Some(entries) = entries.as_array() else {
                continue;
            };
            let slot = hooks
                .entry(event.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            let arr = ensure_array(slot);
            let existing_names = collect_hook_names(arr);
            for entry in entries {
                if let Some(name) = primary_hook_name(entry)
                    && !existing_names.contains(&name)
                {
                    arr.push(entry.clone());
                }
            }
        }
    }

    let plugins_slot = root
        .entry("plugins")
        .or_insert_with(|| Value::Object(Map::new()));
    let plugins = ensure_object(plugins_slot);
    let enabled_slot = plugins
        .entry("enabled")
        .or_insert_with(|| Value::Array(Vec::new()));
    let enabled = ensure_array(enabled_slot);
    if !enabled.iter().any(|v| v.as_str() == Some(plugin_entry)) {
        enabled.push(Value::String(plugin_entry.to_string()));
    }
}

fn prune_managed(root: &mut Map<String, Value>, hook_prefix: &str, plugin_entry: &str) -> bool {
    let mut removed = false;

    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            if let Some(arr) = hooks.get_mut(&event).and_then(Value::as_array_mut) {
                let before = arr.len();
                arr.retain(|entry| !entry_is_owned(entry, hook_prefix));
                if arr.len() != before {
                    removed = true;
                }
                if arr.is_empty() {
                    hooks.remove(&event);
                    removed = true;
                }
            }
        }
        if hooks.is_empty() {
            root.remove("hooks");
        }
    }

    if let Some(plugins) = root.get_mut("plugins").and_then(Value::as_object_mut) {
        if let Some(enabled) = plugins.get_mut("enabled").and_then(Value::as_array_mut) {
            let before = enabled.len();
            enabled.retain(|v| v.as_str() != Some(plugin_entry));
            if enabled.len() != before {
                removed = true;
            }
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

fn settings_managed_present(
    root: &Map<String, Value>,
    expected_hooks: &[String],
    hook_prefix: &str,
    plugin_entry: &str,
) -> (bool, bool) {
    let present = present_hook_names(root);
    let hooks_present = if expected_hooks.is_empty() {
        present.iter().any(|n| n.starts_with(hook_prefix))
    } else {
        expected_hooks.iter().all(|n| present.contains(n))
    };
    let plugin_enabled = root
        .get("plugins")
        .and_then(Value::as_object)
        .and_then(|p| p.get("enabled"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().any(|v| v.as_str() == Some(plugin_entry)))
        .unwrap_or(false);
    (hooks_present, plugin_enabled)
}

fn present_hook_names(root: &Map<String, Value>) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(hooks) = root.get("hooks").and_then(Value::as_object) {
        for entries in hooks.values() {
            if let Some(arr) = entries.as_array() {
                names.extend(collect_hook_names(arr));
            }
        }
    }
    names
}

fn entry_is_owned(entry: &Value, prefix: &str) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("name")
                    .and_then(Value::as_str)
                    .map(|n| n.starts_with(prefix))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn primary_hook_name(entry: &Value) -> Option<String> {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .and_then(|hooks| {
            hooks
                .iter()
                .find_map(|h| h.get("name").and_then(Value::as_str).map(str::to_string))
        })
}

fn collect_hook_names(entries: &[Value]) -> HashSet<String> {
    let mut names = HashSet::new();
    for entry in entries {
        if let Some(hooks) = entry.get("hooks").and_then(Value::as_array) {
            for h in hooks {
                if let Some(name) = h.get("name").and_then(Value::as_str) {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names
}

fn ensure_object(v: &mut Value) -> &mut Map<String, Value> {
    if !v.is_object() {
        *v = Value::Object(Map::new());
    }
    match v.as_object_mut() {
        Some(m) => m,
        None => unreachable!("value coerced to object cannot fail as_object_mut"),
    }
}

fn ensure_array(v: &mut Value) -> &mut Vec<Value> {
    if !v.is_array() {
        *v = Value::Array(Vec::new());
    }
    match v.as_array_mut() {
        Some(a) => a,
        None => unreachable!("value coerced to array cannot fail as_array_mut"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> Value {
        serde_json::from_str(s).expect("valid JSON")
    }

    fn obj(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            other => panic!("expected object, got {other}"),
        }
    }

    fn resolved_hooks() -> Value {
        json(
            r#"{
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "tokenless-rewrite" } ] }
                ],
                "PostToolUse": [
                  { "hooks": [ { "type": "command", "name": "tokenless-compress" } ] }
                ]
              }
            }"#,
        )
    }

    #[test]
    fn merge_into_empty_adds_hooks_and_plugin() {
        let mut root = Map::new();
        merge_managed(&mut root, &resolved_hooks(), "tokenless@local");
        let v = Value::Object(root);
        assert_eq!(
            v["hooks"]["PreToolUse"][0]["hooks"][0]["name"],
            "tokenless-rewrite"
        );
        assert_eq!(v["plugins"]["enabled"][0], "tokenless@local");
    }

    #[test]
    fn merge_preserves_user_hooks_and_config() {
        let mut root = obj(json(
            r#"{
              "theme": "dark",
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "user-audit" } ] }
                ]
              },
              "plugins": { "enabled": ["other@local"], "registry": "corp" }
            }"#,
        ));
        merge_managed(&mut root, &resolved_hooks(), "tokenless@local");
        let v = Value::Object(root);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["plugins"]["registry"], "corp");
        let pre = v["hooks"]["PreToolUse"].as_array().expect("array");
        let names: Vec<&str> = pre
            .iter()
            .filter_map(|e| e["hooks"][0]["name"].as_str())
            .collect();
        assert!(names.contains(&"user-audit"));
        assert!(names.contains(&"tokenless-rewrite"));
        let enabled = v["plugins"]["enabled"].as_array().expect("array");
        assert!(enabled.iter().any(|x| x == "other@local"));
        assert!(enabled.iter().any(|x| x == "tokenless@local"));
    }

    #[test]
    fn merge_is_idempotent() {
        let mut root = Map::new();
        merge_managed(&mut root, &resolved_hooks(), "tokenless@local");
        merge_managed(&mut root, &resolved_hooks(), "tokenless@local");
        let v = Value::Object(root);
        assert_eq!(
            v["hooks"]["PreToolUse"].as_array().expect("array").len(),
            1,
            "re-merge must not duplicate a hook by name"
        );
        assert_eq!(
            v["plugins"]["enabled"].as_array().expect("array").len(),
            1,
            "re-merge must not duplicate the plugin entry"
        );
    }

    #[test]
    fn prune_removes_only_managed_entries() {
        let mut root = obj(json(
            r#"{
              "theme": "dark",
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "user-audit" } ] },
                  { "hooks": [ { "type": "command", "name": "tokenless-rewrite" } ] }
                ],
                "PostToolUse": [
                  { "hooks": [ { "type": "command", "name": "tokenless-compress" } ] }
                ]
              },
              "plugins": { "enabled": ["other@local", "tokenless@local"] }
            }"#,
        ));
        let changed = prune_managed(&mut root, "tokenless-", "tokenless@local");
        assert!(changed);
        let v = Value::Object(root);
        assert_eq!(v["theme"], "dark");
        let pre = v["hooks"]["PreToolUse"].as_array().expect("array");
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["hooks"][0]["name"], "user-audit");
        assert!(v["hooks"].get("PostToolUse").is_none());
        let enabled = v["plugins"]["enabled"].as_array().expect("array");
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0], "other@local");
    }

    #[test]
    fn prune_does_not_match_user_hook_by_command_substring() {
        let mut root = obj(json(
            r#"{
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "my-hook",
                                 "command": "run tokenless stats" } ] }
                ]
              }
            }"#,
        ));
        let changed = prune_managed(&mut root, "tokenless-", "tokenless@local");
        assert!(!changed, "no managed entries present");
        let v = Value::Object(root);
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["name"], "my-hook");
    }

    #[test]
    fn prune_collapses_emptied_containers_and_is_idempotent() {
        let mut root = obj(json(
            r#"{
              "hooks": {
                "PreToolUse": [
                  { "hooks": [ { "type": "command", "name": "tokenless-rewrite" } ] }
                ]
              },
              "plugins": { "enabled": ["tokenless@local"] }
            }"#,
        ));
        assert!(prune_managed(&mut root, "tokenless-", "tokenless@local"));
        assert!(root.get("hooks").is_none());
        assert!(root.get("plugins").is_none());
        assert!(!prune_managed(&mut root, "tokenless-", "tokenless@local"));
    }

    #[test]
    fn settings_managed_present_detects_both_signals() {
        let expected = vec![
            "tokenless-rewrite".to_string(),
            "tokenless-compress-response".to_string(),
        ];
        let root = obj(json(
            r#"{
              "hooks": {
                "PreToolUse": [ { "hooks": [ { "name": "tokenless-rewrite" } ] } ],
                "PostToolUse": [ { "hooks": [ { "name": "tokenless-compress-response" } ] } ]
              },
              "plugins": { "enabled": ["tokenless@local"] }
            }"#,
        ));
        assert_eq!(
            settings_managed_present(&root, &expected, "tokenless-", "tokenless@local"),
            (true, true)
        );

        let partial = obj(json(r#"{ "plugins": { "enabled": ["tokenless@local"] } }"#));
        assert_eq!(
            settings_managed_present(&partial, &expected, "tokenless-", "tokenless@local"),
            (false, true)
        );

        let none = obj(json(r#"{ "theme": "dark" }"#));
        assert_eq!(
            settings_managed_present(&none, &expected, "tokenless-", "tokenless@local"),
            (false, false)
        );
    }

    #[test]
    fn settings_managed_present_requires_all_expected_hooks() {
        let expected = vec![
            "tokenless-rewrite".to_string(),
            "tokenless-compress-response".to_string(),
        ];
        let root = obj(json(
            r#"{
              "hooks": { "PreToolUse": [
                { "hooks": [ { "name": "tokenless-rewrite" } ] } ] },
              "plugins": { "enabled": ["tokenless@local"] }
            }"#,
        ));
        assert_eq!(
            settings_managed_present(&root, &expected, "tokenless-", "tokenless@local"),
            (false, true),
            "a missing managed hook must not read as present"
        );
    }

    #[test]
    fn settings_managed_present_falls_back_to_prefix_when_no_expected() {
        let root = obj(json(
            r#"{ "hooks": { "PreToolUse": [
                { "hooks": [ { "name": "tokenless-rewrite" } ] } ] } }"#,
        ));
        let (hooks_present, _) =
            settings_managed_present(&root, &[], "tokenless-", "tokenless@local");
        assert!(hooks_present);
    }

    #[test]
    fn collect_expected_hook_names_reads_all_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("adapters").join("tokenless").join("qoder");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join(QODER_HOOKS_FILE),
            br#"{"hooks":{
              "PreToolUse":[{"hooks":[{"name":"tokenless-rewrite"}]}],
              "PostToolUse":[{"hooks":[{"name":"tokenless-compress-response"}]}]
            }}"#,
        )
        .expect("write hooks");
        let mut names = collect_expected_hook_names(&root).expect("collect");
        names.sort();
        assert_eq!(
            names,
            vec!["tokenless-compress-response", "tokenless-rewrite"]
        );
    }

    #[test]
    fn load_settings_for_merge_fails_closed_on_malformed_json() {
        let path = Path::new("/home/u/.qoder/settings.json");
        assert!(
            load_settings_for_merge(None, path)
                .expect("absent ok")
                .is_empty()
        );
        assert!(
            load_settings_for_merge(Some(b"[1,2,3]".to_vec()), path)
                .expect("array ok")
                .is_empty()
        );
        let err = load_settings_for_merge(Some(b"{not json".to_vec()), path)
            .expect_err("malformed must fail closed");
        assert!(matches!(err, AdapterError::SettingsUnparseable { .. }));
    }

    #[test]
    fn load_resolved_hooks_substitutes_placeholder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("adapters").join("tokenless").join("qoder");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join(QODER_HOOKS_FILE),
            br#"{"hooks":{"PreToolUse":[{"hooks":[{"name":"tokenless-rewrite","command":"bash ${QODER_TOKENLESS_HOOKS}/rewrite.sh"}]}]}}"#,
        )
        .expect("write hooks");
        let resolved = load_resolved_hooks(&root).expect("resolve");
        let cmd = resolved["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        assert!(
            cmd.contains("adapters/tokenless/common/hooks/rewrite.sh"),
            "{cmd}"
        );
        assert!(!cmd.contains(HOOKS_PLACEHOLDER));
    }
}
