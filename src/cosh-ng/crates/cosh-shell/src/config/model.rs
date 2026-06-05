use std::path::PathBuf;

use crate::tools::readonly_rules::RuntimeReadonlyConfig;

#[derive(Debug, Clone)]
pub struct CoshConfig {
    pub shell_default: String,
    pub analysis_mode: String,
    pub approval_mode: String,
    pub adapter_default: String,
    pub language: String,
    pub startup_banner: bool,
    pub startup_hooks: bool,
    pub debug: bool,
    pub ai_enabled: bool,
    pub trusted_commands: Vec<String>,
    pub trusted_project_roots: Vec<PathBuf>,
    pub(super) readonly: RuntimeReadonlyConfig,
}

impl Default for CoshConfig {
    fn default() -> Self {
        Self {
            shell_default: "auto".into(),
            analysis_mode: "smart".into(),
            approval_mode: "auto".into(),
            adapter_default: "cosh-tui".into(),
            language: "auto".into(),
            startup_banner: true,
            startup_hooks: false,
            debug: false,
            ai_enabled: true,
            trusted_commands: Vec::new(),
            trusted_project_roots: Vec::new(),
            readonly: RuntimeReadonlyConfig::default(),
        }
    }
}

impl CoshConfig {
    pub(crate) fn readonly_config(&self) -> &RuntimeReadonlyConfig {
        &self.readonly
    }
}
