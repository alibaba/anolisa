use std::path::{Path, PathBuf};

use anolisa_core::{InstalledObject, InstalledState, ObjectKind};
use anolisa_platform::fs_layout::FsLayout;

use crate::commands::common;
use crate::context::{CliContext, InstallMode};
use crate::response::CliError;

const INSTALLED_STATE_FILE: &str = "installed.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateScope {
    User,
    System,
}

impl StateScope {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateVisibility {
    UserPlusSystem,
}

#[derive(Debug, Clone)]
pub(crate) struct ScopedStateRoot {
    pub(crate) scope: StateScope,
    pub(crate) layout: FsLayout,
    pub(crate) state_path: PathBuf,
    pub(crate) writable: bool,
    pub(crate) state: InstalledState,
}

#[derive(Debug, Clone)]
pub(crate) struct StateView {
    pub(crate) writable: ScopedStateRoot,
    pub(crate) visible_roots: Vec<ScopedStateRoot>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct RootSpec {
    scope: StateScope,
    writable: bool,
}

impl StateView {
    pub(crate) fn load(
        ctx: &CliContext,
        command: &str,
        visibility: StateVisibility,
    ) -> Result<Self, CliError> {
        let current_layout = common::resolve_layout(ctx);
        let current_scope = scope_for_mode(ctx.install_mode);
        let mut roots = Vec::new();
        roots.push((
            current_layout,
            RootSpec {
                scope: current_scope,
                writable: true,
            },
        ));

        if ctx.install_mode == InstallMode::User && visibility == StateVisibility::UserPlusSystem {
            roots.push((
                FsLayout::system(ctx.prefix.clone()),
                RootSpec {
                    scope: StateScope::System,
                    writable: false,
                },
            ));
        }

        Self::from_layouts(command, roots)
    }

    pub(crate) fn visible_components(&self) -> Vec<ScopedInstalledObject<'_>> {
        let mut records = Vec::new();
        for root in &self.visible_roots {
            for object in root
                .state
                .objects
                .iter()
                .filter(|object| object.kind == ObjectKind::Component)
            {
                let shadowed_by = records
                    .iter()
                    .find(|record: &&ScopedInstalledObject<'_>| record.object.name == object.name)
                    .map(ScopedInstalledObject::scope);
                records.push(ScopedInstalledObject {
                    root,
                    object,
                    active: shadowed_by.is_none(),
                    shadowed_by,
                    mutable_by_current_invocation: root.writable,
                });
            }
        }
        records
    }

    fn from_layouts(command: &str, roots: Vec<(FsLayout, RootSpec)>) -> Result<Self, CliError> {
        let mut visible_roots = Vec::new();
        let mut writable = None;
        let mut warnings = Vec::new();

        for (layout, spec) in roots {
            let state_path = layout.state_dir.join(INSTALLED_STATE_FILE);
            let loaded = load_root_state(command, &state_path, spec.writable);
            let state = match loaded {
                Ok(state) => state,
                Err(RootLoad::Fatal(err)) => return Err(err),
                Err(RootLoad::Warning(warning)) => {
                    warnings.push(warning);
                    continue;
                }
            };
            let root = ScopedStateRoot {
                scope: spec.scope,
                layout,
                state_path,
                writable: spec.writable,
                state,
            };
            if spec.writable {
                writable = Some(root.clone());
            }
            visible_roots.push(root);
        }

        let Some(writable) = writable else {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: "state view has no writable root".to_string(),
            });
        };

        Ok(Self {
            writable,
            visible_roots,
            warnings,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScopedInstalledObject<'a> {
    pub(crate) root: &'a ScopedStateRoot,
    pub(crate) object: &'a InstalledObject,
    pub(crate) active: bool,
    pub(crate) shadowed_by: Option<StateScope>,
    pub(crate) mutable_by_current_invocation: bool,
}

impl ScopedInstalledObject<'_> {
    pub(crate) const fn scope(&self) -> StateScope {
        self.root.scope
    }
}

enum RootLoad {
    Fatal(CliError),
    Warning(String),
}

fn load_root_state(
    command: &str,
    state_path: &Path,
    writable: bool,
) -> Result<InstalledState, RootLoad> {
    InstalledState::load(state_path).map_err(|err| {
        if writable {
            RootLoad::Fatal(CliError::InvalidArgument {
                command: command.to_string(),
                reason: format!(
                    "failed to load installed state at {}: {err}",
                    state_path.display()
                ),
            })
        } else {
            RootLoad::Warning(format!(
                "failed to load visible system state at {}: {err}",
                state_path.display()
            ))
        }
    })
}

const fn scope_for_mode(mode: InstallMode) -> StateScope {
    match mode {
        InstallMode::User => StateScope::User,
        InstallMode::System => StateScope::System,
    }
}

#[cfg(test)]
mod tests;
