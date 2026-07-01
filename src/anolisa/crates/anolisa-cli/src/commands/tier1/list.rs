//! `anolisa list` — list available components from the component index.
//!
//! Reads the repo-side `components.toml` (the component identity index),
//! merges install status from `installed.toml`, and renders as a human
//! table or `--json` envelope.

use anolisa_core::state::{InstalledState, ObjectKind};
use clap::Parser;
use serde::Serialize;

use crate::color::{Palette, pad_right};
use crate::commands::common;
use crate::context::CliContext;
use crate::resolution::{ComponentIndex, ComponentIndexEntry, load_component_index};
use crate::response::{CliError, render_json};

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
        crate::repo_config::RepoConfig::load(&layout).map_err(|err| CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!("failed to load repo.toml: {err}"),
        })?;

    let index =
        load_component_index(&layout, &env, &repo_config).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to load component index: {err}"),
        })?;

    let state = common::load_installed_state(ctx, COMMAND)?;
    let rows = build_rows(&index, &args, &state);

    if ctx.json {
        return render_json(COMMAND, ListPayload { components: rows });
    }

    if !ctx.quiet {
        render_human(&rows, ctx.no_color);
    }
    Ok(())
}

fn build_rows(index: &ComponentIndex, args: &ListArgs, state: &InstalledState) -> Vec<Row> {
    index
        .components
        .iter()
        .map(|entry| entry_to_row(entry, state))
        .filter(|row| !args.installed || common::status_is_enabled(&row.status))
        .collect()
}

fn entry_to_row(entry: &ComponentIndexEntry, state: &InstalledState) -> Row {
    let backends: Vec<String> = entry.backends.iter().map(|b| b.kind.clone()).collect();
    let status = state
        .find_object(ObjectKind::Component, &entry.name)
        .map(|obj| common::object_status_str(obj.status))
        .unwrap_or("not_installed");
    Row {
        name: entry.name.clone(),
        display_name: entry
            .display_name
            .clone()
            .unwrap_or_else(|| entry.name.clone()),
        summary: entry.summary.clone().unwrap_or_default(),
        backends,
        status: status.to_string(),
    }
}

fn render_human(rows: &[Row], no_color: bool) {
    let color = Palette::new(no_color);
    if rows.is_empty() {
        println!("{}", color.muted("no components found"));
        return;
    }

    let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4) + 2;
    let summary_width = rows
        .iter()
        .map(|r| r.summary.chars().count())
        .max()
        .unwrap_or(7)
        .clamp(7, 50)
        + 2;
    let backends_width = rows
        .iter()
        .map(|r| {
            if r.backends.is_empty() {
                1
            } else {
                r.backends.join(",").len()
            }
        })
        .max()
        .unwrap_or(8)
        .max(8)
        + 2;

    println!(
        "{}",
        color.header(format!(
            "{:<name_width$}{:<summary_width$}{:<backends_width$}{}",
            "NAME", "SUMMARY", "BACKENDS", "STATUS",
        ))
    );
    for row in rows {
        let backend_str = if row.backends.is_empty() {
            "-".to_string()
        } else {
            row.backends.join(",")
        };
        let max_chars = summary_width - 2;
        let summary = if row.summary.chars().count() > max_chars {
            let truncated: String = row.summary.chars().take(max_chars - 1).collect();
            format!("{truncated}…")
        } else {
            row.summary.clone()
        };
        println!(
            "{:<name_width$}{:<summary_width$}{:<backends_width$}{}",
            row.name,
            summary,
            backend_str,
            color.status(pad_right(&row.status, 14)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolution::ComponentBackendEntry;
    use anolisa_core::state::{InstalledObject, ObjectStatus};

    fn sample_index() -> ComponentIndex {
        ComponentIndex {
            schema_version: 1,
            generated_at: None,
            publisher: Some("anolisa".to_string()),
            components: vec![
                ComponentIndexEntry {
                    name: "agentsight".to_string(),
                    display_name: Some("AgentSight".to_string()),
                    summary: Some("eBPF-based AI agent observability tool".to_string()),
                    backends: vec![
                        ComponentBackendEntry {
                            kind: "raw".to_string(),
                            package: "agentsight".to_string(),
                            provides: None,
                            legacy_adopt: false,
                        },
                        ComponentBackendEntry {
                            kind: "rpm".to_string(),
                            package: "agentsight".to_string(),
                            provides: Some("anolisa-component(agentsight)".to_string()),
                            legacy_adopt: true,
                        },
                    ],
                    aliases: Vec::new(),
                },
                ComponentIndexEntry {
                    name: "tokenless".to_string(),
                    display_name: Some("Tokenless".to_string()),
                    summary: Some("LLM token optimization toolkit".to_string()),
                    backends: vec![ComponentBackendEntry {
                        kind: "raw".to_string(),
                        package: "tokenless".to_string(),
                        provides: None,
                        legacy_adopt: false,
                    }],
                    aliases: Vec::new(),
                },
            ],
        }
    }

    fn empty_state() -> InstalledState {
        InstalledState::default()
    }

    fn state_with_object(kind: ObjectKind, name: &str, status: ObjectStatus) -> InstalledState {
        let mut state = InstalledState::default();
        state.objects.push(InstalledObject {
            kind,
            name: name.to_string(),
            version: "0.1.0".to_string(),
            status,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: None,
            ownership: None,
            rpm_metadata: None,
            installed_at: "2026-06-12T00:00:00Z".to_string(),
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        state
    }

    #[test]
    fn index_builds_rows() {
        let index = sample_index();
        let args = ListArgs { installed: false };
        let state = empty_state();
        let rows = build_rows(&index, &args, &state);
        assert_eq!(rows.len(), 2);

        let sight = &rows[0];
        assert_eq!(sight.name, "agentsight");
        assert_eq!(sight.display_name, "AgentSight");
        assert_eq!(sight.summary, "eBPF-based AI agent observability tool");
        assert_eq!(sight.backends, vec!["raw", "rpm"]);
        assert_eq!(sight.status, "not_installed");

        let token = &rows[1];
        assert_eq!(token.name, "tokenless");
        assert_eq!(token.backends, vec!["raw"]);
    }

    #[test]
    fn empty_state_all_not_installed() {
        let index = sample_index();
        let args = ListArgs { installed: false };
        let state = empty_state();
        let rows = build_rows(&index, &args, &state);
        for row in &rows {
            assert_eq!(row.status, "not_installed");
        }
    }

    #[test]
    fn installed_component_shows_installed() {
        let index = sample_index();
        let args = ListArgs { installed: false };
        let state = state_with_object(ObjectKind::Component, "tokenless", ObjectStatus::Installed);
        let rows = build_rows(&index, &args, &state);

        let sight = rows.iter().find(|r| r.name == "agentsight").unwrap();
        assert_eq!(sight.status, "not_installed");

        let token = rows.iter().find(|r| r.name == "tokenless").unwrap();
        assert_eq!(token.status, "installed");
    }

    #[test]
    fn adopted_rpm_component_shows_adopted() {
        let index = sample_index();
        let args = ListArgs { installed: false };
        let state = state_with_object(ObjectKind::Component, "agentsight", ObjectStatus::Adopted);
        let rows = build_rows(&index, &args, &state);

        let sight = rows.iter().find(|r| r.name == "agentsight").unwrap();
        assert_eq!(sight.status, "adopted");
    }

    #[test]
    fn adapter_object_does_not_mark_component_installed() {
        let index = sample_index();
        let args = ListArgs { installed: false };
        let state = state_with_object(ObjectKind::Adapter, "tokenless", ObjectStatus::Installed);
        let rows = build_rows(&index, &args, &state);
        let token = rows.iter().find(|r| r.name == "tokenless").unwrap();
        assert_eq!(token.status, "not_installed");
    }

    #[test]
    fn failed_component_shows_failed() {
        let index = sample_index();
        let args = ListArgs { installed: false };
        let state = state_with_object(ObjectKind::Component, "tokenless", ObjectStatus::Failed);
        let rows = build_rows(&index, &args, &state);
        let token = rows.iter().find(|r| r.name == "tokenless").unwrap();
        assert_eq!(token.status, "failed");
    }

    #[test]
    fn disabled_component_shows_disabled() {
        let index = sample_index();
        let args = ListArgs { installed: false };
        let state = state_with_object(ObjectKind::Component, "tokenless", ObjectStatus::Disabled);
        let rows = build_rows(&index, &args, &state);
        let token = rows.iter().find(|r| r.name == "tokenless").unwrap();
        assert_eq!(token.status, "disabled");
    }

    #[test]
    fn installed_filter_returns_only_installed() {
        let index = sample_index();
        let args = ListArgs { installed: true };
        let state = state_with_object(ObjectKind::Component, "tokenless", ObjectStatus::Installed);
        let rows = build_rows(&index, &args, &state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "tokenless");
        assert_eq!(rows[0].status, "installed");
    }

    #[test]
    fn installed_filter_includes_adopted_rpm_components() {
        let index = sample_index();
        let args = ListArgs { installed: true };
        let state = state_with_object(ObjectKind::Component, "agentsight", ObjectStatus::Adopted);
        let rows = build_rows(&index, &args, &state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "agentsight");
        assert_eq!(rows[0].status, "adopted");
    }

    #[test]
    fn installed_filter_with_empty_state_returns_empty() {
        let index = sample_index();
        let args = ListArgs { installed: true };
        let state = empty_state();
        let rows = build_rows(&index, &args, &state);
        assert!(rows.is_empty());
    }

    #[test]
    fn json_payload_uses_components_key() {
        let index = sample_index();
        let args = ListArgs { installed: false };
        let state = empty_state();
        let rows = build_rows(&index, &args, &state);
        let payload = ListPayload { components: rows };
        let json_str = serde_json::to_string(&payload).expect("serialize");
        let val: serde_json::Value = serde_json::from_str(&json_str).expect("reparse");
        assert!(val.get("components").is_some());
    }

    #[test]
    fn json_payload_status_reflects_install_state() {
        let index = sample_index();
        let args = ListArgs { installed: false };
        let state = state_with_object(ObjectKind::Component, "agentsight", ObjectStatus::Installed);
        let rows = build_rows(&index, &args, &state);
        let payload = ListPayload { components: rows };
        let json_str = serde_json::to_string(&payload).expect("serialize");
        let val: serde_json::Value = serde_json::from_str(&json_str).expect("reparse");
        let components = val["components"].as_array().unwrap();
        let sight = components
            .iter()
            .find(|c| c["name"] == "agentsight")
            .unwrap();
        assert_eq!(sight["status"], "installed");
        let token = components
            .iter()
            .find(|c| c["name"] == "tokenless")
            .unwrap();
        assert_eq!(token["status"], "not_installed");
    }

    #[test]
    fn missing_optional_fields_use_defaults() {
        let index = ComponentIndex {
            schema_version: 1,
            generated_at: None,
            publisher: None,
            components: vec![ComponentIndexEntry {
                name: "minimal".to_string(),
                display_name: None,
                summary: None,
                backends: Vec::new(),
                aliases: Vec::new(),
            }],
        };
        let args = ListArgs { installed: false };
        let state = empty_state();
        let rows = build_rows(&index, &args, &state);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.name, "minimal");
        assert_eq!(row.display_name, "minimal");
        assert!(row.summary.is_empty());
        assert!(row.backends.is_empty());
        assert_eq!(row.status, "not_installed");
    }

    #[test]
    fn unknown_backend_kind_preserved() {
        let index = ComponentIndex {
            schema_version: 1,
            generated_at: None,
            publisher: None,
            components: vec![ComponentIndexEntry {
                name: "test".to_string(),
                display_name: None,
                summary: None,
                backends: vec![ComponentBackendEntry {
                    kind: "custom-repo".to_string(),
                    package: "test".to_string(),
                    provides: None,
                    legacy_adopt: false,
                }],
                aliases: Vec::new(),
            }],
        };
        let args = ListArgs { installed: false };
        let state = empty_state();
        let rows = build_rows(&index, &args, &state);
        assert_eq!(rows[0].backends, vec!["custom-repo"]);
    }
}
