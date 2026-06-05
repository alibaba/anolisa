use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CoshConfig {
    pub shell_default: String,
    pub analysis_mode: String,
    pub approval_mode: String,
    pub adapter_default: String,
    pub startup_banner: bool,
    pub startup_hooks: bool,
    pub ai_enabled: bool,
}

impl Default for CoshConfig {
    fn default() -> Self {
        Self {
            shell_default: "bash".into(),
            analysis_mode: "smart".into(),
            approval_mode: "ask".into(),
            adapter_default: "claude".into(),
            startup_banner: true,
            startup_hooks: false,
            ai_enabled: true,
        }
    }
}

pub fn load_config() -> CoshConfig {
    let mut config = CoshConfig::default();

    if let Some(path) = config_file_path() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            parse_simple_config(&content, &mut config);
        }
    }

    if let Ok(v) = std::env::var("COSH_SHELL_ANALYSIS_MODE") {
        config.analysis_mode = v;
    }
    if let Ok(v) = std::env::var("COSH_SHELL_AI") {
        config.ai_enabled = v != "off";
    }

    config
}

fn config_file_path() -> Option<PathBuf> {
    dirs_next_or_home().map(|d| d.join(".config/cosh/config.toml"))
}

fn dirs_next_or_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn parse_simple_config(content: &str, config: &mut CoshConfig) {
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
                "adapter.default" => config.adapter_default = value.into(),
                "ui.startup_banner" => config.startup_banner = value == "true",
                "ui.startup_hooks" => config.startup_hooks = value == "true",
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = CoshConfig::default();
        assert_eq!(cfg.shell_default, "bash");
        assert_eq!(cfg.analysis_mode, "smart");
        assert_eq!(cfg.approval_mode, "ask");
        assert_eq!(cfg.adapter_default, "claude");
        assert!(cfg.startup_banner);
        assert!(!cfg.startup_hooks);
        assert!(cfg.ai_enabled);
    }

    #[test]
    fn parse_simple_key_value() {
        let content = r#"
shell.default = "zsh"
analysis.mode = conservative
adapter.default = "qwen"
"#;
        let mut cfg = CoshConfig::default();
        parse_simple_config(content, &mut cfg);
        assert_eq!(cfg.shell_default, "zsh");
        assert_eq!(cfg.analysis_mode, "conservative");
        assert_eq!(cfg.adapter_default, "qwen");
    }

    #[test]
    fn parse_ignores_comments_and_blank_lines() {
        let content = "# this is a comment\n\nshell.default = fish\n";
        let mut cfg = CoshConfig::default();
        parse_simple_config(content, &mut cfg);
        assert_eq!(cfg.shell_default, "fish");
        assert_eq!(cfg.analysis_mode, "smart");
    }

    #[test]
    fn parse_boolean_fields() {
        let content = "ui.startup_banner = false\nui.startup_hooks = true\n";
        let mut cfg = CoshConfig::default();
        parse_simple_config(content, &mut cfg);
        assert!(!cfg.startup_banner);
        assert!(cfg.startup_hooks);
    }

    #[test]
    fn parse_unknown_keys_ignored() {
        let content = "unknown.key = value\nshell.default = dash\n";
        let mut cfg = CoshConfig::default();
        parse_simple_config(content, &mut cfg);
        assert_eq!(cfg.shell_default, "dash");
    }
}
