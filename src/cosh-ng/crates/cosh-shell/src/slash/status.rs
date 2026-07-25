use crate::runtime::prelude::*;
use crate::slash::panel::render_notice_panel;

pub(super) fn render_status_command<W: Write>(
    adapter: &AdapterInstance,
    _state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let provider = adapter.name();
    let invocation = adapter
        .provider_invocation()
        .unwrap_or_else(|| "<none>".to_string());
    let session_id = adapter
        .committed_session_id()
        .unwrap_or_else(|| "<none>".to_string());
    render_notice_panel(
        output,
        "Status",
        vec![
            format!("Version: cosh-shell {}", version),
            format!("Provider: {}", provider),
            format!("Invocation: {}", invocation),
            format!("Session: {}", session_id),
        ],
        None,
    )
}

pub(super) fn render_about_command<W: Write>(output: &mut W) -> std::io::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    render_notice_panel(
        output,
        "About",
        vec![
            format!("cosh-shell {}", version),
            "Part of cosh-ng".to_string(),
            "Repository: https://github.com/alibaba/anolisa".to_string(),
        ],
        None,
    )
}

pub(super) fn render_stats_command<W: Write>(
    adapter: &AdapterInstance,
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let command_count = state.session_blocks.len();
    let invocation = adapter
        .provider_invocation()
        .unwrap_or_else(|| "<none>".to_string());
    let mut body = vec![
        format!("Session commands: {}", command_count),
        format!("Provider invocation: {}", invocation),
    ];
    if command_count == 0 {
        body.push("No commands run yet.".to_string());
    }
    render_notice_panel(output, "Stats", body, None)
}
