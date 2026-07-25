//! `/stats` slash command: display session usage statistics.
//! Shows session information based on the current session state.
//! Sub-commands `model` and `tools` are accepted for forward
//! compatibility but currently show a placeholder notice.

use crate::runtime::prelude::*;

pub(crate) fn render_stats_command<W: Write>(
    sub: Option<&str>,
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let i18n = state.i18n();

    let mut body = Vec::new();

    // Show session block count as a basic usage metric.
    let block_count = state.session_blocks.len();
    if block_count > 0 {
        body.push(format!("session commands: {block_count}"));
    } else {
        body.push(i18n.t(MessageId::SlashStatsNoSessionBody).to_string());
    }

    match sub.unwrap_or("") {
        "model" => body.push("model usage tracking is not yet implemented.".to_string()),
        "tools" => body.push("tool usage tracking is not yet implemented.".to_string()),
        _ => {}
    }

    super::panel::render_notice_panel(
        output,
        i18n.t(MessageId::SlashStatsTitle),
        body,
        Some(i18n.t(MessageId::SlashStatsFooter)),
    )
}
