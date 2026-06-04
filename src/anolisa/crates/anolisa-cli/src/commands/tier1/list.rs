//! `anolisa list` — enumerate capabilities from the bundled `Catalog`,
//! overlay [`InstalledState`], and render.
//!
//! P1-E1 wiring: this is the first read-only Tier-1 command beyond `logs`
//! to leave the `NOT_IMPLEMENTED` placeholder behind. Behavior:
//!
//! 1. Load the bundled catalog via [`crate::commands::common::load_bundled_catalog`].
//! 2. Load `installed.toml` via [`crate::commands::common::load_installed_state`]
//!    (missing file ⇒ `Default`, fresh-install case).
//! 3. For every capability, project a [`Row`] carrying name, summary, priority,
//!    real status string, installed version (if any), and `available`.
//!    - `status` is the same `installed | degraded | disabled | failed |
//!      adopted | not_installed` vocabulary used by `status` (shared via
//!      [`crate::commands::common::object_status_str`]). The previous
//!      `installed: bool` shape conflated `Disabled` / `Failed` / `Adopted`
//!      with `Installed`, which is wrong.
//!    - `available` is hard-coded to `true` for E1 — env-fact gating lands with
//!      the capability resolver / `EnvFacts` wiring.
//! 4. Apply `--enabled` (only actively-serving rows: `installed | degraded |
//!    adopted` — `disabled` and `failed` are excluded) and `--available`
//!    (no-op for now; flag is still honored so a future env-facts pass
//!    changes only the predicate). Both flags ⇒ intersection.
//! 5. Render JSON envelope via [`render_json`] when `ctx.json`, else a
//!    plain table on stdout (suppressed under `ctx.quiet`).
//!
//! Schema-field decisions:
//! - `summary` ← `capability.description` (no dedicated `summary` field
//!   exists in the manifest v2 schema).
//! - `priority` ← `capability.stability` (no `priority` field exists; we
//!   surface the stability tag — "stable" by default — until the manifest
//!   schema grows a real priority axis).

use clap::Parser;
use serde::Serialize;

use anolisa_core::{Catalog, InstalledState, ObjectKind};

use crate::color::{Palette, pad_right};
use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "list";

#[derive(Parser)]
pub struct ListArgs {
    /// Show only capabilities available on this machine
    #[arg(long)]
    pub available: bool,
    /// Show only currently enabled (installed) capabilities
    #[arg(long)]
    pub enabled: bool,
}

/// Projection of a `Catalog` capability + `InstalledState` overlay. Kept
/// `Serialize` so the same struct feeds the `--json` envelope and the
/// human renderer.
///
/// `status` is the authoritative install state (`installed | degraded |
/// disabled | failed | adopted | not_installed`) — JSON consumers should
/// branch on this rather than re-deriving a boolean.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub summary: String,
    pub priority: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    pub available: bool,
}

#[derive(Serialize)]
struct ListPayload {
    capabilities: Vec<Row>,
}

pub fn handle(args: ListArgs, ctx: &CliContext) -> Result<(), CliError> {
    let catalog = common::load_bundled_catalog(ctx, COMMAND)?;
    let state = common::load_installed_state(ctx, COMMAND)?;
    let rows = build_rows(&catalog, &state, &args);

    if ctx.json {
        return render_json(COMMAND, ListPayload { capabilities: rows });
    }

    if !ctx.quiet {
        render_human(&rows, ctx.verbose, ctx.no_color);
    }
    Ok(())
}

/// Pure helper: combine catalog + installed state + filter flags into a
/// row list. Lives outside `handle` so unit tests can exercise it
/// without mocking [`CliContext`].
pub(crate) fn build_rows(catalog: &Catalog, state: &InstalledState, args: &ListArgs) -> Vec<Row> {
    catalog
        .list_capabilities()
        .into_iter()
        .map(|cap| {
            let installed_obj = state.find_object(ObjectKind::Capability, &cap.capability.name);
            let status = installed_obj
                .map(|o| common::object_status_str(o.status).to_string())
                .unwrap_or_else(|| "not_installed".to_string());
            let installed_version = installed_obj.map(|o| o.version.clone());
            // TODO(owner: cli-planning, when: EnvFacts resolver is wired):
            // report env-fact-based availability. For E1 every capability is
            // reported as available so CLI plumbing stays exercisable.
            let available = true;
            Row {
                name: cap.capability.name.clone(),
                summary: cap.capability.description.clone(),
                priority: cap.capability.stability.clone(),
                status,
                installed_version,
                available,
            }
        })
        .filter(|row| !args.enabled || common::status_is_enabled(&row.status))
        .filter(|row| !args.available || row.available)
        .collect()
}

fn render_human(rows: &[Row], verbose: bool, no_color: bool) {
    let color = Palette::new(no_color);
    println!(
        "{}",
        color.header(format!(
            "{:<28} {:<10} {:<14} {}",
            "NAME", "PRIORITY", "STATUS", "VERSION"
        ))
    );
    for row in rows {
        let version = row.installed_version.as_deref().unwrap_or("-");
        println!(
            "{:<28} {:<10} {:<14} {}",
            row.name,
            row.priority,
            color.status(pad_right(&row.status, 14)),
            version,
        );
        if verbose && !row.summary.is_empty() {
            println!("    {}", color.muted(&row.summary));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use anolisa_core::manifest::{CapabilityManifest, CapabilityMeta, EnvRequirements};
    use anolisa_core::{
        Catalog, CatalogLayers, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
        SubscriptionScope,
    };
    use std::collections::BTreeMap;

    fn make_cap(name: &str, description: &str) -> CapabilityManifest {
        CapabilityManifest {
            schema_version: 2,
            capability: CapabilityMeta {
                name: name.to_string(),
                description: description.to_string(),
                layer: "tier1-capability".to_string(),
                stability: "stable".to_string(),
            },
            components: Vec::new(),
            default_features: Vec::new(),
            env_requirements: EnvRequirements::default(),
        }
    }

    fn make_catalog(caps: Vec<CapabilityManifest>) -> Catalog {
        let mut capabilities = BTreeMap::new();
        for cap in caps {
            capabilities.insert(cap.capability.name.clone(), cap);
        }
        Catalog {
            capabilities,
            components: BTreeMap::new(),
            layers: CatalogLayers::bundled_only(PathBuf::from("/dev/null")),
        }
    }

    fn make_installed(name: &str, version: &str) -> InstalledObject {
        make_object(name, version, ObjectStatus::Installed)
    }

    fn make_object(name: &str, version: &str, status: ObjectStatus) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Capability,
            name: name.to_string(),
            version: version.to_string(),
            status,
            manifest_digest: None,
            distribution_source: None,
            installed_at: "2026-06-01T00:00:00Z".to_string(),
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: SubscriptionScope::None,
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        }
    }

    #[test]
    fn empty_catalog_yields_no_rows() {
        let catalog = make_catalog(Vec::new());
        let state = InstalledState::default();
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let rows = build_rows(&catalog, &state, &args);
        assert!(rows.is_empty());
    }

    #[test]
    fn empty_state_marks_every_capability_not_installed() {
        let catalog = make_catalog(vec![
            make_cap("agent-observability", "Agent behavior tracing"),
            make_cap("tokenless", "Token compression"),
        ]);
        let state = InstalledState::default();
        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let rows = build_rows(&catalog, &state, &args);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.status, "not_installed");
            assert!(row.installed_version.is_none());
            assert!(row.available);
        }
        // Catalog is a BTreeMap keyed by name, so order is alphabetical.
        assert_eq!(rows[0].name, "agent-observability");
        assert_eq!(rows[1].name, "tokenless");
    }

    #[test]
    fn installed_state_populates_version_for_matching_capability() {
        let catalog = make_catalog(vec![
            make_cap("agent-observability", "Agent behavior tracing"),
            make_cap("tokenless", "Token compression"),
        ]);
        let mut state = InstalledState::default();
        state.upsert_object(make_installed("agent-observability", "0.1.0"));

        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let rows = build_rows(&catalog, &state, &args);
        assert_eq!(rows.len(), 2);
        let cap = rows
            .iter()
            .find(|r| r.name == "agent-observability")
            .unwrap();
        assert_eq!(cap.status, "installed");
        assert_eq!(cap.installed_version.as_deref(), Some("0.1.0"));
        let other = rows.iter().find(|r| r.name == "tokenless").unwrap();
        assert_eq!(other.status, "not_installed");
        assert!(other.installed_version.is_none());
    }

    #[test]
    fn enabled_filter_keeps_only_installed_rows() {
        let catalog = make_catalog(vec![
            make_cap("agent-observability", "Agent behavior tracing"),
            make_cap("tokenless", "Token compression"),
        ]);
        let mut state = InstalledState::default();
        state.upsert_object(make_installed("agent-observability", "0.1.0"));

        let args = ListArgs {
            available: false,
            enabled: true,
        };
        let rows = build_rows(&catalog, &state, &args);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "agent-observability");
        assert_eq!(rows[0].status, "installed");
    }

    /// `--enabled` must exclude `Disabled` and `Failed` even though those
    /// objects still exist in `InstalledState`. Regression for the bug
    /// where `installed = installed_obj.is_some()` flagged every state row
    /// as "installed" regardless of lifecycle.
    #[test]
    fn enabled_filter_excludes_disabled_and_failed() {
        let catalog = make_catalog(vec![
            make_cap("agent-observability", "tracing"),
            make_cap("tokenless", "compression"),
            make_cap("ws-ckpt", "checkpoint"),
            make_cap("sandbox", "isolation"),
        ]);
        let mut state = InstalledState::default();
        state.upsert_object(make_object(
            "agent-observability",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        state.upsert_object(make_object("tokenless", "0.2.0", ObjectStatus::Disabled));
        state.upsert_object(make_object("ws-ckpt", "0.3.0", ObjectStatus::Failed));
        state.upsert_object(make_object("sandbox", "0.4.0", ObjectStatus::Adopted));

        let args = ListArgs {
            available: false,
            enabled: true,
        };
        let rows = build_rows(&catalog, &state, &args);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"agent-observability"));
        assert!(names.contains(&"sandbox"));
        assert!(
            !names.contains(&"tokenless"),
            "disabled must not pass --enabled"
        );
        assert!(
            !names.contains(&"ws-ckpt"),
            "failed must not pass --enabled"
        );
        assert_eq!(rows.len(), 2);
    }

    /// Status strings must round-trip the full ObjectStatus vocabulary so
    /// human + JSON consumers can branch on the real state, not on a
    /// collapsed boolean.
    #[test]
    fn status_string_matches_object_status() {
        let catalog = make_catalog(vec![
            make_cap("a", ""),
            make_cap("b", ""),
            make_cap("c", ""),
            make_cap("d", ""),
            make_cap("e", ""),
        ]);
        let mut state = InstalledState::default();
        state.upsert_object(make_object("a", "1", ObjectStatus::Installed));
        state.upsert_object(make_object("b", "1", ObjectStatus::Partial));
        state.upsert_object(make_object("c", "1", ObjectStatus::Disabled));
        state.upsert_object(make_object("d", "1", ObjectStatus::Failed));
        state.upsert_object(make_object("e", "1", ObjectStatus::Adopted));

        let args = ListArgs {
            available: false,
            enabled: false,
        };
        let rows = build_rows(&catalog, &state, &args);
        let by_name = |n: &str| {
            rows.iter()
                .find(|r| r.name == n)
                .map(|r| r.status.as_str())
                .unwrap()
        };
        assert_eq!(by_name("a"), "installed");
        assert_eq!(by_name("b"), "degraded");
        assert_eq!(by_name("c"), "disabled");
        assert_eq!(by_name("d"), "failed");
        assert_eq!(by_name("e"), "adopted");
    }

    #[test]
    fn available_and_enabled_intersect() {
        let catalog = make_catalog(vec![
            make_cap("agent-observability", "Agent behavior tracing"),
            make_cap("tokenless", "Token compression"),
        ]);
        let mut state = InstalledState::default();
        state.upsert_object(make_installed("tokenless", "0.2.0"));

        let args = ListArgs {
            available: true,
            enabled: true,
        };
        let rows = build_rows(&catalog, &state, &args);
        // available is always true today, so the intersection is just the
        // installed set — but the flag must still be honored.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "tokenless");
        assert_eq!(rows[0].status, "installed");
        assert!(rows[0].available);
    }
}
