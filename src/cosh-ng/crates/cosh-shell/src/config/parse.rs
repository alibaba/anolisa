use std::path::PathBuf;

use super::language::apply_language_value;
use super::readonly::{parse_disabled_rules, parse_runtime_spec, string_array};
use super::CoshConfig;

pub(super) fn parse_simple_config(content: &str, config: &mut CoshConfig) {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || !line.contains('=') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            match key {
                "shell.default" => config.shell_default = value.into(),
                "analysis.mode" => config.analysis_mode = value.into(),
                "approval.mode" => config.approval_mode = value.into(),
                "approval.trusted_command" => {
                    if !value.is_empty() {
                        config.trusted_commands.push(value.into());
                    }
                }
                "hooks.trusted_project_root" => {
                    if !value.is_empty() {
                        config.trusted_project_roots.push(PathBuf::from(value));
                    }
                }
                "adapter.default" => config.adapter_default = value.into(),
                "ui.language" => apply_language_value(config, value),
                "ui.startup_banner" => config.startup_banner = parse_bool_value(value),
                "ui.startup_hooks" => config.startup_hooks = parse_bool_value(value),
                "ui.debug" => config.debug = parse_bool_value(value),
                _ => {}
            }
        }
    }
}

pub(super) fn parse_toml_config(content: &str, config: &mut CoshConfig) {
    let value = match content.parse::<toml::Value>() {
        Ok(value) => value,
        Err(err) => {
            if content.contains("approval.readonly") || content.contains("readonly_disabled") {
                config
                    .readonly
                    .errors
                    .push(format!("invalid readonly config TOML: {err}"));
            }
            return;
        }
    };
    if let Some(ui) = value.get("ui").and_then(toml::Value::as_table) {
        if let Some(language) = ui.get("language").and_then(toml::Value::as_str) {
            apply_language_value(config, language);
        }
        if let Some(startup_banner) = ui.get("startup_banner").and_then(toml::Value::as_bool) {
            config.startup_banner = startup_banner;
        }
        if let Some(startup_hooks) = ui.get("startup_hooks").and_then(toml::Value::as_bool) {
            config.startup_hooks = startup_hooks;
        }
        if let Some(debug) = ui.get("debug").and_then(toml::Value::as_bool) {
            config.debug = debug;
        }
    }
    if let Some(hooks) = value.get("hooks").and_then(toml::Value::as_table) {
        if let Some(roots) = hooks.get("trusted_project_roots") {
            match string_array(roots, "hooks.trusted_project_roots") {
                Ok(roots) => config
                    .trusted_project_roots
                    .extend(roots.into_iter().map(PathBuf::from)),
                Err(err) => config.readonly.errors.push(err),
            }
        }
    }
    if let Some(adapter) = value.get("adapter").and_then(toml::Value::as_table) {
        if let Some(default) = adapter.get("default").and_then(toml::Value::as_str) {
            config.adapter_default = default.to_string();
        }
    }

    let Some(approval) = value.get("approval").and_then(toml::Value::as_table) else {
        return;
    };

    if let Some(disabled) = approval.get("readonly_disabled") {
        match parse_disabled_rules(disabled) {
            Ok(keys) => config.readonly.disabled.extend(keys),
            Err(err) => config.readonly.errors.push(err),
        }
    }

    let Some(readonly) = approval.get("readonly").and_then(toml::Value::as_table) else {
        return;
    };
    for (command, spec_value) in readonly {
        match parse_runtime_spec(command, spec_value) {
            Ok(Some(spec)) => config.readonly.overrides.push(spec),
            Ok(None) => {}
            Err(err) => config.readonly.errors.push(err),
        }
    }
}

pub(super) fn parse_bool_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}
