use super::*;
use activity_runtime::render_activity_details_by_id;
use approval_runtime::{render_approval_details, render_approval_journal};

pub(super) fn render_runtime_details<W: Write>(
    state: &InlineState,
    id: &str,
    output: &mut W,
) -> std::io::Result<()> {
    if id == "approvals" {
        return render_approval_journal(state, output);
    }

    if let Some(request) = state
        .approval_requests
        .iter()
        .find(|request| request.id == id)
    {
        return render_approval_details(request, output);
    }

    if let Some(result) = render_activity_details_by_id(state, id, output) {
        return result;
    }

    RatatuiInlineRenderer::for_terminal().write_notice(
        output,
        "Details unavailable",
        vec![format!(
            "{id} is not available; use /details with an approval or activity id"
        )],
        None,
    )
}
