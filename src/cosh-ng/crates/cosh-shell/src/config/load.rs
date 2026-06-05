use std::path::{Path, PathBuf};

use super::language::apply_language_value;
use super::parse::{parse_bool_value, parse_simple_config, parse_toml_config};
use super::trust::{load_project_trust_store, project_trust_store_path};
use super::CoshConfig;

pub fn load_config() -> CoshConfig {
    let mut config = CoshConfig::default();

    if let Some(path) = config_file_path() {
        load_config_file_into(&path, &mut config);
    }
    if let Some(path) = project_trust_store_path() {
        load_project_trust_store(&mut config, &path);
    }

    apply_env_overrides(&mut config);
    config
}

pub(super) fn load_config_file_into(path: &Path, config: &mut CoshConfig) {
    if let Ok(content) = std::fs::read_to_string(path) {
        parse_simple_config(&content, config);
        parse_toml_config(&content, config);
    }
}

pub(super) fn config_file_path() -> Option<PathBuf> {
    dirs_next_or_home().map(|d| d.join(".config/cosh/config.toml"))
}

pub(super) fn dirs_next_or_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn apply_env_overrides(config: &mut CoshConfig) {
    if let Ok(v) = std::env::var("COSH_SHELL_ANALYSIS_MODE") {
        config.analysis_mode = v;
    }
    if let Ok(v) = std::env::var("COSH_SHELL_APPROVAL_MODE") {
        config.approval_mode = v;
    }
    if let Ok(v) = std::env::var("COSH_SHELL_DEFAULT_SHELL") {
        config.shell_default = v;
    }
    if let Ok(v) = std::env::var("COSH_SHELL_ADAPTER") {
        config.adapter_default = v;
    } else if let Ok(v) = std::env::var("COSH_SHELL_ADAPTER_DEFAULT") {
        config.adapter_default = v;
    }
    if let Ok(v) = std::env::var("COSH_SHELL_AI") {
        config.ai_enabled = v != "off";
    }
    if let Ok(v) = std::env::var("COSH_SHELL_DEBUG") {
        config.debug = parse_bool_value(&v);
    }
    if let Ok(v) = std::env::var("COSH_SHELL_LANG") {
        apply_language_value(config, &v);
    }
}
