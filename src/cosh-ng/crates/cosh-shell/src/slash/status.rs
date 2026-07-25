//! `/status` and `/about` slash commands: display version, backend, shell,
//! approval/analysis mode, language, and cwd information as an inline panel.
//! `/about` is an alias that renders the same view.

use crate::runtime::prelude::*;

pub(crate) fn render_status_command<W: Write>(
    adapter: &AdapterInstance,
    state: &InlineState,
    shell_cwd: Option<&str>,
    output: &mut W,
) -> std::io::Result<()> {
    let config = load_config();
    let i18n = state.i18n();

    let version = env!("CARGO_PKG_VERSION");
    let cwd = shell_cwd.unwrap_or_else(|| {
        // Provide a fallback; the caller will own the buffer.
        "<unknown>"
    });

    let body = vec![
        i18n.format(MessageId::SlashStatusVersionLine, &[("version", version)]),
        i18n.format(
            MessageId::SlashStatusAdapterLine,
            &[("adapter", adapter.name())],
        ),
        i18n.format(
            MessageId::SlashStatusShellLine,
            &[("shell", &config.shell_default)],
        ),
        i18n.format(
            MessageId::SlashStatusApprovalLine,
            &[("mode", state.approval_mode.label())],
        ),
        i18n.format(
            MessageId::SlashStatusAnalysisLine,
            &[("mode", state.analysis_mode.label())],
        ),
        i18n.format(
            MessageId::SlashStatusLanguageLine,
            &[("language", state.language.as_config_value())],
        ),
        i18n.format(MessageId::SlashStatusCwdLine, &[("cwd", cwd)]),
    ];

    super::panel::render_notice_panel(
        output,
        i18n.t(MessageId::SlashStatusTitle),
        body,
        Some(i18n.t(MessageId::SlashStatusFooter)),
    )
}
