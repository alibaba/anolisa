//! `anolisa list` — list available components from the component index.
//!
//! Reads the repo-side `components.toml` (the component identity index),
//! merges install status from `installed.toml`, and renders as a human
//! table or `--json` envelope.

mod render;
mod state_view;

#[cfg(test)]
mod tests;

#[cfg(test)]
use anolisa_core::state::InstalledState;
use anolisa_platform::pkg_query::PackageQuery;
use anolisa_platform::rpm_query::RpmPackageQuery;
use clap::Parser;
use serde::Serialize;

use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::commands::state_view::{StateScope, StateView, StateVisibility};
use crate::context::{CliContext, InstallMode};
use crate::resolution::{ComponentIndex, ComponentIndexEntry, load_component_index};
use crate::response::{CliError, render_json};

use self::render::render_human;
use self::state_view::{LocalProjection, project_component};

const COMMAND: &str = "list";

#[derive(Parser)]
pub struct ListArgs {
    /// Show only currently installed components
    #[arg(long, alias = "enabled")]
    pub installed: bool,
}

// ── Wire / JSON output types ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub display_name: String,
    pub summary: String,
    pub backends: Vec<String>,
    pub status: String,
    pub local_state: String,
    pub ownership: String,
    pub scope: String,
    pub active: bool,
    pub mutable_by_current_invocation: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_path: Option<String>,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpm_package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpm_evr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpm_arch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpm_source_repo: Option<String>,
}

#[derive(Serialize)]
struct ListPayload {
    components: Vec<Row>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

// ── Handler ────────────────────────────────────────────────────────

pub fn handle(args: ListArgs, ctx: &CliContext) -> Result<(), CliError> {
    let layout = common::resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config =
        common::load_repo_config(ctx, &layout, COMMAND, RepoPersistPolicy::BestEffort)?;

    let index =
        load_component_index(&layout, &env, &repo_config).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to load component index: {err}"),
        })?;

    let view = StateView::load(ctx, COMMAND, StateVisibility::UserPlusSystem)?;
    let rpm_query = match ctx.install_mode {
        InstallMode::System => Some(RpmPackageQuery::system()),
        InstallMode::User => None,
    };
    let rows = build_rows_from_view(
        &index,
        &args,
        &view,
        rpm_query.as_ref().map(|query| query as &dyn PackageQuery),
    );

    if ctx.json {
        return render_json(
            COMMAND,
            ListPayload {
                components: rows,
                warnings: view.warnings,
            },
        );
    }

    if !ctx.quiet {
        render_warnings(&view.warnings);
        render_human(&rows, ctx.no_color);
    }
    Ok(())
}

#[cfg(test)]
fn build_rows(
    index: &ComponentIndex,
    args: &ListArgs,
    state: &InstalledState,
    rpm_query: Option<&dyn PackageQuery>,
) -> Vec<Row> {
    index
        .components
        .iter()
        .filter_map(|entry| {
            let projection = project_component(entry, state, rpm_query);
            if args.installed && !projection.local_state.matches_installed_filter() {
                return None;
            }
            Some(entry_to_row(
                entry,
                projection,
                RowScope {
                    scope: "none".to_string(),
                    active: false,
                    mutable_by_current_invocation: false,
                    shadowed_by: None,
                    state_path: None,
                },
            ))
        })
        .collect()
}

fn build_rows_from_view(
    index: &ComponentIndex,
    args: &ListArgs,
    view: &StateView,
    rpm_query: Option<&dyn PackageQuery>,
) -> Vec<Row> {
    let visible_components = view.visible_components();
    index
        .components
        .iter()
        .filter_map(|entry| {
            let active = visible_components
                .iter()
                .find(|record| record.object.name == entry.name && record.active);
            let (projection, row_scope) = match active {
                Some(record) => (
                    project_component(entry, &record.root.state, rpm_query),
                    RowScope {
                        scope: record.scope().label().to_string(),
                        active: true,
                        mutable_by_current_invocation: record.mutable_by_current_invocation,
                        shadowed_by: record
                            .shadowed_by
                            .map(StateScope::label)
                            .map(str::to_string),
                        state_path: Some(record.root.state_path.display().to_string()),
                    },
                ),
                None => {
                    let projection = project_component(entry, &view.writable.state, rpm_query);
                    let scope = match projection.local_state {
                        self::state_view::LocalState::Observed => view.writable.scope.label(),
                        self::state_view::LocalState::Tracked
                        | self::state_view::LocalState::Installed
                        | self::state_view::LocalState::Drifted
                        | self::state_view::LocalState::Missing
                        | self::state_view::LocalState::Failed
                        | self::state_view::LocalState::Degraded
                        | self::state_view::LocalState::Disabled
                        | self::state_view::LocalState::NotInstalled => "none",
                    };
                    (
                        projection,
                        RowScope {
                            scope: scope.to_string(),
                            active: false,
                            mutable_by_current_invocation: false,
                            shadowed_by: None,
                            state_path: None,
                        },
                    )
                }
            };
            if args.installed && !projection.local_state.matches_installed_filter() {
                return None;
            }
            Some(entry_to_row(entry, projection, row_scope))
        })
        .collect()
}

struct RowScope {
    scope: String,
    active: bool,
    mutable_by_current_invocation: bool,
    shadowed_by: Option<String>,
    state_path: Option<String>,
}

fn entry_to_row(
    entry: &ComponentIndexEntry,
    projection: LocalProjection,
    row_scope: RowScope,
) -> Row {
    let backends: Vec<String> = entry.backends.iter().map(|b| b.kind.clone()).collect();
    let local_state = projection.local_state.label().to_string();
    let ownership = projection.ownership_label().to_string();
    let action = projection.action_label().to_string();
    Row {
        name: entry.name.clone(),
        display_name: entry
            .display_name
            .clone()
            .unwrap_or_else(|| entry.name.clone()),
        summary: entry.summary.clone().unwrap_or_default(),
        backends,
        status: projection.status,
        local_state,
        ownership,
        scope: row_scope.scope,
        active: row_scope.active,
        mutable_by_current_invocation: row_scope.mutable_by_current_invocation,
        shadowed_by: row_scope.shadowed_by,
        state_path: row_scope.state_path,
        action,
        rpm_package: projection.rpm_package,
        rpm_evr: projection.rpm_evr,
        rpm_arch: projection.rpm_arch,
        rpm_source_repo: projection.rpm_source_repo,
    }
}

fn render_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}
