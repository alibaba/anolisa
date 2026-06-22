//! `anolisa list` — list available components from a remote catalog.
//!
//! Fetches a JSON component declaration file (v1 schema) from a
//! configurable URL, maps each entry to a [`Row`], and renders as a
//! human table or `--json` envelope.
//!
//! Install status is resolved from `installed.toml` by matching
//! [`ObjectKind::Component`] objects against catalog entries.
//! `--enabled` filters to rows whose status is `installed`.

use anolisa_core::state::{InstalledState, ObjectKind, ObjectStatus};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::color::{Palette, pad_right};
use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, CliResponse, SCHEMA_VERSION, render_json};

const COMMAND: &str = "list";

#[derive(Parser)]
pub struct ListArgs {
    /// Show only components marked as available
    #[arg(long)]
    pub available: bool,
    /// Show only currently installed components
    #[arg(long)]
    pub enabled: bool,
}

// ── Deserialization types for the v1 component catalog JSON ─────────

#[derive(Debug, Deserialize)]
struct ComponentCatalogV1 {
    schema_version: u32,
    #[serde(default)]
    components: Vec<ComponentEntry>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ComponentEntry {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    backends: Option<Vec<BackendEntry>>,
    #[serde(default)]
    platforms: Option<Vec<PlatformEntry>>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct BackendEntry {
    #[serde(rename = "type")]
    backend_type: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PlatformEntry {
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    arch: Option<String>,
    #[serde(default)]
    distros: Option<Vec<String>>,
}

// ── Wire / JSON output types ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub display_name: String,
    pub summary: String,
    pub category: String,
    pub version: String,
    pub backends: Vec<String>,
    pub status: String,
    pub available: bool,
}

#[derive(Serialize)]
struct ListPayload {
    components: Vec<Row>,
}

// ── Handler ────────────────────────────────────────────────────────

pub fn handle(args: ListArgs, ctx: &CliContext) -> Result<(), CliError> {
    let Some(url) = common::resolve_catalog_url(ctx, COMMAND)? else {
        return render_missing_catalog(ctx);
    };
    let bytes = common::fetch_catalog_bytes(&url, COMMAND)?;
    let catalog = parse_catalog(&bytes)?;
    let state = common::load_installed_state(ctx, COMMAND)?;
    let rows = build_rows(&catalog, &args, &state)?;

    if ctx.json {
        return render_json(COMMAND, ListPayload { components: rows });
    }

    if !ctx.quiet {
        render_human(&rows, ctx.no_color);
    }
    Ok(())
}

fn render_missing_catalog(ctx: &CliContext) -> Result<(), CliError> {
    let config_path = common::resolve_layout(ctx).etc_dir.join("repo.toml");
    let warning = format!(
        "component catalog is not configured; \
         the catalog provides the list of available components — without it, \
         `anolisa ls` cannot display anything. \
         Set ANOLISA_CATALOG_URL or configure [backends.raw].base_url in {}",
        config_path.display()
    );

    if ctx.json {
        let response = CliResponse {
            ok: true,
            schema_version: SCHEMA_VERSION,
            command: COMMAND.to_string(),
            data: Some(ListPayload {
                components: Vec::new(),
            }),
            warnings: vec![warning],
            error: None,
        };
        let s = serde_json::to_string_pretty(&response).map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("failed to serialize JSON response: {err}"),
        })?;
        println!("{s}");
        return Ok(());
    }

    if !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        println!("{}", color.muted("no component catalog configured"));
        println!();
        println!("  The component catalog provides the list of available ANOLISA components.");
        println!("  Without it, `anolisa ls` cannot display anything.");
        println!();
        println!("  {}", color.label("config file:"));
        println!("    {}", config_path.display());
        println!();
        println!("  {}", color.label("option 1 — environment variable:"));
        println!(
            "    export ANOLISA_CATALOG_URL=https://example.com/anolisa-releases/anolisa/v1/catalog.json"
        );
        println!();
        println!("  {}", color.label("option 2 — add to repo.toml:"));
        println!("    [backends.raw]");
        println!("    base_url = \"https://example.com/anolisa-releases/anolisa/v1/\"");
    }
    Ok(())
}

fn parse_catalog(bytes: &[u8]) -> Result<ComponentCatalogV1, CliError> {
    let catalog: ComponentCatalogV1 =
        serde_json::from_slice(bytes).map_err(|err| CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!("failed to parse component catalog JSON: {err}"),
        })?;

    if catalog.schema_version != 1 {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "unsupported component catalog schema_version {}; expected 1",
                catalog.schema_version
            ),
        });
    }

    for entry in &catalog.components {
        if entry.name.trim().is_empty() {
            return Err(CliError::InvalidArgument {
                command: COMMAND.to_string(),
                reason: "component catalog contains an entry with an empty name".to_string(),
            });
        }
    }

    Ok(catalog)
}

fn build_rows(
    catalog: &ComponentCatalogV1,
    args: &ListArgs,
    state: &InstalledState,
) -> Result<Vec<Row>, CliError> {
    let rows: Vec<Row> = catalog
        .components
        .iter()
        .map(|entry| {
            let backends: Vec<String> = entry
                .backends
                .as_ref()
                .map(|bs| bs.iter().map(|b| b.backend_type.clone()).collect())
                .unwrap_or_default();
            let available = entry.status.as_deref() == Some("available");
            let installed = state
                .find_object(ObjectKind::Component, &entry.name)
                .is_some_and(|obj| obj.status == ObjectStatus::Installed);
            let status = if installed {
                "installed"
            } else {
                "not_installed"
            };
            Row {
                name: entry.name.clone(),
                display_name: entry
                    .display_name
                    .clone()
                    .unwrap_or_else(|| entry.name.clone()),
                summary: entry.summary.clone().unwrap_or_default(),
                category: entry.category.clone().unwrap_or_default(),
                version: entry.version.clone().unwrap_or_default(),
                backends,
                status: status.to_string(),
                available,
            }
        })
        .filter(|row| !args.available || row.available)
        .filter(|row| !args.enabled || row.status == "installed")
        .collect();

    Ok(rows)
}

fn render_human(rows: &[Row], no_color: bool) {
    let color = Palette::new(no_color);
    if rows.is_empty() {
        println!("{}", color.muted("no components found"));
        return;
    }
    println!(
        "{}",
        color.header(format!(
            "{:<24} {:<16} {:<10} {:<12} {}",
            "NAME", "CATEGORY", "VERSION", "BACKEND", "STATUS"
        ))
    );
    for row in rows {
        let backend_str = if row.backends.is_empty() {
            "-".to_string()
        } else {
            row.backends.join(",")
        };
        println!(
            "{:<24} {:<16} {:<10} {:<12} {}",
            row.name,
            row.category,
            row.version,
            backend_str,
            color.status(pad_right(&row.status, 14)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_core::state::InstalledObject;

    fn sample_catalog_json() -> &'static str {
        r#"{
            "schema_version": 1,
            "generated_at": "2026-06-11T00:00:00Z",
            "channel": "stable",
            "components": [
                {
                    "name": "agentsight",
                    "display_name": "AgentSight",
                    "summary": "Agent behavior tracing and token attribution",
                    "category": "observability",
                    "version": "0.1.4",
                    "status": "available",
                    "backends": [
                        {"type": "oss", "url": "https://example.com/agentsight.tar.gz", "sha256": "abc"},
                        {"type": "rpm", "repo_url": "https://repo.example.com", "package": "anolisa-agentsight"}
                    ],
                    "platforms": [{"os": "linux", "arch": "x86_64", "distros": ["alinux3"]}],
                    "tags": ["agent", "trace"]
                },
                {
                    "name": "tokenless",
                    "display_name": "Tokenless",
                    "summary": "Token compression runtime",
                    "category": "runtime",
                    "version": "0.2.0",
                    "status": "deprecated",
                    "backends": [{"type": "oss", "url": "https://example.com/tokenless.tar.gz"}],
                    "tags": ["token"]
                }
            ]
        }"#
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
    fn parse_v1_catalog_builds_rows() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("valid v1 JSON");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = empty_state();
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        assert_eq!(rows.len(), 2);

        let sight = &rows[0];
        assert_eq!(sight.name, "agentsight");
        assert_eq!(sight.display_name, "AgentSight");
        assert_eq!(
            sight.summary,
            "Agent behavior tracing and token attribution"
        );
        assert_eq!(sight.category, "observability");
        assert_eq!(sight.version, "0.1.4");
        assert_eq!(sight.backends, vec!["oss", "rpm"]);
        assert_eq!(sight.status, "not_installed");
        assert!(sight.available);

        let token = &rows[1];
        assert_eq!(token.name, "tokenless");
        assert_eq!(token.backends, vec!["oss"]);
        assert!(!token.available);
    }

    #[test]
    fn schema_version_mismatch_errors() {
        let json = r#"{"schema_version": 2, "components": []}"#;
        let err = parse_catalog(json.as_bytes()).expect_err("must reject schema v2");
        assert!(err.reason().contains("schema_version 2"));
    }

    #[test]
    fn available_filter_keeps_only_available() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: true,
            enabled: false,
        };
        let state = empty_state();
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "agentsight");
        assert!(rows[0].available);
    }

    #[test]
    fn empty_state_all_not_installed() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = empty_state();
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        for row in &rows {
            assert_eq!(row.status, "not_installed");
        }
    }

    #[test]
    fn installed_component_shows_installed() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = state_with_object(ObjectKind::Component, "tokenless", ObjectStatus::Installed);
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");

        let sight = rows.iter().find(|r| r.name == "agentsight").unwrap();
        assert_eq!(sight.status, "not_installed");

        let token = rows.iter().find(|r| r.name == "tokenless").unwrap();
        assert_eq!(token.status, "installed");
    }

    #[test]
    fn adapter_object_does_not_mark_component_installed() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = state_with_object(ObjectKind::Adapter, "tokenless", ObjectStatus::Installed);
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        let token = rows.iter().find(|r| r.name == "tokenless").unwrap();
        assert_eq!(token.status, "not_installed");
    }

    #[test]
    fn failed_component_shows_not_installed() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = state_with_object(ObjectKind::Component, "tokenless", ObjectStatus::Failed);
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        let token = rows.iter().find(|r| r.name == "tokenless").unwrap();
        assert_eq!(token.status, "not_installed");
    }

    #[test]
    fn disabled_component_shows_not_installed() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = state_with_object(ObjectKind::Component, "tokenless", ObjectStatus::Disabled);
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        let token = rows.iter().find(|r| r.name == "tokenless").unwrap();
        assert_eq!(token.status, "not_installed");
    }

    #[test]
    fn enabled_returns_only_installed() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: true,
        };
        let state = state_with_object(ObjectKind::Component, "tokenless", ObjectStatus::Installed);
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "tokenless");
        assert_eq!(rows[0].status, "installed");
    }

    #[test]
    fn enabled_with_empty_state_returns_empty() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: true,
        };
        let state = empty_state();
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        assert!(rows.is_empty());
    }

    #[test]
    fn available_and_enabled_returns_intersection() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: true,
            enabled: true,
        };
        // tokenless is installed but not available; agentsight is available but not installed
        let state = state_with_object(ObjectKind::Component, "tokenless", ObjectStatus::Installed);
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        assert!(rows.is_empty());

        // agentsight is both available and installed
        let state = state_with_object(ObjectKind::Component, "agentsight", ObjectStatus::Installed);
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "agentsight");
    }

    #[test]
    fn json_payload_uses_components_key() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = empty_state();
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        let payload = ListPayload { components: rows };
        let json_str = serde_json::to_string(&payload).expect("serialize");
        let val: serde_json::Value = serde_json::from_str(&json_str).expect("reparse");
        assert!(val.get("components").is_some());
        assert!(val.get("capabilities").is_none());
    }

    #[test]
    fn json_payload_status_reflects_install_state() {
        let catalog = parse_catalog(sample_catalog_json().as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = state_with_object(ObjectKind::Component, "agentsight", ObjectStatus::Installed);
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
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
    fn empty_name_rejected() {
        let json = r#"{"schema_version": 1, "components": [{"name": ""}]}"#;
        let err = parse_catalog(json.as_bytes()).expect_err("must reject empty name");
        assert!(err.reason().contains("empty name"));
    }

    #[test]
    fn unknown_backend_type_preserved() {
        let json = r#"{
            "schema_version": 1,
            "components": [{
                "name": "test",
                "backends": [{"type": "custom-repo"}]
            }]
        }"#;
        let catalog = parse_catalog(json.as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = empty_state();
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        assert_eq!(rows[0].backends, vec!["custom-repo"]);
    }

    #[test]
    fn missing_optional_fields_use_defaults() {
        let json = r#"{"schema_version": 1, "components": [{"name": "minimal"}]}"#;
        let catalog = parse_catalog(json.as_bytes()).expect("parse");
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let state = empty_state();
        let rows = build_rows(&catalog, &args, &state).expect("build_rows");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.name, "minimal");
        assert_eq!(row.display_name, "minimal");
        assert!(row.summary.is_empty());
        assert!(row.category.is_empty());
        assert!(row.version.is_empty());
        assert!(row.backends.is_empty());
        assert_eq!(row.status, "not_installed");
        assert!(!row.available);
    }
}
