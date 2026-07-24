//! Contract tests for the `/hooks` status panel, which shares the
//! reference-panel visual contract with `/help`.

use super::{
    strip_ansi_escape, AgentHooksView, HookEntryView, HookEventGroup, HookStatusPanelModel,
    RatatuiInlineRenderer,
};

fn sample_model() -> HookStatusPanelModel<'static> {
    HookStatusPanelModel {
        title: "Hook status",
        shell_label: "Shell Hooks",
        shell_lines: vec![
            "Registered: 2; enabled: 2; disabled: 0.".to_string(),
            "Sources: builtin=2; user=0; project=0.".to_string(),
        ],
        agent_label: "Agent Hooks",
        agent: AgentHooksView::Groups(vec![
            HookEventGroup {
                event: "PreToolUse".to_string(),
                hooks: vec![
                    HookEntryView {
                        name: "skill-ledger".to_string(),
                        extension: "agent-sec-core".to_string(),
                        disabled: false,
                    },
                    HookEntryView {
                        name: "pii-checker".to_string(),
                        extension: "agent-sec-core".to_string(),
                        disabled: true,
                    },
                ],
            },
            HookEventGroup {
                event: "Stop".to_string(),
                hooks: vec![HookEntryView {
                    name: "observability-hook".to_string(),
                    extension: "agent-sec-core".to_string(),
                    disabled: false,
                }],
            },
        ]),
        footer: "3 hook(s) registered.".to_string(),
    }
}

#[test]
fn hook_status_panel_layers_sections_events_and_entries() {
    let renderer = RatatuiInlineRenderer::with_width(80);
    let mut output = Vec::new();
    renderer
        .write_hook_status_panel(&mut output, sample_model())
        .unwrap();

    let text = strip_ansi_escape(&String::from_utf8(output).unwrap());
    let lines = text.lines().collect::<Vec<_>>();
    // Section headers at column zero, like /help group headers.
    assert!(
        lines.iter().any(|line| line.starts_with("│ Shell Hooks")),
        "{text}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("│ Agent Hooks")),
        "{text}"
    );
    // Shell stats indented under their section.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│   Registered: 2")),
        "{text}"
    );
    // Event group headers indented by two, entries by four.
    assert!(
        lines.iter().any(|line| line.starts_with("│   PreToolUse")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│     • skill-ledger (ext: agent-sec-core)")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│     ○ pii-checker (ext: agent-sec-core) [disabled]")),
        "{text}"
    );
    // No legacy per-line event suffix.
    assert!(!text.contains("[PreToolUse]"), "{text}");
    // Footer inside the panel body.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("│ 3 hook(s) registered.")),
        "{text}"
    );
}

#[test]
fn hook_status_panel_message_view_and_plain_backend_degrade() {
    let mut model = sample_model();
    model.agent = AgentHooksView::Message("(none)".to_string());

    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let mut output = Vec::new();
    renderer
        .write_hook_status_panel(&mut output, model)
        .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Hook status:"), "{text}");
    assert!(text.contains("── Shell Hooks ──"), "{text}");
    assert!(text.contains("── Agent Hooks ──"), "{text}");
    assert!(text.contains("(none)"), "{text}");
    assert!(text.contains("3 hook(s) registered."), "{text}");
    assert!(!text.contains('│'), "{text}");
    assert!(!text.contains('\u{1b}'), "{text}");
}
