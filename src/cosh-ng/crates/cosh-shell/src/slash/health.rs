//! `/health` slash command: render the shared on-demand doctor report as an
//! inline card. Uses the same [`run_doctor_report`] engine and status model as
//! the `cosh-shell doctor` CLI, so both entry points report identical checks.

use crate::diagnostics::doctor::run_doctor_report;
use crate::runtime::prelude::*;

pub(crate) fn render_health_command<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let config = load_config();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let report = run_doctor_report(&config, &cwd);

    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
    renderer.write_health_banner(output, HealthBannerModel { report: &report })?;
    output.flush()
}
