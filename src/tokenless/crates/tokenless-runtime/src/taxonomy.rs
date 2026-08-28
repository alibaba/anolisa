//! Truncation thresholds for the unified entry point (roadmap §5.4).
//!
//! Thresholds come from the common Python hooks (`hook_utils.py`), sourced
//! from the same `adapters/tokenless/common/hooks/tool_categories.json` so
//! the JSON file stays the single source of truth (OpenClaw still reads it
//! at runtime).
//! The file is embedded at compile time — the binary must work without
//! adapter files installed — which ties this crate to the workspace layout;
//! acceptable for a workspace-internal crate that is never `cargo package`d.
//!
//! A malformed edit of the JSON falls back to the hardcoded sets below
//! (mirroring the Python fail-open behaviour), and the unit tests fail so
//! CI reports the breakage instead of silently degrading.
//!
//! The layer-1 skip list is gone from here (roadmap §6.3): whether content
//! is compressed follows from its type, its origin, and whether a compressor
//! is registered. `tool_categories.json` keeps the list for the adapters,
//! which use it to avoid a spawn; this crate no longer reads it.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::Deserialize;
use tokenless_protocol::ContentOrigin;

static TAXONOMY_JSON: &str =
    include_str!("../../../adapters/tokenless/common/hooks/tool_categories.json");

/// Per-tool truncation limits handed to the response compressor
/// (layer 2 shell vs layer 3 API in `tool_categories.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolThresholds {
    pub truncate_strings_at: usize,
    pub truncate_arrays_at: usize,
    pub max_depth: usize,
}

const SHELL_THRESHOLDS: ToolThresholds = ToolThresholds {
    truncate_strings_at: 65_536,
    truncate_arrays_at: 128,
    max_depth: 8,
};

const API_THRESHOLDS: ToolThresholds = ToolThresholds {
    truncate_strings_at: 1_048_576,
    truncate_arrays_at: 65_536,
    max_depth: 32,
};

// Minimum safe classification, matching `hook_utils.py`'s fallback set.
// Only `ContentOrigin::Unspecified` still reaches it; it goes when every
// adapter declares an origin (roadmap §6.3, item 10).
const FALLBACK_SHELL_TOOLS: &[&str] = &[
    "Bash",
    "bash",
    "Shell",
    "shell",
    "exec",
    "terminal",
    "run_shell_command",
    "run_in_terminal",
    "get_terminal_output",
    "execute_command",
    "process",
];

#[derive(Deserialize)]
struct RawTaxonomy {
    layer_2_shell: RawLayer,
    layer_3_api: RawLayer,
}

#[derive(Deserialize)]
struct RawLayer {
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    thresholds: Option<RawThresholds>,
}

#[derive(Deserialize)]
struct RawThresholds {
    truncate_strings_at: Option<usize>,
    truncate_arrays_at: Option<usize>,
    max_depth: Option<usize>,
}

impl RawThresholds {
    fn resolve(raw: Option<Self>, defaults: ToolThresholds) -> ToolThresholds {
        let raw = match raw {
            Some(raw) => raw,
            None => return defaults,
        };
        ToolThresholds {
            truncate_strings_at: raw
                .truncate_strings_at
                .unwrap_or(defaults.truncate_strings_at),
            truncate_arrays_at: raw
                .truncate_arrays_at
                .unwrap_or(defaults.truncate_arrays_at),
            max_depth: raw.max_depth.unwrap_or(defaults.max_depth),
        }
    }
}

struct Taxonomy {
    shell_tools: HashSet<String>,
    shell_thresholds: ToolThresholds,
    api_thresholds: ToolThresholds,
}

fn fallback_taxonomy() -> Taxonomy {
    Taxonomy {
        shell_tools: FALLBACK_SHELL_TOOLS.iter().map(|t| (*t).into()).collect(),
        shell_thresholds: SHELL_THRESHOLDS,
        api_thresholds: API_THRESHOLDS,
    }
}

fn taxonomy() -> &'static Taxonomy {
    static TAXONOMY: OnceLock<Taxonomy> = OnceLock::new();
    TAXONOMY.get_or_init(
        || match serde_json::from_str::<RawTaxonomy>(TAXONOMY_JSON) {
            Ok(raw) if !raw.layer_2_shell.tools.is_empty() => Taxonomy {
                shell_tools: raw.layer_2_shell.tools.into_iter().collect(),
                shell_thresholds: RawThresholds::resolve(
                    raw.layer_2_shell.thresholds,
                    SHELL_THRESHOLDS,
                ),
                api_thresholds: RawThresholds::resolve(raw.layer_3_api.thresholds, API_THRESHOLDS),
            },
            _ => fallback_taxonomy(),
        },
    )
}

/// Truncation thresholds: layer 2 limits for command output, layer 3
/// zero-truncation limits for everything else.
///
/// A declared origin decides on its own. `Unspecified` falls back to the
/// tool-name test, because the two sets differ by more than an order of
/// magnitude — handing an unmigrated caller the API set would silently
/// change the shape of its shell output (roadmap §4.3).
pub(crate) fn thresholds_for(origin: ContentOrigin, tool_name: Option<&str>) -> ToolThresholds {
    let tax = taxonomy();
    match origin {
        ContentOrigin::CommandOutput => tax.shell_thresholds,
        ContentOrigin::FileContent | ContentOrigin::ApiResponse => tax.api_thresholds,
        ContentOrigin::Unspecified => match tool_name {
            Some(name) if tax.shell_tools.contains(name) => tax.shell_thresholds,
            _ => tax.api_thresholds,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Drift guard for the embedded JSON: a malformed edit must fail here,
    // not silently fall back at runtime.
    #[test]
    fn embedded_taxonomy_parses_with_expected_layers() {
        let raw: RawTaxonomy = serde_json::from_str(TAXONOMY_JSON).expect("embedded JSON parses");
        assert!(raw.layer_2_shell.tools.contains(&"Bash".to_string()));
        assert!(raw.layer_2_shell.thresholds.is_some());
        assert!(raw.layer_3_api.thresholds.is_some());
    }

    #[test]
    fn unspecified_origin_still_classifies_by_tool_name() {
        use ContentOrigin::Unspecified;
        assert_eq!(thresholds_for(Unspecified, Some("Bash")), SHELL_THRESHOLDS);
        assert_eq!(
            thresholds_for(Unspecified, Some("run_shell_command")),
            SHELL_THRESHOLDS
        );
        assert_eq!(
            thresholds_for(Unspecified, Some("WebFetch")),
            API_THRESHOLDS
        );
        assert_eq!(thresholds_for(Unspecified, None), API_THRESHOLDS);
    }

    #[test]
    fn a_declared_origin_ignores_the_tool_name() {
        // Both directions: the origin decides even when the tool name would
        // have said otherwise.
        assert_eq!(
            thresholds_for(ContentOrigin::CommandOutput, Some("WebFetch")),
            SHELL_THRESHOLDS
        );
        assert_eq!(
            thresholds_for(ContentOrigin::ApiResponse, Some("Bash")),
            API_THRESHOLDS
        );
        assert_eq!(
            thresholds_for(ContentOrigin::FileContent, Some("Bash")),
            API_THRESHOLDS
        );
    }

    #[test]
    fn fallback_matches_the_embedded_values() {
        // The hardcoded fallback must stay in sync with the JSON defaults,
        // so a fallback activation changes availability, never behaviour.
        let fallback = fallback_taxonomy();
        let parsed = taxonomy();
        assert_eq!(fallback.shell_tools, parsed.shell_tools);
        assert_eq!(fallback.shell_thresholds, parsed.shell_thresholds);
        assert_eq!(fallback.api_thresholds, parsed.api_thresholds);
    }
}
