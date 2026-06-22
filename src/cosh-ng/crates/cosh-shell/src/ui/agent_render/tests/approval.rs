use super::*;
use crate::ui::CommandAssessmentSummaryModel;

#[test]
fn approval_panel_renders_active_request_with_queue_summary() {
    let renderer = RatatuiInlineRenderer::with_width(140);
    let text = renderer
        .approval_panel_lines(ApprovalPanelModel {
            id: "req-1",
            kind: "tool request",
            risk: "medium",
            reason: Some("diagnostic-pipeline-heuristic"),
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "top -l 1 -o mem -n 20 | head -30",
            queue_position: 1,
            queue_total: 4,
            next_label: Some("req-2 tool Bash"),
            selected_action: ApprovalPanelAction::Approve,
            expanded: false,
            hook_warnings: Vec::new(),
        })
        .join("\n");

    assert!(text.contains("Approval req-1"), "{text}");
    assert!(text.contains("Run Bash command?"), "{text}");
    assert!(
        text.contains("Reason: diagnostic-pipeline-heuristic"),
        "{text}"
    );
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
    assert_rendered_width(&text, 140);
}

#[test]
fn approval_panel_uses_zh_labels_without_translating_command() {
    let renderer = RatatuiInlineRenderer::with_width(140).with_language(crate::Language::ZhCn);
    let text = renderer
        .approval_panel_lines(ApprovalPanelModel {
            id: "req-1",
            kind: "tool request",
            risk: "medium",
            reason: None,
            subject: "tool Bash",
            preview_label: "Tool 输入",
            preview: "top -l 1 -o mem -n 20 | head -30",
            queue_position: 1,
            queue_total: 2,
            next_label: Some("req-2 tool Bash"),
            selected_action: ApprovalPanelAction::Approve,
            expanded: true,
            hook_warnings: Vec::new(),
        })
        .join("\n");

    assert!(text.contains("审批 req-1"), "{text}");
    assert!(text.contains("运行 Bash 命令？"), "{text}");
    assert!(
        text.contains("$ top -l 1 -o mem -n 20 | head -30"),
        "{text}"
    );
    assert!(text.contains("队列: 1/2 待处理"), "{text}");
    assert!(text.contains("下一个 req-2 tool Bash"), "{text}");
    assert!(text.contains("允许一次"), "{text}");
    assert!(text.contains("始终信任"), "{text}");
    assert!(text.contains("拒绝"), "{text}");
    assert!(text.contains("详情"), "{text}");
    assert!(text.contains("按键:"), "{text}");
    assert!(text.contains("默认: 拒绝"), "{text}");
    assert_rendered_width(&text, 140);
}

#[test]
fn approval_panel_keeps_focus_visible_and_caps_long_preview() {
    let renderer = RatatuiInlineRenderer::with_width(82);
    let text = renderer
        .approval_panel_lines(ApprovalPanelModel {
            id: "req-1",
            kind: "tool request",
            risk: "medium",
            reason: None,
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "echo \"=== 系统内存概览 ===\" && vm_stat && echo \"\" && echo \"=== 内存占用 Top 10 进程 ===\" && ps aux -m | head -11 && echo \"=== CPU 占用 Top 10 进程 ===\" && ps aux -r | head -11 && echo \"=== AliEntSafe 进程 ===\" && ps aux | grep AliEntSafe",
            queue_position: 1,
            queue_total: 1,
            next_label: None,
            selected_action: ApprovalPanelAction::Deny,
            expanded: false,
            hook_warnings: Vec::new(),
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
    let renderer = RatatuiInlineRenderer::with_width(70);
    let text = renderer
        .approval_panel_lines(ApprovalPanelModel {
            id: "req-宽",
            kind: "tool request",
            risk: "medium",
            reason: None,
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "cat /tmp/cosh-shell-中文-smoke.txt && echo 🧪 系统负载分析完成 && printf 'done\\n'",
            queue_position: 1,
            queue_total: 3,
            next_label: Some("req-2 tool Bash"),
            selected_action: ApprovalPanelAction::Details,
            expanded: true,
            hook_warnings: Vec::new(),
        })
        .join("\n");

    assert!(text.contains("Approval req-宽"), "{text}");
    assert!(text.contains("$ cat /tmp/cosh-shell-中文"), "{text}");
    assert!(text.contains("> [ Details ]"), "{text}");
    assert!(text.contains("Queue: 1/3 pending"), "{text}");
    assert_rendered_width(&text, 70);
    assert_box_lines_aligned(&text, 70);
}

#[test]
fn approval_panel_renders_shell_command_request_as_compact_command() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let text = renderer
        .approval_panel_lines(ApprovalPanelModel {
            id: "req-2",
            kind: "shell command request",
            risk: "high",
            reason: None,
            subject: "shell command",
            preview_label: "Command",
            preview: "touch /tmp/cosh-shell-fake-action-should-not-run",
            queue_position: 1,
            queue_total: 1,
            next_label: None,
            selected_action: ApprovalPanelAction::Deny,
            expanded: false,
            hook_warnings: Vec::new(),
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
        language: crate::Language::EnUs,
    };
    let mut output = Vec::new();

    renderer
        .write_approval_panel(
            &mut output,
            ApprovalPanelModel {
                id: "req-1",
                kind: "tool request",
                risk: "high",
                reason: None,
                subject: "tool Bash",
                preview_label: "Tool input",
                preview: "pwd",
                queue_position: 1,
                queue_total: 1,
                next_label: None,
                selected_action: ApprovalPanelAction::Deny,
                expanded: false,
                hook_warnings: Vec::new(),
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
        language: crate::Language::EnUs,
    }
    .write_approval_panel(
        &mut deny_output,
        ApprovalPanelModel {
            id: "req-1",
            kind: "tool request",
            risk: "medium",
            reason: None,
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "pwd",
            queue_position: 1,
            queue_total: 1,
            next_label: None,
            selected_action: ApprovalPanelAction::Deny,
            expanded: false,
            hook_warnings: Vec::new(),
        },
    )
    .expect("render deny approval panel");
    let deny = String::from_utf8(deny_output).expect("utf8 deny panel");

    let mut details_output = Vec::new();
    RatatuiInlineRenderer {
        width: 90,
        plain: false,
        styled: true,
        language: crate::Language::EnUs,
    }
    .write_approval_panel(
        &mut details_output,
        ApprovalPanelModel {
            id: "req-2",
            kind: "tool request",
            risk: "medium",
            reason: None,
            subject: "tool Bash",
            preview_label: "Tool input",
            preview: "pwd",
            queue_position: 1,
            queue_total: 1,
            next_label: None,
            selected_action: ApprovalPanelAction::Details,
            expanded: false,
            hook_warnings: Vec::new(),
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
        reason: None,
        subject: "tool Bash",
        preview_label: "Tool input",
        preview: "git status",
        queue_position: 1,
        queue_total: 2,
        next_label: Some("req-2 shell command"),
        selected_action: ApprovalPanelAction::Approve,
        expanded: false,
        hook_warnings: Vec::new(),
    });
    let text = lines.join("\n");

    assert!(text.contains("Approval required"), "{text}");
    assert!(text.contains("Queue: 1/2 pending"), "{text}");
    assert!(text.contains("Run Bash command?"), "{text}");
    assert!(text.contains("$ git status"), "{text}");
    assert!(text.contains("next req-2 shell command"), "{text}");
    assert!(
        text.contains("[Allow once]  Always trust  Deny  Details"),
        "{text}"
    );
    assert!(
        line_index(&lines, "Queue: 1/2 pending; next req-2 shell command")
            < line_index(&lines, "[Allow once]  Always trust  Deny  Details"),
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
            negative: true,
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
fn approval_receipt_panel_uses_zh_fallback_labels() {
    let renderer = RatatuiInlineRenderer::with_width(100).with_language(crate::Language::ZhCn);
    let shell_text = renderer
        .approval_receipt_panel_lines(ApprovalReceiptPanelModel {
            title: "已拒绝",
            negative: true,
            id: "req-1",
            kind: "shell 命令请求",
            decision: "已拒绝",
            subject: "shell command",
            preview: "git status",
            message: "命令未运行。",
        })
        .join("\n");
    let preview_text = renderer
        .approval_receipt_panel_lines(ApprovalReceiptPanelModel {
            title: "已拒绝",
            negative: true,
            id: "req-2",
            kind: "tool 请求",
            decision: "已拒绝",
            subject: "tool Read",
            preview: r#"{"file_path":"Cargo.toml"}"#,
            message: "Tool 未运行。",
        })
        .join("\n");

    assert!(shell_text.contains("命令: git status"), "{shell_text}");
    assert!(
        preview_text.contains(r#"预览: {"file_path":"Cargo.toml"}"#),
        "{preview_text}"
    );
    assert!(!shell_text.contains("Command:"), "{shell_text}");
    assert!(!preview_text.contains("Preview:"), "{preview_text}");
}

#[test]
fn approval_receipt_panel_uses_negative_state_not_localized_title_for_style() {
    let renderer = RatatuiInlineRenderer {
        width: 100,
        plain: false,
        styled: true,
        language: crate::Language::ZhCn,
    };
    let mut output = Vec::new();

    renderer
        .write_approval_receipt_panel(
            &mut output,
            ApprovalReceiptPanelModel {
                title: "已拒绝",
                negative: true,
                id: "req-1",
                kind: "shell 命令请求",
                decision: "已拒绝",
                subject: "shell command",
                preview: "git status",
                message: "命令未运行。",
            },
        )
        .expect("render styled zh approval receipt");

    let text = String::from_utf8(output).expect("utf8 receipt");
    let clean = strip_ansi_escape(&text);
    assert!(text.contains("\x1b[0;31m"), "{text:?}");
    assert!(clean.contains("已拒绝 req-1"), "{clean}");
    assert!(clean.contains("命令: git status"), "{clean}");
}

#[test]
fn approval_receipt_panel_can_render_compact_bash_approval() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let text = renderer
        .approval_receipt_panel_lines(ApprovalReceiptPanelModel {
            title: "Approved",
            negative: false,
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
            negative: true,
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
            negative: true,
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
            negative: true,
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
            negative: true,
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
            request_id: None,
            tool_use_id: None,
            execution_path: Some("foreground_shell_pty"),
            command_block_id: Some("cmd-7"),
            redaction_status: Some("ref_only"),
            assessment: Some(CommandAssessmentSummaryModel {
                impact: "medium",
                execution: "ask-user",
                confidence: "medium",
                primary_reason: "diagnostic-pipeline-heuristic",
                reason_trace: "diagnostic-pipeline-heuristic,pipeline-not-auto-executable",
                auto_allow: None,
                output_stability: "stable-snapshot",
                output_exposure: "may-contain-command-line",
            }),
        })
        .join("\n");

    assert!(text.contains("Approval details req-7"), "{text}");
    assert!(text.contains("tool request  pending  high risk"), "{text}");
    assert!(text.contains("Source: agent"), "{text}");
    assert!(text.contains("Run: run-12"), "{text}");
    assert!(text.contains("Execution: foreground_shell_pty"), "{text}");
    assert!(text.contains("Command block: cmd-7"), "{text}");
    assert!(text.contains("Redaction: ref_only"), "{text}");
    assert!(
        text.contains("Assessment: impact medium; decision ask-user; confidence medium"),
        "{text}"
    );
    assert!(
        text.contains("Reason: diagnostic-pipeline-heuristic"),
        "{text}"
    );
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
fn approval_details_panel_uses_zh_catalog_labels() {
    let renderer = RatatuiInlineRenderer::with_width(70).with_language(crate::Language::ZhCn);
    let text = renderer
        .approval_details_panel_lines(ApprovalDetailsPanelModel {
            id: "req-7",
            run_id: "run-12",
            source: "agent",
            kind: "tool request",
            status: "pending",
            risk: "high",
            subject: "tool Bash",
            preview_label: "Tool 输入",
            preview: "echo system && ps aux -m | head -11 && echo done",
            request_id: None,
            tool_use_id: None,
            execution_path: Some("foreground_shell_pty"),
            command_block_id: Some("cmd-7"),
            redaction_status: Some("ref_only"),
            assessment: Some(CommandAssessmentSummaryModel {
                impact: "medium",
                execution: "ask-user",
                confidence: "medium",
                primary_reason: "diagnostic-pipeline-heuristic",
                reason_trace: "diagnostic-pipeline-heuristic,pipeline-not-auto-executable",
                auto_allow: None,
                output_stability: "stable-snapshot",
                output_exposure: "may-contain-command-line",
            }),
        })
        .join("\n");

    assert!(text.contains("审批详情 req-7"), "{text}");
    assert!(text.contains("风险 high"), "{text}");
    assert!(text.contains("来源: agent"), "{text}");
    assert!(text.contains("运行: run-12"), "{text}");
    assert!(text.contains("执行: foreground_shell_pty"), "{text}");
    assert!(text.contains("命令块: cmd-7"), "{text}");
    assert!(text.contains("脱敏: ref_only"), "{text}");
    assert!(
        text.contains("评估: 影响 medium；决策 ask-user；置信度 medium"),
        "{text}"
    );
    assert!(
        text.contains("原因: diagnostic-pipeline-heuristic"),
        "{text}"
    );
    assert!(text.contains("默认: 拒绝"), "{text}");
    assert!(text.contains("请求: Bash 命令"), "{text}");
    assert!(text.contains("命令:"), "{text}");
    assert!(
        text.contains("策略: 可执行 tool 请求必须先经过用户审批。"),
        "{text}"
    );
    assert!(!text.contains("Approval details"), "{text}");
    assert!(!text.contains("Tool input"), "{text}");
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
            request_id: None,
            tool_use_id: None,
            execution_path: None,
            command_block_id: None,
            redaction_status: None,
            assessment: None,
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
            preview_hash: "fnv1a64:test0001",
            request_id: Some("ctrl-1"),
            tool_use_id: Some("toolu-1"),
            actor: "agent-auto",
            execution_path: Some("foreground_shell_pty"),
            command_block_id: Some("cmd-1"),
            redaction_status: Some("ref_only"),
            assessment: Some(CommandAssessmentSummaryModel {
                impact: "low",
                execution: "auto-allow",
                confidence: "high",
                primary_reason: "bounded-readonly",
                reason_trace: "bounded-readonly",
                auto_allow: Some("bounded-readonly"),
                output_stability: "stable-snapshot",
                output_exposure: "normal",
            }),
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
            preview_hash: "fnv1a64:test0002",
            request_id: None,
            tool_use_id: None,
            actor: "user",
            execution_path: Some("not_executed_denied"),
            command_block_id: None,
            redaction_status: None,
            assessment: None,
        },
    ];
    let text = renderer
        .approval_journal_panel_lines(ApprovalJournalPanelModel { entries: &entries })
        .join("\n");

    assert!(text.contains("Approval journal 2 decisions"), "{text}");
    assert!(text.contains("req-1  approved  tool request"), "{text}");
    assert!(text.contains("Source: agent  Run: run-1"), "{text}");
    assert!(text.contains("Execution: foreground_shell_pty"), "{text}");
    assert!(text.contains("Command block: cmd-1"), "{text}");
    assert!(text.contains("Redaction: ref_only"), "{text}");
    assert!(
        text.contains("Assessment: impact low; decision auto-allow; confidence high"),
        "{text}"
    );
    assert!(text.contains("Reason: bounded-readonly"), "{text}");
    assert!(text.contains("Actor: agent-auto"), "{text}");
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
fn approval_journal_panel_uses_zh_catalog_labels() {
    let renderer = RatatuiInlineRenderer::with_width(88).with_language(crate::Language::ZhCn);
    let entries = vec![ApprovalJournalEntryModel {
        id: "req-1",
        run_id: "run-1",
        source: "agent",
        decision: "approved",
        kind: "tool request",
        risk: "medium",
        subject: "tool shell",
        preview: "git status",
        preview_hash: "fnv1a64:test0001",
        request_id: Some("ctrl-1"),
        tool_use_id: Some("toolu-1"),
        actor: "agent-auto",
        execution_path: Some("foreground_shell_pty"),
        command_block_id: Some("cmd-1"),
        redaction_status: Some("ref_only"),
        assessment: None,
    }];
    let text = renderer
        .approval_journal_panel_lines(ApprovalJournalPanelModel { entries: &entries })
        .join("\n");

    assert!(text.contains("审批记录 1 条决策"), "{text}");
    assert!(text.contains("风险 medium"), "{text}");
    assert!(text.contains("来源: agent"), "{text}");
    assert!(text.contains("运行: run-1"), "{text}");
    assert!(text.contains("执行: foreground_shell_pty"), "{text}");
    assert!(text.contains("命令块: cmd-1"), "{text}");
    assert!(text.contains("脱敏: ref_only"), "{text}");
    assert!(text.contains("Provider 请求: ctrl-1"), "{text}");
    assert!(text.contains("Tool 使用: toolu-1"), "{text}");
    assert!(text.contains("执行者: agent-auto"), "{text}");
    assert!(text.contains("预览哈希: fnv1a64:test0001"), "{text}");
    assert!(text.contains("对象: tool shell"), "{text}");
    assert!(text.contains("命令: git status"), "{text}");
    assert!(!text.contains("Approval journal"), "{text}");
    assert!(!text.contains("Command block:"), "{text}");
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
        preview_hash: "fnv1a64:test0003",
        request_id: None,
        tool_use_id: None,
        actor: "user",
        execution_path: Some("not_executed_denied"),
        command_block_id: None,
        redaction_status: None,
        assessment: None,
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
        preview_hash: "fnv1a64:test0004",
        request_id: None,
        tool_use_id: None,
        actor: "user",
        execution_path: Some("not_executed_cancelled"),
        command_block_id: None,
        redaction_status: None,
        assessment: None,
    }];
    let text = renderer
        .approval_journal_panel_lines(ApprovalJournalPanelModel { entries: &entries })
        .join("\n");

    assert!(text.contains("Approval journal - 1 decisions"), "{text}");
    assert!(text.contains("req-1 cancelled - tool request"), "{text}");
    assert!(text.contains("Execution: not_executed_cancelled"), "{text}");
    assert!(text.contains("Command: git status"), "{text}");
    assert!(!text.contains('┌'), "{text}");
}
