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
                "shell.analysis_mode" => config.analysis_mode = value.into(),
                "shell.approval_mode" => config.approval_mode = value.into(),
                "shell.adapter_default" => config.adapter_default = value.into(),
                "shell.trusted_command" => {
                    if !value.is_empty() {
                        config.trusted_commands.push(value.into());
                    }
                }
                "shell.trusted_project_root" => {
                    if !value.is_empty() {
                        config.trusted_project_roots.push(PathBuf::from(value));
                    }
                }
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
            if content.contains("shell.readonly") || content.contains("readonly_disabled") {
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
    parse_shell_toml_config(&value, config);
}

fn parse_shell_toml_config(value: &toml::Value, config: &mut CoshConfig) {
    let Some(shell) = value.get("shell").and_then(toml::Value::as_table) else {
        return;
    };

    if let Some(default) = shell.get("default").and_then(toml::Value::as_str) {
        config.shell_default = default.to_string();
    }
    if let Some(analysis_mode) = shell.get("analysis_mode").and_then(toml::Value::as_str) {
        config.analysis_mode = analysis_mode.to_string();
    }
    if let Some(approval_mode) = shell.get("approval_mode").and_then(toml::Value::as_str) {
        config.approval_mode = approval_mode.to_string();
    }
    if let Some(adapter_default) = shell.get("adapter_default").and_then(toml::Value::as_str) {
        config.adapter_default = adapter_default.to_string();
    }
    if let Some(commands) = shell.get("trusted_commands") {
        match string_array(commands, "shell.trusted_commands") {
            Ok(commands) => config
                .trusted_commands
                .extend(commands.into_iter().filter(|command| !command.is_empty())),
            Err(err) => config.readonly.errors.push(err),
        }
    }
    parse_trusted_project_roots(config, shell, "shell.trusted_project_roots");
    if let Some(disabled) = shell.get("readonly_disabled") {
        match parse_disabled_rules(disabled, "shell.readonly_disabled") {
            Ok(keys) => config.readonly.disabled.extend(keys),
            Err(err) => config.readonly.errors.push(err),
        }
    }

    parse_readonly_table(
        config,
        shell.get("readonly").and_then(toml::Value::as_table),
        "shell.readonly",
    );
}

fn parse_trusted_project_roots(
    config: &mut CoshConfig,
    table: &toml::map::Map<String, toml::Value>,
    label: &str,
) {
    let Some(roots) = table.get("trusted_project_roots") else {
        return;
    };
    match string_array(roots, label) {
        Ok(roots) => config
            .trusted_project_roots
            .extend(roots.into_iter().map(PathBuf::from)),
        Err(err) => config.readonly.errors.push(err),
    }
}

fn parse_readonly_table(
    config: &mut CoshConfig,
    readonly: Option<&toml::map::Map<String, toml::Value>>,
    label: &str,
) {
    let Some(readonly) = readonly else {
        return;
    };
    for (command, spec_value) in readonly {
        match parse_runtime_spec(command, spec_value, label) {
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
