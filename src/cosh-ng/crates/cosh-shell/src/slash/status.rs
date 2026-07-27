//! `/status` slash command: display current version, backend adapter, and
//! provider information so users can quickly verify their runtime configuration.

use crate::runtime::prelude::*;

pub(crate) fn render_status_command<W: Write>(
    adapter: &AdapterInstance,
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let adapter_name = adapter.name();
    let approval_mode = match state.approval_mode {
        CoshApprovalMode::Auto => "auto",
        CoshApprovalMode::Trust => "trust",
        CoshApprovalMode::Recommend => "recommend",
    };
    let analysis_mode = match state.analysis_mode {
        AnalysisMode::Smart => "smart",
        AnalysisMode::Auto => "auto",
        AnalysisMode::Manual => "manual",
    };

    writeln!(output, "  cosh-shell  {version}")?;
    writeln!(output, "  Backend     {adapter_name}")?;
    writeln!(output, "  Approval    {approval_mode}")?;
    writeln!(output, "  Analysis    {analysis_mode}")?;
    output.flush()
}
