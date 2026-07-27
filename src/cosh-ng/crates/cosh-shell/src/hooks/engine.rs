#[cfg(test)]
use crate::hooks::model::HookTrigger;
use crate::hooks::model::{HookInput, HookMatcher};
#[cfg(test)]
use crate::types::HookProvenance;
use crate::types::{
    BuiltinFindingFacts, CommandBlock, CommandOrigin, EvaluatedHookFinding, FindingSeverity,
    HookFinding,
};
use loader::load_external_hook_configs;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[path = "engine/loader.rs"]
mod loader;
#[path = "engine/matcher.rs"]
mod matcher;
#[path = "engine/runtime.rs"]
mod runtime;

pub trait BuiltinHook: Send + Sync {
    fn id(&self) -> &str;
    fn matcher(&self) -> &HookMatcher;
    fn evaluate(&self, input: &HookInput) -> Option<HookFinding>;
    fn builtin_facts(&self, _input: &HookInput) -> Option<BuiltinFindingFacts> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct ExternalHookConfig {
    pub path: PathBuf,
    pub matcher: HookMatcher,
    pub timeout_ms: u64,
    pub source: ExternalHookSource,
    pub project_root: Option<PathBuf>,
    pub trusted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalHookSource {
    User,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredHookInfo {
    pub id: String,
    pub source: HookSourceInfo,
    pub path: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
    pub trusted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSourceInfo {
    Builtin,
    ExternalUser,
    ExternalProject,
}

pub struct HookEngine {
    builtin_hooks: Vec<Box<dyn BuiltinHook>>,
    external_hooks: Vec<ExternalHookConfig>,
}

impl Default for HookEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HookEngine {
    pub fn new() -> Self {
        Self {
            builtin_hooks: Vec::new(),
            external_hooks: Vec::new(),
        }
    }

    pub fn register(&mut self, hook: Box<dyn BuiltinHook>) {
        self.builtin_hooks.push(hook);
    }

    pub fn register_external(&mut self, config: ExternalHookConfig) {
        self.external_hooks.push(config);
    }

    pub fn load_hooks_from_dir(&mut self, dir: &Path) {
        self.load_external_hooks_from_dir(dir, ExternalHookSource::User, None, true);
    }

    pub fn load_project_hooks_from_root(&mut self, project_root: &Path, trusted: bool) {
        let root = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let hooks_dir = root.join(".cosh/hooks");
        self.load_external_hooks_from_dir(
            &hooks_dir,
            ExternalHookSource::Project,
            Some(root),
            trusted,
        );
    }

    fn load_external_hooks_from_dir(
        &mut self,
        dir: &Path,
        source: ExternalHookSource,
        project_root: Option<PathBuf>,
        trusted: bool,
    ) {
        self.external_hooks.extend(load_external_hook_configs(
            dir,
            source,
            project_root,
            trusted,
        ));
    }

    pub fn evaluate(&self, block: &CommandBlock) -> Vec<EvaluatedHookFinding> {
        self.evaluate_with_disabled(block, &HashSet::new())
    }

    pub fn evaluate_with_disabled(
        &self,
        block: &CommandBlock,
        disabled_hooks: &HashSet<String>,
    ) -> Vec<EvaluatedHookFinding> {
        self.evaluate_with_disabled_and_origin(
            block,
            disabled_hooks,
            CommandOrigin::UserInteractive,
        )
    }

    pub fn evaluate_with_disabled_and_origin(
        &self,
        block: &CommandBlock,
        disabled_hooks: &HashSet<String>,
        origin: CommandOrigin,
    ) -> Vec<EvaluatedHookFinding> {
        let input = runtime::hook_input_from_block(block);
        let mut findings = Vec::new();
        for hook in &self.builtin_hooks {
            if disabled_hooks.contains(hook.id()) {
                continue;
            }
            if matcher::matches_command(hook.matcher(), &input) {
                if let Some(finding) = hook.evaluate(&input) {
                    findings.push(EvaluatedHookFinding::builtin_with_facts(
                        hook.id(),
                        finding,
                        hook.builtin_facts(&input),
                    ));
                }
            }
        }
        for (registration_index, ext) in self.external_hooks.iter().enumerate() {
            if disabled_hooks.contains(&ext.matcher.id) {
                continue;
            }
            if ext.source == ExternalHookSource::Project && !ext.trusted {
                continue;
            }
            if !external_hook_allowed_for_origin(ext, origin) {
                continue;
            }
            if matcher::matches_command(&ext.matcher, &input) {
                if let Some(finding) = runtime::run_external_hook(ext, &input) {
                    findings.push(EvaluatedHookFinding::external(
                        format!("external:{registration_index}"),
                        finding,
                    ));
                }
            }
        }
        for finding in &mut findings {
            runtime::redact_hook_finding(&mut finding.finding);
        }
        findings.sort_by_key(|f| match f.severity {
            FindingSeverity::Critical => 0,
            FindingSeverity::Warning => 1,
            FindingSeverity::Info => 2,
        });
        findings
    }

    pub fn registered_hooks(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.builtin_hooks.iter().map(|h| h.id()).collect();
        for ext in &self.external_hooks {
            ids.push(&ext.matcher.id);
        }
        ids
    }

    pub fn registered_hook_infos(&self) -> Vec<RegisteredHookInfo> {
        let mut hooks = self
            .builtin_hooks
            .iter()
            .map(|hook| RegisteredHookInfo {
                id: hook.id().to_string(),
                source: HookSourceInfo::Builtin,
                path: None,
                project_root: None,
                trusted: None,
            })
            .collect::<Vec<_>>();
        for ext in &self.external_hooks {
            hooks.push(RegisteredHookInfo {
                id: ext.matcher.id.clone(),
                source: match ext.source {
                    ExternalHookSource::User => HookSourceInfo::ExternalUser,
                    ExternalHookSource::Project => HookSourceInfo::ExternalProject,
                },
                path: Some(ext.path.clone()),
                project_root: ext.project_root.clone(),
                trusted: Some(ext.trusted),
            });
        }
        hooks
    }

    pub fn set_project_hooks_trusted(&mut self, trusted: bool) -> usize {
        let mut updated = 0;
        for ext in &mut self.external_hooks {
            if ext.source == ExternalHookSource::Project {
                ext.trusted = trusted;
                updated += 1;
            }
        }
        updated
    }

    pub fn external_hooks(&self) -> &[ExternalHookConfig] {
        &self.external_hooks
    }
}

fn external_hook_allowed_for_origin(config: &ExternalHookConfig, origin: CommandOrigin) -> bool {
    match config.source {
        ExternalHookSource::User => matches!(
            origin,
            CommandOrigin::UserInteractive | CommandOrigin::UserSendToShell
        ),
        ExternalHookSource::Project => matches!(origin, CommandOrigin::UserInteractive),
    }
}

#[cfg(test)]
#[path = "engine/tests.rs"]
mod tests;
