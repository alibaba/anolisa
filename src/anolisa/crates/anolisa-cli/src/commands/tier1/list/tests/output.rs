use anolisa_core::state::InstalledState;

use crate::commands::tier1::list::render::human_header;
use crate::commands::tier1::list::{ListArgs, ListPayload, Row, build_rows};

use super::support::{FakeRpmQuery, pkg_info, sample_index};

#[test]
fn row_json_retains_status_and_adds_local_fields() {
    let index = sample_index();
    let args = ListArgs { installed: false };
    let state = InstalledState::default();
    let query = FakeRpmQuery {
        installed: vec![(
            "agentsight".to_string(),
            pkg_info("agentsight", "1.2.3", Some("1.al8"), "x86_64"),
        )],
        command_missing: false,
        what_provides: Vec::new(),
    };
    let rows = build_rows(&index, &args, &state, Some(&query));
    let payload = ListPayload { components: rows };

    let json_str = serde_json::to_string(&payload).expect("serialize");
    let val: serde_json::Value = serde_json::from_str(&json_str).expect("reparse");
    let sight = val["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["name"] == "agentsight")
        .unwrap();

    assert_eq!(sight["status"], "not_installed");
    assert_eq!(sight["local_state"], "observed");
    assert_eq!(sight["ownership"], "rpm");
    assert_eq!(sight["action"], "install");
}

#[test]
fn human_header_contains_local_state() {
    let rows = vec![Row {
        name: "agentsight".to_string(),
        display_name: "AgentSight".to_string(),
        summary: "observability".to_string(),
        backends: vec!["rpm".to_string()],
        status: "not_installed".to_string(),
        local_state: "observed".to_string(),
        ownership: "none".to_string(),
        action: "install".to_string(),
        rpm_package: Some("agentsight".to_string()),
        rpm_evr: Some("1.2.3-1.al8".to_string()),
        rpm_arch: Some("x86_64".to_string()),
        rpm_source_repo: Some("@System".to_string()),
    }];

    assert!(human_header(&rows).contains("LOCAL STATE"));
}

#[test]
fn list_clap_surface_has_no_catalog_or_tracked_args() {
    use crate::commands::Cli;
    use clap::CommandFactory;

    let help = Cli::command().render_long_help().to_string();

    assert!(!help.contains("--catalog"));
    assert!(!help.contains("--tracked"));
}
