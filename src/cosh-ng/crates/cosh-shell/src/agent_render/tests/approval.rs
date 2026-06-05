use super::*;

#[test]
fn approval_panel_renders_active_request_with_queue_summary() {
    let renderer = RatatuiInlineRenderer::with_width(140);
    let text = renderer
        .approval_panel_lines(ApprovalPanelModel {
            id: "req-1",
            kind: "tool request",
            risk: "medium",
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "top -l 1 -o mem -n 20 | head -30",
            queue_position: 1,
            queue_total: 4,
            next_label: Some("req-2 tool Bash"),
            selected_action: ApprovalPanelAction::Approve,
            expanded: false,
        })
        .join("\n");

    assert!(text.contains("Approval req-1"), "{text}");
    assert!(text.contains("Run Bash command?"), "{text}");
    assert!(
        text.contains("$ top -l 1 -o mem -n 20 | head -30"),
        "{text}"
    );
    assert!(text.contains("Queue: 1/4 pending"), "{text}");
    assert!(text.contains("next req-2 tool Bash"), "{text}");
    assert!(text.contains("Allow once"), "{text}");
    assert!(text.contains("Deny"), "{text}");
    assert!(text.contains("Details"), "{text}");
    assert!(!text.contains("medium risk"), "{text}");
    assert!(!text.contains("Command:"), "{text}");
    assert!(!text.contains("Review tool request"), "{text}");
    assert!(!text.contains("/approve"), "{text}");
    assert!(!text.contains("Subject: tool Bash"), "{text}");
    assert!(!text.contains("Tool input"), "{text}");
    assert_rendered_width(&text, 118);
}

#[test]
fn approval_panel_keeps_focus_visible_and_caps_long_preview() {
    let renderer = RatatuiInlineRenderer::with_width(82);
    let text = renderer
        .approval_panel_lines(ApprovalPanelModel {
            id: "req-1",
            kind: "tool request",
            risk: "medium",
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "echo \"=== 系统内存概览 ===\" && vm_stat && echo \"\" && echo \"=== 内存占用 Top 10 进程 ===\" && ps aux -m | head -11 && echo \"=== CPU 占用 Top 10 进程 ===\" && ps aux -r | head -11 && echo \"=== AliEntSafe 进程 ===\" && ps aux | grep AliEntSafe",
            queue_position: 1,
            queue_total: 1,
            next_label: None,
            selected_action: ApprovalPanelAction::Deny,
            expanded: false,
        })
        .join("\n");

    assert!(text.contains("> [ Deny ]"), "{text}");
    assert!(text.contains("..."), "{text}");
    assert!(!text.contains("Keys:"), "{text}");
    assert!(!text.contains("Left/Right select"), "{text}");
    assert_rendered_width(&text, 82);
}

#[test]
fn approval_panel_keeps_cjk_and_emoji_borders_aligned() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let text = renderer
        .approval_panel_lines(ApprovalPanelModel {
            id: "req-宽",
            kind: "tool request",
            risk: "medium",
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "cat /tmp/cosh-shell-中文-smoke.txt && echo 🧪 系统负载分析完成 && printf 'done\\n'",
            queue_position: 1,
            queue_total: 3,
            next_label: Some("req-2 tool Bash"),
            selected_action: ApprovalPanelAction::Details,
            expanded: true,
        })
        .join("\n");

    assert!(text.contains("Approval req-宽"), "{text}");
    assert!(text.contains("$ cat /tmp/cosh-shell-中文"), "{text}");
    assert!(text.contains("> [ Details ]"), "{text}");
    assert!(text.contains("Queue: 1/3 pending"), "{text}");
    assert_rendered_width(&text, 54);
    assert_box_lines_aligned(&text, 54);
}

#[test]
fn approval_panel_renders_shell_command_request_as_compact_command() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let text = renderer
        .approval_panel_lines(ApprovalPanelModel {
            id: "req-2",
            kind: "shell command request",
            risk: "high",
            subject: "shell command",
            preview_label: "Command",
            preview: "touch /tmp/cosh-shell-fake-action-should-not-run",
            queue_position: 1,
            queue_total: 1,
            next_label: None,
            selected_action: ApprovalPanelAction::Deny,
            expanded: false,
        })
        .join("\n");

    assert!(text.contains("Approval req-2"), "{text}");
    assert!(text.contains("Run shell command?"), "{text}");
    assert!(
        text.contains("$ touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{text}"
    );
    assert!(text.contains("> [ Deny ]"), "{text}");
    assert!(!text.contains("shell command request"), "{text}");
    assert!(!text.contains("high risk"), "{text}");
    assert!(!text.contains("Subject:"), "{text}");
    assert!(!text.contains("Command:"), "{text}");
    assert_rendered_width(&text, 100);
}

#[test]
fn approval_panel_write_preserves_ratatui_styles_for_terminal_output() {
    let renderer = RatatuiInlineRenderer {
        width: 90,
        plain: false,
        styled: true,
    };
    let mut output = Vec::new();

    renderer
        .write_approval_panel(
            &mut output,
            ApprovalPanelModel {
                id: "req-1",
                kind: "tool request",
                risk: "high",
                subject: "tool Bash",
                preview_label: "Tool input",
                preview: "pwd",
                queue_position: 1,
                queue_total: 1,
                next_label: None,
                selected_action: ApprovalPanelAction::Deny,
                expanded: false,
            },
        )
        .expect("render approval panel");

    let text = String::from_utf8(output).expect("utf8 panel");
    let clean = strip_ansi_escape(&text);
    assert!(text.contains("\x1b["), "{text:?}");
    assert!(clean.contains("> [ Deny ]"), "{clean}");
    assert!(clean.contains("pwd"), "{clean}");
}

#[test]
fn approval_panel_styles_selected_actions_by_decision_kind() {
    let mut deny_output = Vec::new();
    RatatuiInlineRenderer {
        width: 90,
        plain: false,
        styled: true,
    }
    .write_approval_panel(
        &mut deny_output,
        ApprovalPanelModel {
            id: "req-1",
            kind: "tool request",
            risk: "medium",
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "pwd",
            queue_position: 1,
            queue_total: 1,
            next_label: None,
            selected_action: ApprovalPanelAction::Deny,
            expanded: false,
        },
    )
    .expect("render deny approval panel");
    let deny = String::from_utf8(deny_output).expect("utf8 deny panel");

    let mut details_output = Vec::new();
    RatatuiInlineRenderer {
        width: 90,
        plain: false,
        styled: true,
    }
    .write_approval_panel(
        &mut details_output,
        ApprovalPanelModel {
            id: "req-2",
            kind: "tool request",
            risk: "medium",
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "pwd",
            queue_position: 1,
            queue_total: 1,
            next_label: None,
            selected_action: ApprovalPanelAction::Details,
            expanded: false,
        },
    )
    .expect("render details approval panel");
    let details = String::from_utf8(details_output).expect("utf8 details panel");

    assert!(deny.contains("\x1b[0;1;97;41m> [ Deny ]"), "{deny:?}");
    assert!(!deny.contains("\x1b[0;1;97;42m> [ Deny ]"), "{deny:?}");
    assert!(
        details.contains("\x1b[0;1;97;44m> [ Details ]"),
        "{details:?}"
    );
}

#[test]
fn plain_approval_panel_keeps_queue_before_actions() {
    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let lines = renderer.approval_panel_lines(ApprovalPanelModel {
        id: "req-1",
        kind: "tool request",
        risk: "medium",
        subject: "tool Bash",
        preview_label: "Tool input",
        preview: "git status",
        queue_position: 1,
        queue_total: 2,
        next_label: Some("req-2 shell command"),
        selected_action: ApprovalPanelAction::Approve,
        expanded: false,
    });
    let text = lines.join("\n");

    assert!(text.contains("Approval required"), "{text}");
    assert!(text.contains("Queue: 1/2 pending"), "{text}");
    assert!(text.contains("Run Bash command?"), "{text}");
    assert!(text.contains("$ git status"), "{text}");
    assert!(text.contains("next req-2 shell command"), "{text}");
    assert!(text.contains("[Allow once]  Deny  Details"), "{text}");
    assert!(
        line_index(&lines, "Queue: 1/2 pending; next req-2 shell command")
            < line_index(&lines, "[Allow once]  Deny  Details"),
        "{text}"
    );
    assert!(!text.contains("medium risk"), "{text}");
    assert!(!text.contains("Command:"), "{text}");
    assert!(!text.contains("Review tool request"), "{text}");
}

#[test]
fn approval_receipt_panel_renders_auditable_decision() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let text = renderer
        .approval_receipt_panel_lines(ApprovalReceiptPanelModel {
            title: "Denied",
            id: "req-1",
            kind: "Bash tool",
            decision: "denied by user",
            subject: "tool shell",
            preview: "git status",
            message: "No command ran.",
        })
        .join("\n");

    assert!(text.contains("Denied req-1"), "{text}");
    assert!(text.contains("Command: git status"), "{text}");
    assert!(text.contains("No command ran."), "{text}");
    assert!(!text.contains("Bash tool - denied by user"), "{text}");
    assert!(!text.contains("Subject:"), "{text}");
    assert_rendered_width(&text, 100);
}

#[test]
fn approval_receipt_panel_can_render_compact_bash_approval() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let text = renderer
        .approval_receipt_panel_lines(ApprovalReceiptPanelModel {
            title: "Approved",
            id: "req-1",
            kind: "",
            decision: "",
            subject: "tool Bash",
            preview: "",
            message: "",
        })
        .join("\n");

    assert!(text.contains("Approved req-1"), "{text}");
    assert!(!text.contains("Bash tool - approved"), "{text}");
    assert!(!text.contains("Command:"), "{text}");
    assert!(!text.contains("Running command"), "{text}");
    assert!(!text.contains('┌'), "{text}");
    assert!(!text.contains('└'), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
    assert_rendered_width(&text, 100);
}

#[test]
fn approval_receipt_panel_wraps_long_command_and_message() {
    let renderer = RatatuiInlineRenderer::with_width(62);
    let text = renderer
        .approval_receipt_panel_lines(ApprovalReceiptPanelModel {
            title: "Denied",
            id: "req-9",
            kind: "shell command request",
            decision: "denied",
            subject: "shell command",
            preview: "touch /tmp/cosh-shell-fake-action-should-not-run && echo should-not-run",
            message: "No command ran; the shell prompt stays available for the next user command.",
        })
        .join("\n");

    assert!(text.contains("Denied req-9"), "{text}");
    assert!(
        text.contains("Command: touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{text}"
    );
    assert!(text.contains("         && echo should-not-run"), "{text}");
    assert!(
        text.contains("No command ran; the shell prompt stays available for the"),
        "{text}"
    );
    assert!(text.contains("next user command."), "{text}");
    assert_rendered_width(&text, 62);
}

#[test]
fn approval_receipt_panel_keeps_cjk_and_emoji_borders_aligned() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let text = renderer
        .approval_receipt_panel_lines(ApprovalReceiptPanelModel {
            title: "Denied",
            id: "req-宽",
            kind: "shell command request",
            decision: "denied",
            subject: "shell command",
            preview: "cat /tmp/cosh-shell-中文-smoke.txt && echo 🧪 should-not-run",
            message: "No command ran; shell prompt stays available.",
        })
        .join("\n");

    assert!(text.contains("Denied req-宽"), "{text}");
    assert!(text.contains("Command: cat"), "{text}");
    assert!(text.contains("中文-smoke.txt"), "{text}");
    assert!(text.contains("No command ran"), "{text}");
    assert_rendered_width(&text, 54);
    assert_box_lines_aligned(&text, 54);
}

#[test]
fn plain_approval_receipt_panel_keeps_cancel_text() {
    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let text = renderer
        .approval_receipt_panel_lines(ApprovalReceiptPanelModel {
            title: "Cancelled",
            id: "req-2",
            kind: "shell command request",
            decision: "cancelled by user",
            subject: "shell command",
            preview: "touch /tmp/nope",
            message: "No command ran.",
        })
        .join("\n");

    assert!(text.contains("Cancelled req-2"), "{text}");
    assert!(text.contains("Command: touch /tmp/nope"), "{text}");
    assert!(text.contains("No command ran."), "{text}");
    assert!(
        !text.contains("shell command request - cancelled by user"),
        "{text}"
    );
    assert!(!text.contains('╭'), "{text}");
}

#[test]
fn plain_approval_receipt_panel_wraps_long_command() {
    let renderer = RatatuiInlineRenderer::plain_with_width(50);
    let text = renderer
        .approval_receipt_panel_lines(ApprovalReceiptPanelModel {
            title: "Denied",
            id: "req-10",
            kind: "shell command request",
            decision: "denied",
            subject: "shell command",
            preview: "touch /tmp/cosh-shell-fake-action-should-not-run && echo should-not-run",
            message: "No command ran; the shell prompt stays available.",
        })
        .join("\n");

    assert!(text.contains("Denied req-10"), "{text}");
    assert!(text.contains("Command: touch"), "{text}");
    assert!(
        text.contains("         /tmp/cosh-shell-fake-action-should-no"),
        "{text}"
    );
    assert!(
        text.contains("         t-run && echo should-not-run"),
        "{text}"
    );
    assert!(
        text.contains("No command ran; the shell prompt stays"),
        "{text}"
    );
    assert!(text.contains("available."), "{text}");
    assert!(!text.contains('┌'), "{text}");
    assert_rendered_width(&text, 50);
}

#[test]
fn approval_details_panel_renders_structured_request_context() {
    let renderer = RatatuiInlineRenderer::with_width(70);
    let text = renderer
        .approval_details_panel_lines(ApprovalDetailsPanelModel {
            id: "req-7",
            run_id: "run-12",
            source: "agent",
            kind: "tool request",
            status: "pending",
            risk: "high",
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "echo system && ps aux -m | head -11 && echo done",
        })
        .join("\n");

    assert!(text.contains("Approval details req-7"), "{text}");
    assert!(text.contains("tool request  pending  high risk"), "{text}");
    assert!(text.contains("Source: agent"), "{text}");
    assert!(text.contains("Run: run-12"), "{text}");
    assert!(text.contains("Default: deny"), "{text}");
    assert!(text.contains("Request: Bash command"), "{text}");
    assert!(text.contains("Command:"), "{text}");
    assert!(text.contains("ps aux -m"), "{text}");
    assert!(text.contains("Policy: user approval is required"), "{text}");
    assert!(!text.contains("Subject: tool Bash"), "{text}");
    assert!(!text.contains("Tool input"), "{text}");
    assert!(!text.contains("Approval details\nid:"), "{text}");
    assert_rendered_width(&text, 70);
}

#[test]
fn approval_details_panel_keeps_cjk_and_emoji_borders_aligned() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let text = renderer
        .approval_details_panel_lines(ApprovalDetailsPanelModel {
            id: "req-宽",
            run_id: "run-中文-1",
            source: "agent",
            kind: "tool request",
            status: "pending",
            risk: "high",
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "cat /tmp/cosh-shell-中文-smoke.txt && echo 🧪 approval details",
        })
        .join("\n");

    assert!(text.contains("Approval details req-宽"), "{text}");
    assert!(text.contains("run-中文-1"), "{text}");
    assert!(text.contains("中文-smoke.txt"), "{text}");
    assert_rendered_width(&text, 54);
    assert_box_lines_aligned(&text, 54);
}

#[test]
fn approval_journal_panel_renders_decision_history() {
    let renderer = RatatuiInlineRenderer::with_width(88);
    let entries = vec![
        ApprovalJournalEntryModel {
            id: "req-1",
            run_id: "run-1",
            source: "agent",
            decision: "approved",
            kind: "tool request",
            risk: "medium",
            subject: "tool shell",
            preview: "git status",
        },
        ApprovalJournalEntryModel {
            id: "req-2",
            run_id: "run-1",
            source: "agent",
            decision: "denied",
            kind: "shell command request",
            risk: "high",
            subject: "shell command",
            preview: "touch /tmp/cosh-shell-fake-action-should-not-run",
        },
    ];
    let text = renderer
        .approval_journal_panel_lines(ApprovalJournalPanelModel { entries: &entries })
        .join("\n");

    assert!(text.contains("Approval journal 2 decisions"), "{text}");
    assert!(text.contains("req-1  approved  tool request"), "{text}");
    assert!(text.contains("Source: agent  Run: run-1"), "{text}");
    assert!(text.contains("Command: git status"), "{text}");
    assert!(
        text.contains("req-2  denied  shell command request"),
        "{text}"
    );
    assert!(
        text.contains("touch /tmp/cosh-shell-fake-action-should-not-run"),
        "{text}"
    );
    assert!(!text.contains("run:"), "{text}");
    assert_rendered_width(&text, 88);
}

#[test]
fn approval_journal_panel_keeps_cjk_and_emoji_borders_aligned() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let entries = vec![ApprovalJournalEntryModel {
        id: "req-宽",
        run_id: "run-中文-1",
        source: "agent",
        decision: "denied",
        kind: "shell command request",
        risk: "high",
        subject: "shell command",
        preview: "cat /tmp/cosh-shell-中文-smoke.txt && echo 🧪 should-not-run",
    }];
    let text = renderer
        .approval_journal_panel_lines(ApprovalJournalPanelModel { entries: &entries })
        .join("\n");

    assert!(text.contains("Approval journal 1 decisions"), "{text}");
    assert!(text.contains("req-宽"), "{text}");
    assert!(text.contains("run-中文-1"), "{text}");
    assert!(text.contains("中文-smoke.txt"), "{text}");
    assert_rendered_width(&text, 54);
    assert_box_lines_aligned(&text, 54);
}

#[test]
fn plain_approval_journal_panel_keeps_decision_history() {
    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let entries = vec![ApprovalJournalEntryModel {
        id: "req-1",
        run_id: "run-1",
        source: "agent",
        decision: "cancelled",
        kind: "tool request",
        risk: "medium",
        subject: "tool shell",
        preview: "git status",
    }];
    let text = renderer
        .approval_journal_panel_lines(ApprovalJournalPanelModel { entries: &entries })
        .join("\n");

    assert!(text.contains("Approval journal - 1 decisions"), "{text}");
    assert!(text.contains("req-1 cancelled - tool request"), "{text}");
    assert!(text.contains("Command: git status"), "{text}");
    assert!(!text.contains('┌'), "{text}");
}
