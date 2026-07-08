//! `anolisa list` — list available components from the component index.
//!
//! Reads the repo-side `components.toml` (the component identity index),
//! merges install status from `installed.toml`, and renders as a human
//! table or `--json` envelope.

mod render;
mod state_view;

#[cfg(test)]
mod tests;

use anolisa_platform::pkg_query::PackageQuery;
use anolisa_platform::rpm_query::RpmPackageQuery;
use clap::Parser;
use serde::Serialize;

use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::commands::visible_view::VisibleInstalledView;
use crate::context::CliContext;
use crate::resolution::{ComponentIndex, ComponentIndexEntry, load_component_index};
use crate::response::{CliError, render_json};

use self::render::render_human;
use self::state_view::{LocalProjection, project_visible};

const COMMAND: &str = "list";

#[derive(Parser)]
pub struct ListArgs {
    /// Show only currently installed components
    #[arg(long, alias = "enabled")]
    pub installed: bool,
}

/// Wire / JSON output type for one component row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub display_name: String,
    pub summary: String,
    pub backends: Vec<String>,
    pub status: String,
    pub local_state: String,
    pub ownership: String,
    pub action: String,
    /// Physical scope of the active record: `"user"` or `"system"`.
    pub scope: String,
    /// Whether this row is the active (highest-priority) record.
    pub active: bool,
    /// When `active = false`, which scope shadows this record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<String>,
    /// Whether the current user can directly modify this record.
    pub mutable_by_current_user: bool,
    /// Path to the `installed.toml` this record came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_path: Option<String>,
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

    // Load current-scope state via fail-fast path so corrupted state
    // surfaces as an error instead of being silently treated as empty.
    let state = common::load_installed_state(ctx, COMMAND)?;
    let view = VisibleInstalledView::load_with_current_state(ctx, &state);

    if !view.system_state_readable() && !ctx.quiet {
        let color = crate::color::Palette::new(ctx.no_color);
        if let Some(path) = view.system_state_path() {
            eprintln!(
                "{} could not read system state at {}",
                color.warn("\u{26a0}"),
                color.path(path.display().to_string()),
            );
        } else {
            eprintln!(
                "{} system state is not available; showing user-scope records only",
                color.warn("\u{26a0}"),
            );
        }
    }

    let rpm_query = RpmPackageQuery::system();
    let rows = build_rows(&index, &args, &view, &rpm_query as &dyn PackageQuery);

    if ctx.json {
        return render_json(COMMAND, ListPayload { components: rows });
    }

    if !ctx.quiet {
        render_human(&rows, ctx.no_color);
    }
    Ok(())
}

fn build_rows(
    index: &ComponentIndex,
    args: &ListArgs,
    view: &VisibleInstalledView,
    rpm_query: &dyn PackageQuery,
) -> Vec<Row> {
    index
        .components
        .iter()
        .filter_map(|entry| {
            let projection = project_visible(entry, view, Some(rpm_query));
            if args.installed && !projection.local_state.matches_installed_filter() {
                return None;
            }
            // Collect scope metadata from the visible view for this component.
            let records = view.records_for(&entry.name);
            let active_record = records.first().filter(|r| r.active);
            let (scope, active, shadowed_by, mutable, state_path) = scope_fields(active_record);
            Some(entry_to_row(
                entry,
                projection,
                scope,
                active,
                shadowed_by,
                mutable,
                state_path,
            ))
        })
        .collect()
}

/// Derive scope-related `Row` fields from the active visible record (if any).
///
/// When the component is not in any scope's state (local_state = `not_installed`
/// or `observed`), scope/active/mutable are filled with defaults because there
/// is no physical record to attribute them to.
fn scope_fields(
    active_record: Option<&&crate::commands::visible_view::VisibleRecord>,
) -> (String, bool, Option<String>, bool, Option<String>) {
    match active_record {
        Some(rec) => (
            rec.scope.as_str().to_string(),
            rec.active,
            rec.shadowed_by.map(|s| s.as_str().to_string()),
            rec.mutable_by_current_user,
            Some(rec.state_path.display().to_string()),
        ),
        None => ("none".to_string(), false, None, false, None),
    }
}

fn entry_to_row(
    entry: &ComponentIndexEntry,
    projection: LocalProjection,
    scope: String,
    active: bool,
    shadowed_by: Option<String>,
    mutable_by_current_user: bool,
    state_path: Option<String>,
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
        action,
        scope,
        active,
        shadowed_by,
        mutable_by_current_user,
        state_path,
        rpm_package: projection.rpm_package,
        rpm_evr: projection.rpm_evr,
        rpm_arch: projection.rpm_arch,
        rpm_source_repo: projection.rpm_source_repo,
    }
}
