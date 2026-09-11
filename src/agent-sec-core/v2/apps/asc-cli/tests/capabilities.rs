//! Public-surface tests for the environment-only capability view.
//!
//! These drive the same entry points `main` uses, so they cover the argv
//! contract and the rendered projections rather than the resolution internals.

use asc_cli::capabilities::{Environment, query, render_json, render_table};
use asc_cli::{Cli, Plan};

const AGENTS: [&str; 6] = ["qoder", "qwen", "codex", "cosh", "openclaw", "hermes"];
const CAPABILITIES: [&str; 5] = [
    "code-scan",
    "prompt-scan",
    "pii-check",
    "skill-ledger",
    "observability",
];

fn environment(pairs: &[(&str, &str)]) -> Environment {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn the_default_view_covers_every_agent_capability_pair_in_stable_order() {
    let records = query(&environment(&[]), None, None).expect("no filters");
    let payload: serde_json::Value =
        serde_json::from_str(&render_json(&records).expect("serializes")).expect("valid JSON");
    let pairs: Vec<(String, String)> = payload
        .as_array()
        .expect("array")
        .iter()
        .map(|record| {
            (
                record["agent"].as_str().expect("agent").to_owned(),
                record["capability"]
                    .as_str()
                    .expect("capability")
                    .to_owned(),
            )
        })
        .collect();
    let mut expected: Vec<(String, String)> = AGENTS
        .iter()
        .flat_map(|agent| {
            CAPABILITIES
                .iter()
                .map(move |capability| ((*agent).to_owned(), (*capability).to_owned()))
        })
        .collect();
    expected.sort_unstable();
    assert_eq!(pairs, expected);
}

#[test]
fn every_pair_can_be_selected_individually() {
    for agent in AGENTS {
        for capability in CAPABILITIES {
            let records =
                query(&environment(&[]), Some(agent), Some(capability)).expect("valid filters");
            assert_eq!(records.len(), 1, "{agent}/{capability}");
            let rendered = render_table(&records);
            assert!(rendered.starts_with(&format!("[{agent}]")), "{rendered}");
            assert!(rendered.contains(capability), "{rendered}");
        }
    }
}

#[test]
fn no_output_ever_carries_a_raw_environment_value() {
    let sensitive = "invalid-token-like-value";
    let env = environment(&[
        ("PROMPT_SCANNER_MODE", sensitive),
        ("CODE_SCANNER_MODE", sensitive),
        ("PII_CHECKER_TIMEOUT", sensitive),
    ]);
    let records = query(&env, None, None).expect("no filters");
    let json = render_json(&records).expect("serializes");
    let table = render_table(&records);
    assert!(!json.contains(sensitive), "{json}");
    assert!(!json.contains("\"raw\""), "{json}");
    assert!(!table.contains(sensitive), "{table}");
    assert!(json.contains("has an invalid value"), "{json}");
}

#[test]
fn the_view_is_parsed_as_a_local_plan_regardless_of_the_socket_option() {
    for argv in [
        vec!["agent-sec-cli", "capabilities"],
        vec!["agent-sec-cli", "capabilities", "--output", "json"],
        // A socket may be inherited from the environment or passed explicitly;
        // neither turns the view into a daemon call.
        vec![
            "agent-sec-cli",
            "--socket",
            "/run/agent-sec-core/daemon.sock",
            "capabilities",
        ],
    ] {
        let cli = Cli::parse_from(argv.clone()).expect("capabilities parses");
        assert!(
            matches!(cli.plan(), Plan::Local(_)),
            "{argv:?} must render locally"
        );
    }
}

#[test]
fn unknown_filter_values_are_reported_without_echoing_control_characters() {
    let error =
        query(&environment(&[]), Some("qoder\u{1}\u{7f}"), None).expect_err("unknown agent");
    let message = error.to_string();
    assert!(message.contains("qoder\\x01\\x7f"), "{message}");
    assert!(!message.contains('\u{1}'), "{message}");
    assert!(
        message.ends_with("Allowed values: qoder, qwen, codex, cosh, openclaw, hermes"),
        "{message}"
    );
}
