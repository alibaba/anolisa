use super::{
    strip_ansi_escape, ActivityDetailsPanelModel, ActivityPanelModel, ActivityRowModel,
    ApprovalDetailsPanelModel, ApprovalJournalEntryModel, ApprovalJournalPanelModel,
    ApprovalPanelAction, ApprovalPanelModel, ApprovalReceiptPanelModel, NoticePanelModel,
    QuestionAnswerPanelModel, QuestionPanelModel, RatatuiInlineRenderer,
    RecommendationActionPanelModel, RecommendationPanelModel,
};
use crate::types::{
    AgentEvent, GovernanceDecision, GovernancePolicyDecision, GovernedEvent, QuestionSelectionMode,
};

mod approval;
mod markdown;
mod question;
#[test]
fn wraps_long_agent_text_with_ratatui() {
    let event = GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::DisplayOnly,
        event: AgentEvent::TextDelta {
            run_id: "run-1".to_string(),
            text:
                "hello 你好 this is a long response that should wrap inside a narrow shell viewport"
                    .to_string(),
        },
        reason: "display".to_string(),
        display_text:
            "hello 你好 this is a long response that should wrap inside a narrow shell viewport"
                .to_string(),
        auto_execute: false,
    };

    let lines = RatatuiInlineRenderer::with_width(40).governed_event_lines(&[event]);

    assert!(lines.len() > 1, "{lines:?}");
    assert!(lines[0].starts_with("hello"));
    assert!(lines.iter().all(|line| line.chars().count() <= 40));
}

#[test]
fn governed_events_use_zh_renderer_labels_without_translating_commands() {
    let event = GovernedEvent {
        decision: GovernanceDecision::Display,
        policy_decision: GovernancePolicyDecision::DisplayOnly,
        event: AgentEvent::Recommendation {
            run_id: "run-1".to_string(),
            summary: "建议先检查磁盘。".to_string(),
            commands: vec!["df -h".to_string(), "du -sh .".to_string()],
            auto_execute: false,
        },
        reason: "display".to_string(),
        display_text: "unused fallback".to_string(),
        auto_execute: false,
    };
    let renderer = RatatuiInlineRenderer::with_width(80).with_language(crate::Language::ZhCn);

    let lines = renderer.governed_event_lines(std::slice::from_ref(&event));
    let text = lines.join("\n");
    assert!(text.contains("推荐命令:"), "{text}");
    assert!(text.contains("df -h"), "{text}");
    assert!(!text.contains("recommended commands:"), "{text}");

    let mut output = Vec::new();
    renderer
        .write_governed_events(&mut output, &[event])
        .unwrap();
    let block = String::from_utf8(output).unwrap();
    assert!(block.contains("治理"), "{block}");
    assert!(!block.contains("Governance"), "{block}");
}

#[test]
fn streaming_agent_strips_bold_markers_without_dropping_bullets() {
    let renderer = RatatuiInlineRenderer::with_width(60);
    let mut stream = renderer.stream_agent();
    let mut output = Vec::new();

    stream
        .write_delta(&mut output, "\n\n**建议:**\n* keep this bullet")
        .unwrap();
    stream.finish(&mut output, None).unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(!text.contains("│ \n│ \n"));
    assert!(text.contains("│ 建议:"));
    assert!(text.contains("│ * keep this bullet"));
    assert!(!text.contains("**"));
}

#[test]
fn streaming_agent_uses_zh_catalog_title() {
    let renderer = RatatuiInlineRenderer::with_width(60).with_language(crate::Language::ZhCn);
    let mut stream = renderer.stream_agent();
    let mut output = Vec::new();

    stream.write_delta(&mut output, "你好").unwrap();
    stream.finish(&mut output, None).unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("╭ Agent 回复"), "{text}");
    assert!(!text.contains("╭ Agent ─"), "{text}");
}

#[test]
fn streaming_agent_suppresses_code_fence_language_and_left_trims_lines() {
    let renderer = RatatuiInlineRenderer::with_width(80);
    let mut stream = renderer.stream_agent();
    let mut output = Vec::new();

    stream
        .write_delta(
            &mut output,
            "原因: 目标不存在。\n```bash\n    ls\n    find . -name \"ccc*\"\n```\n完成。",
        )
        .unwrap();
    stream.finish(&mut output, None).unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("│ 原因: 目标不存在。"));
    assert!(text.contains("│ ls"));
    assert!(text.contains("│ find . -name \"ccc*\""));
    assert!(text.contains("│ 完成。"));
    assert!(!text.contains("bash"));
    assert!(!text.contains("```"));
    assert!(!text.contains("│     ls"));
}

#[test]
fn streaming_agent_prefers_word_boundaries_across_deltas() {
    let renderer = RatatuiInlineRenderer::with_width(30);
    let mut stream = renderer.stream_agent();
    let mut output = Vec::new();

    stream
        .write_delta(&mut output, "alpha beta gamma delta epsilon wo")
        .unwrap();
    stream.write_delta(&mut output, "rkspace command").unwrap();
    stream.finish(&mut output, None).unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(!text.contains("epsilon wo\n│ rkspace"));
    assert!(text.contains("│ alpha beta gamma delta epsilon"), "{text}");
    assert!(
        text.contains("│ workspace command"),
        "streaming output should wrap before the whole word:\n{text}"
    );
}

#[test]
fn plain_renderer_uses_text_blocks_without_box_drawing() {
    let renderer = RatatuiInlineRenderer::plain_with_width(44);
    let mut output = Vec::new();

    renderer
        .write_notice_panel(
            &mut output,
            NoticePanelModel {
                title: "Agent status",
                body: vec![
                    "Phase: requesting".to_string(),
                    "waiting for backend".to_string(),
                ],
                footer: None,
            },
        )
        .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Agent status:"));
    assert!(text.contains("  Phase: requesting"));
    assert!(!text.contains('╭'));
    assert!(!text.contains('│'));
    assert!(!text.contains('╰'));
}

#[test]
fn plain_streaming_agent_uses_text_prefix() {
    let renderer = RatatuiInlineRenderer::plain_with_width(44);
    let mut stream = renderer.stream_agent();
    let mut output = Vec::new();

    stream
        .write_delta(&mut output, "hello from stream")
        .unwrap();
    stream
        .finish(
            &mut output,
            Some("Commands are suggestions only; nothing was executed."),
        )
        .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Agent:"));
    assert!(text.contains("  hello from stream"));
    assert!(text.contains("Commands are suggestions only"));
    assert!(text.contains("nothing"));
    assert!(text.contains("executed."));
    assert!(!text.contains('╭'));
    assert!(!text.contains('│'));
    assert!(!text.contains('╰'));
}

#[test]
fn activity_panel_renders_tool_output_rows() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let text = renderer
        .activity_panel_lines(ActivityPanelModel {
            rows: vec![
                ActivityRowModel {
                    id: "out-1",
                    kind: "output",
                    status: "captured",
                    subject: "tool-1",
                    summary: "stdout captured; [Details] out-1",
                },
                ActivityRowModel {
                    id: "tool-1",
                    kind: "tool",
                    status: "completed",
                    subject: "tool-1",
                    summary: "exit 0",
                },
            ],
        })
        .join("\n");

    assert!(text.contains("Activity"), "{text}");
    assert!(
        text.contains("Tool output: stdout captured; [Details] out-1"),
        "{text}"
    );
    assert!(text.contains("Tool completed: exit 0"), "{text}");
    assert!(
        !text.contains("out-1 output: stdout captured; [Details] out-1"),
        "{text}"
    );
    assert!(!text.contains("tool-1 tool: completed"), "{text}");
    assert_rendered_width(&text, 100);
}

#[test]
fn activity_panel_uses_zh_catalog_labels() {
    let renderer = RatatuiInlineRenderer::with_width(100).with_language(crate::Language::ZhCn);
    let text = renderer
        .activity_panel_lines(ActivityPanelModel {
            rows: vec![
                ActivityRowModel {
                    id: "out-1",
                    kind: "output",
                    status: "captured",
                    subject: "tool-1",
                    summary: "stdout 已捕获；[Details] out-1",
                },
                ActivityRowModel {
                    id: "skill-1",
                    kind: "skill",
                    status: "failed",
                    subject: "linux_memory",
                    summary: "linux_memory 失败",
                },
                ActivityRowModel {
                    id: "tool-1",
                    kind: "tool",
                    status: "requested",
                    subject: "toolu-1",
                    summary: "run_shell_command 请求审批：$ df -h；[Details] tool-1",
                },
            ],
        })
        .join("\n");

    assert!(text.contains("活动"), "{text}");
    assert!(
        text.contains("Tool 输出: stdout 已捕获；[Details] out-1"),
        "{text}"
    );
    assert!(text.contains("技能 失败: linux_memory"), "{text}");
    assert!(
        text.contains("Tool 请求审批: run_shell_command 请求审批：$ df -h；[Details] tool-1"),
        "{text}"
    );
    assert!(!text.contains("Activity"), "{text}");
    assert!(!text.contains("Tool output:"), "{text}");
    assert!(!text.contains("Skill failed"), "{text}");
    assert!(!text.contains("Tool requested"), "{text}");
}

#[test]
fn activity_panel_wraps_long_rows_without_dropping_details_reference() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let text = renderer
        .activity_panel_lines(ActivityPanelModel {
            rows: vec![ActivityRowModel {
                id: "out-1",
                kind: "output",
                status: "captured",
                subject: "req-7",
                summary: "stdout captured from approved request with a long summary; inspect [Details] out-1",
            }],
        })
        .join("\n");

    assert!(text.contains("stdout captured"), "{text}");
    assert!(text.contains("[Details]"), "{text}");
    assert!(text.contains("out-1"), "{text}");
    assert!(!text.contains("req-7"), "{text}");
    assert_rendered_width(&text, 54);
}

#[test]
fn activity_panel_keeps_card_border_aligned_to_renderer_width() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let text = renderer
        .activity_panel_lines(ActivityPanelModel {
            rows: vec![ActivityRowModel {
                id: "out-1",
                kind: "output",
                status: "captured",
                subject: "tool-1",
                summary: "stdout captured from 中文路径 🧪; inspect [Details] out-1",
            }],
        })
        .join("\n");

    assert!(text.contains("Activity"), "{text}");
    assert!(text.contains("中文路径 🧪"), "{text}");
    assert_rendered_width(&text, 54);
    assert_box_lines_aligned(&text, 54);
}

#[test]
fn activity_panel_write_preserves_ratatui_styles_for_terminal_output() {
    let renderer = RatatuiInlineRenderer {
        width: 100,
        plain: false,
        styled: true,
        language: crate::Language::EnUs,
    };
    let mut output = Vec::new();

    renderer
        .write_activity_panel(
            &mut output,
            ActivityPanelModel {
                rows: vec![ActivityRowModel {
                    id: "out-1",
                    kind: "output",
                    status: "captured",
                    subject: "tool-1",
                    summary: "stdout captured; [Details] out-1",
                }],
            },
        )
        .expect("render activity panel");

    let text = String::from_utf8(output).expect("utf8 panel");
    let clean = strip_ansi_escape(&text);
    assert!(text.contains("\x1b["), "{text:?}");
    assert!(clean.contains("Activity"), "{clean}");
    assert!(clean.contains("Tool output"), "{clean}");
    assert!(!clean.contains("out-1 output"), "{clean}");
}

#[test]
fn plain_activity_panel_keeps_user_facing_row_text() {
    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let text = renderer
        .activity_panel_lines(ActivityPanelModel {
            rows: vec![ActivityRowModel {
                id: "out-1",
                kind: "output",
                status: "captured",
                subject: "tool-1",
                summary: "stdout captured; [Details] out-1",
            }],
        })
        .join("\n");

    assert!(text.contains("Activity:"), "{text}");
    assert!(
        text.contains("Tool output: stdout captured; [Details] out-1"),
        "{text}"
    );
    assert!(!text.contains("out-1 output:"), "{text}");
    assert!(!text.contains('╭'), "{text}");
}

#[test]
fn plain_activity_panel_wraps_long_rows_without_dropping_details_reference() {
    let renderer = RatatuiInlineRenderer::plain_with_width(50);
    let text = renderer
        .activity_panel_lines(ActivityPanelModel {
            rows: vec![ActivityRowModel {
                id: "out-1",
                kind: "output",
                status: "captured",
                subject: "req-7",
                summary: "stdout captured from approved request with a long summary; inspect [Details] out-1",
            }],
        })
        .join("\n");

    assert!(text.contains("Activity:"), "{text}");
    assert!(text.contains("Tool output: stdout captured from"), "{text}");
    assert!(
        text.contains("request with a long summary; inspect [Details]"),
        "{text}"
    );
    assert!(text.contains("out-1"), "{text}");
    assert!(!text.contains("req-7"), "{text}");
    assert!(!text.contains('╭'), "{text}");
    assert_rendered_width(&text, 50);
}

#[test]
fn activity_details_panel_renders_output_ref_and_detail_tail() {
    let renderer = RatatuiInlineRenderer::with_width(58);
    let text = renderer
        .activity_details_panel_lines(ActivityDetailsPanelModel {
            id: "out-1",
            run_id: "run-7",
            kind: "output",
            status: "captured",
            subject: "tool-1",
            summary: "stdout captured; [Details] out-1",
            detail: "tool: tool-1\nstream: stdout\nlines: 24\nref: /tmp/cosh-shell/out-1.txt\nline 1: fake tool output for details view\nline 24: fake tool output for details view",
        })
        .join("\n");

    assert!(text.contains("Activity details out-1"), "{text}");
    assert!(text.contains("output - stdout captured"), "{text}");
    assert!(text.contains("Run: run-7"), "{text}");
    assert!(text.contains("Detail:"), "{text}");
    assert!(text.contains("ref: /tmp/cosh-shell/out-1.txt"), "{text}");
    assert!(text.contains("line 24: fake tool output"), "{text}");
    assert!(!text.contains("id: out-1"), "{text}");
    assert_rendered_width(&text, 58);
}

#[test]
fn activity_details_panel_uses_zh_catalog_labels() {
    let renderer = RatatuiInlineRenderer::with_width(58).with_language(crate::Language::ZhCn);
    let text = renderer
        .activity_details_panel_lines(ActivityDetailsPanelModel {
            id: "out-1",
            run_id: "run-7",
            kind: "output",
            status: "captured",
            subject: "tool-1",
            summary: "stdout 已捕获；[Details] out-1",
            detail: "tool: tool-1\nstream: stdout\nref: /tmp/cosh-shell/out-1.txt",
        })
        .join("\n");

    assert!(text.contains("活动详情 out-1"), "{text}");
    assert!(
        text.contains("Tool 输出 - stdout 已捕获；[Details] out-1"),
        "{text}"
    );
    assert!(text.contains("运行: run-7"), "{text}");
    assert!(text.contains("详情:"), "{text}");
    assert!(text.contains("ref: /tmp/cosh-shell/out-1.txt"), "{text}");
    assert!(!text.contains("Activity details"), "{text}");
    assert!(!text.contains("output - stdout"), "{text}");
    assert!(!text.contains("Run:"), "{text}");
    assert!(!text.contains("Detail:"), "{text}");
}

#[test]
fn activity_details_panel_keeps_card_border_aligned_to_renderer_width() {
    let renderer = RatatuiInlineRenderer::with_width(58);
    let text = renderer
        .activity_details_panel_lines(ActivityDetailsPanelModel {
            id: "out-1",
            run_id: "run-中文-1",
            kind: "output",
            status: "captured",
            subject: "tool-1",
            summary: "stdout captured; [Details] out-1",
            detail:
                "ref: /tmp/cosh-shell/中文/out-1.txt\nline: CPU 🧪 output summary with long tail",
        })
        .join("\n");

    assert!(text.contains("Activity details out-1"), "{text}");
    assert!(text.contains("run-中文-1"), "{text}");
    assert!(text.contains("/tmp/cosh-shell/中文/out-1.txt"), "{text}");
    assert_rendered_width(&text, 58);
    assert_box_lines_aligned(&text, 58);
}

#[test]
fn plain_activity_details_panel_wraps_long_lines() {
    let renderer = RatatuiInlineRenderer::plain_with_width(50);
    let text = renderer
        .activity_details_panel_lines(ActivityDetailsPanelModel {
            id: "out-1",
            run_id: "run-with-a-very-long-identifier",
            kind: "output",
            status: "captured",
            subject: "tool-1",
            summary: "stdout captured; [Details] out-1",
            detail: "tool: tool-1\nstream: stdout\nref: /tmp/cosh-shell/very/long/path/out-1.txt\nline 24: fake tool output for details view with long trailing text",
        })
        .join("\n");

    assert!(text.contains("Activity details out-1"), "{text}");
    assert!(
        text.contains("output - stdout captured; [Details] out-1"),
        "{text}"
    );
    assert!(text.contains("tool-1"), "{text}");
    assert!(
        text.contains("Run: run-with-a-very-long-identifier"),
        "{text}"
    );
    assert!(text.contains("Detail:"), "{text}");
    assert!(
        text.contains("ref: /tmp/cosh-shell/very/long/path/out-1.txt"),
        "{text}"
    );
    assert!(
        text.contains("line 24: fake tool output for details view with"),
        "{text}"
    );
    assert!(text.contains("long trailing text"), "{text}");
    assert!(!text.contains('╭'), "{text}");
    assert_rendered_width(&text, 50);
}

#[test]
fn recommendation_panel_renders_display_only_commands() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let commands = vec!["pwd".to_string(), "echo $PATH".to_string()];
    let text = renderer
        .recommendation_panel_lines(RecommendationPanelModel {
            commands: &commands,
        })
        .join("\n");

    assert!(text.contains("Recommendations"), "{text}");
    assert!(text.contains("1. pwd"), "{text}");
    assert!(text.contains("2. echo $PATH"), "{text}");
    assert!(text.contains("│  1. pwd"), "{text}");
    assert!(text.contains("│  2. echo $PATH"), "{text}");
    assert!(text.contains("[Copy] [Insert] [Details]"), "{text}");
    assert!(text.contains("display-only"), "{text}");
    assert!(!text.contains("/allow N"), "{text}");
    assert_rendered_width(&text, 100);
}

#[test]
fn recommendation_panel_wraps_long_commands_without_dropping_tail() {
    let renderer = RatatuiInlineRenderer::with_width(56);
    let commands = vec![
        "cargo test --package cosh-shell --test raw_cli raw_cli_streaming_tool_approval_renders_before_agent_finishes -- --test-threads=1".to_string(),
    ];
    let text = renderer
        .recommendation_panel_lines(RecommendationPanelModel {
            commands: &commands,
        })
        .join("\n");

    assert!(text.contains("cargo test --package cosh-shell"), "{text}");
    assert!(text.contains("raw_cli_streaming_tool_approval"), "{text}");
    assert!(text.contains("--test-threads=1"), "{text}");
    assert!(text.contains("[Copy] [Insert] [Details]"), "{text}");
    assert!(text.contains("display-only"), "{text}");
    assert_rendered_width(&text, 56);
}

#[test]
fn recommendation_panel_keeps_card_border_aligned_to_renderer_width() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let commands = vec![
        "cat /tmp/cosh-shell-中文-smoke.txt && echo 🧪 recommendation done".to_string(),
        "cargo test --package cosh-shell -- --test-threads=1".to_string(),
    ];
    let text = renderer
        .recommendation_panel_lines(RecommendationPanelModel {
            commands: &commands,
        })
        .join("\n");

    assert!(text.contains("Recommendations"), "{text}");
    assert!(text.contains("中文-smoke.txt"), "{text}");
    assert!(text.contains("🧪"), "{text}");
    assert_rendered_width(&text, 54);
    assert_box_lines_aligned(&text, 54);
}

#[test]
fn recommendation_panel_uses_zh_labels_without_translating_commands() {
    let renderer = RatatuiInlineRenderer::with_width(54).with_language(crate::Language::ZhCn);
    let commands = vec!["cat /tmp/cosh-shell-中文-smoke.txt".to_string()];
    let text = renderer
        .recommendation_panel_lines(RecommendationPanelModel {
            commands: &commands,
        })
        .join("\n");

    assert!(text.contains("推荐"), "{text}");
    assert!(
        text.contains("[Copy] [Insert] [Details] - 仅展示"),
        "{text}"
    );
    assert!(
        text.contains("cat /tmp/cosh-shell-中文-smoke.txt"),
        "{text}"
    );
    assert_rendered_width(&text, 54);
    assert_box_lines_aligned(&text, 54);
}

#[test]
fn zh_cards_keep_40_and_80_column_widths() {
    for width in [40, 80] {
        let renderer =
            RatatuiInlineRenderer::with_width(width).with_language(crate::Language::ZhCn);
        let approval = renderer
            .approval_panel_lines(ApprovalPanelModel {
                id: "req-1",
                kind: "tool request",
                risk: "medium",
                reason: None,
                subject: "tool Bash",
                preview_label: "Tool 输入",
                preview: "cat /tmp/cosh-shell-中文-smoke.txt",
                queue_position: 1,
                queue_total: 1,
                next_label: None,
                selected_action: ApprovalPanelAction::Approve,
                expanded: true,
            })
            .join("\n");
        assert!(approval.contains("审批"), "{approval}");
        assert!(approval.contains("/tmp/cosh-shell"), "{approval}");
        assert_rendered_width(&approval, width as usize);
        assert_box_lines_aligned(&approval, width as usize);

        let options = vec!["Green".to_string(), "Blue".to_string()];
        let question = renderer
            .question_panel_lines(QuestionPanelModel {
                id: "q-1",
                question: "Choose 中文 option",
                options: &options,
                selected_option: 0,
                selected_options: &[],
                custom_answer: "",
                allow_free_text: true,
                selection_mode: QuestionSelectionMode::Single,
            })
            .join("\n");
        assert!(question.contains("Agent 问题"), "{question}");
        assert!(question.contains("[1] Green"), "{question}");
        assert_rendered_width(&question, width as usize);
        assert_box_lines_aligned(&question, width as usize);

        let commands = vec!["cat /tmp/cosh-shell-中文-smoke.txt".to_string()];
        let recommendation = renderer
            .recommendation_panel_lines(RecommendationPanelModel {
                commands: &commands,
            })
            .join("\n");
        assert!(recommendation.contains("推荐"), "{recommendation}");
        assert!(
            recommendation.contains("[Copy] [Insert] [Details] - 仅展示"),
            "{recommendation}"
        );
        assert_rendered_width(&recommendation, width as usize);
        assert_box_lines_aligned(&recommendation, width as usize);
    }
}

#[test]
fn recommendation_panel_write_preserves_ratatui_styles_for_terminal_output() {
    let renderer = RatatuiInlineRenderer {
        width: 100,
        plain: false,
        styled: true,
        language: crate::Language::EnUs,
    };
    let commands = vec!["pwd".to_string()];
    let mut output = Vec::new();

    renderer
        .write_recommendation_panel(
            &mut output,
            RecommendationPanelModel {
                commands: &commands,
            },
        )
        .expect("render recommendation panel");

    let text = String::from_utf8(output).expect("utf8 panel");
    let clean = strip_ansi_escape(&text);
    assert!(text.contains("\x1b["), "{text:?}");
    assert!(clean.contains("Recommendations"), "{clean}");
    assert!(clean.contains("1. pwd"), "{clean}");
    assert!(clean.contains("│  1. pwd"), "{clean}");
    assert!(clean.contains("[Copy] [Insert] [Details]"), "{clean}");
}

#[test]
fn plain_recommendation_panel_keeps_display_only_commands() {
    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let commands = vec!["pwd".to_string(), "echo $PATH".to_string()];
    let text = renderer
        .recommendation_panel_lines(RecommendationPanelModel {
            commands: &commands,
        })
        .join("\n");

    assert!(text.contains("Recommendations:"), "{text}");
    assert!(text.contains("  1. pwd"), "{text}");
    assert!(text.contains("  2. echo $PATH"), "{text}");
    assert!(text.contains("[Copy] [Insert] [Details]"), "{text}");
    assert!(text.contains("display-only"), "{text}");
    assert!(!text.contains("/allow N"), "{text}");
    assert!(!text.contains('╭'), "{text}");
    assert_rendered_width(&text, 80);
}

#[test]
fn plain_recommendation_panel_wraps_long_commands_without_dropping_tail() {
    let renderer = RatatuiInlineRenderer::plain_with_width(50);
    let commands = vec![
        "cargo test --package cosh-shell --test raw_cli raw_cli_streaming_tool_approval_renders_before_agent_finishes -- --test-threads=1".to_string(),
    ];
    let text = renderer
        .recommendation_panel_lines(RecommendationPanelModel {
            commands: &commands,
        })
        .join("\n");

    assert!(text.contains("Recommendations:"), "{text}");
    assert!(
        text.contains("  1. cargo test --package cosh-shell --test"),
        "{text}"
    );
    assert!(text.contains("     raw_cli"), "{text}");
    assert!(text.contains("raw_cli_streaming_tool_approval"), "{text}");
    assert!(text.contains("--test-threads=1"), "{text}");
    assert!(text.contains("[Copy] [Insert] [Details]"), "{text}");
    assert!(text.contains("display-only"), "{text}");
    assert!(!text.contains("/allow N"), "{text}");
    assert!(!text.contains('╭'), "{text}");
    assert_rendered_width(&text, 50);
}

#[test]
fn recommendation_action_panel_renders_display_only_receipt() {
    let renderer = RatatuiInlineRenderer::with_width(100);
    let text = renderer
        .recommendation_action_panel_lines(RecommendationActionPanelModel {
            title: "Recommendation selected",
            primary: "Selected recommendation 2".to_string(),
            command: Some("echo $PATH"),
            message: "Display-only: command was not executed; copy or re-enter it to run",
        })
        .join("\n");

    assert!(text.contains("Recommendation selected"), "{text}");
    assert!(text.contains("Selected recommendation 2"), "{text}");
    assert!(text.contains("echo $PATH"), "{text}");
    assert!(
        text.contains("Display-only: command was not executed"),
        "{text}"
    );
    assert_rendered_width(&text, 100);
}

#[test]
fn recommendation_action_panel_wraps_long_receipt_without_dropping_command() {
    let renderer = RatatuiInlineRenderer::with_width(56);
    let text = renderer
        .recommendation_action_panel_lines(RecommendationActionPanelModel {
            title: "Recommendation selected",
            primary: "Selected recommendation with a long display-only command".to_string(),
            command: Some(
                "cargo test --package cosh-shell --test raw_cli raw_cli_selects_recommendation_without_executing_it",
            ),
            message: "Display-only: command was not executed; copy or re-enter it to run",
        })
        .join("\n");

    assert!(text.contains("Selected recommendation"), "{text}");
    assert!(text.contains("cargo test --package cosh-shell"), "{text}");
    assert!(
        text.contains("raw_cli_selects_recommendation_without_executing_it"),
        "{text}"
    );
    assert!(text.contains("command was not executed"), "{text}");
    assert_rendered_width(&text, 56);
}

#[test]
fn recommendation_action_panel_keeps_card_border_aligned_to_renderer_width() {
    let renderer = RatatuiInlineRenderer::with_width(54);
    let text = renderer
        .recommendation_action_panel_lines(RecommendationActionPanelModel {
            title: "Recommendation selected",
            primary: "Selected command with 中文 path and emoji 🧪".to_string(),
            command: Some("cat /tmp/cosh-shell-中文-smoke.txt && echo 🧪 done"),
            message: "Display-only: command was not executed; copy or re-enter it to run",
        })
        .join("\n");

    assert!(text.contains("Recommendation selected"), "{text}");
    assert!(text.contains("中文 path"), "{text}");
    assert!(text.contains("/tmp/cosh-shell-中文-smoke.txt"), "{text}");
    assert_rendered_width(&text, 54);
    assert_box_lines_aligned(&text, 54);
}

#[test]
fn recommendation_action_panel_write_preserves_ratatui_styles_for_terminal_output() {
    let renderer = RatatuiInlineRenderer {
        width: 100,
        plain: false,
        styled: true,
        language: crate::Language::EnUs,
    };
    let mut output = Vec::new();

    renderer
        .write_recommendation_action_panel(
            &mut output,
            RecommendationActionPanelModel {
                title: "Recommendation selected",
                primary: "Selected recommendation 1".to_string(),
                command: Some("pwd"),
                message: "Display-only: command was not executed",
            },
        )
        .expect("render recommendation action panel");

    let text = String::from_utf8(output).expect("utf8 panel");
    let clean = strip_ansi_escape(&text);
    assert!(text.contains("\x1b["), "{text:?}");
    assert!(clean.contains("Recommendation selected"), "{clean}");
    assert!(clean.contains("pwd"), "{clean}");
}

#[test]
fn plain_recommendation_action_panel_keeps_receipt_text() {
    let renderer = RatatuiInlineRenderer::plain_with_width(80);
    let text = renderer
        .recommendation_action_panel_lines(RecommendationActionPanelModel {
            title: "Recommendation selected",
            primary: "Selected recommendation 2".to_string(),
            command: Some("echo $PATH"),
            message: "Display-only: command was not executed",
        })
        .join("\n");

    assert!(text.contains("Recommendation selected:"), "{text}");
    assert!(text.contains("Selected recommendation 2"), "{text}");
    assert!(text.contains("  echo $PATH"), "{text}");
    assert!(text.contains("command was not executed"), "{text}");
    assert!(!text.contains('╭'), "{text}");
    assert_rendered_width(&text, 80);
}

#[test]
fn plain_recommendation_action_panel_wraps_long_receipt() {
    let renderer = RatatuiInlineRenderer::plain_with_width(50);
    let text = renderer
        .recommendation_action_panel_lines(RecommendationActionPanelModel {
            title: "Recommendation selected",
            primary: "Selected recommendation with a long display-only command".to_string(),
            command: Some(
                "cargo test --package cosh-shell --test raw_cli raw_cli_selects_recommendation_without_executing_it",
            ),
            message: "Display-only: command was not executed; copy or re-enter it to run",
        })
        .join("\n");

    assert!(text.contains("Recommendation selected:"), "{text}");
    assert!(
        text.contains("Selected recommendation with a long display-only"),
        "{text}"
    );
    assert!(text.contains("command"), "{text}");
    assert!(
        text.contains("  cargo test --package cosh-shell --test"),
        "{text}"
    );
    assert!(
        text.contains("raw_cli_selects_recommendation_without_executi"),
        "{text}"
    );
    assert!(text.contains("ng_it"), "{text}");
    assert!(
        text.contains("Display-only: command was not executed; copy or"),
        "{text}"
    );
    assert!(text.contains("re-enter it to run"), "{text}");
    assert!(!text.contains('╭'), "{text}");
    assert_rendered_width(&text, 50);
}

#[test]
fn renderer_snapshot_matrix_keeps_box_output_within_width() {
    let markdown = "### 结果\n\
        中文段落 with emoji ✅ and \u{1b}[31mred text\u{1b}[0m should wrap cleanly.\n\n\
        - inspect `/very/long/path/that/should/wrap/without/drifting`\n\
        - run cargo test --package cosh-shell --test raw_cli\n\n\
        ```bash\n\
        cargo test --package cosh-shell --test raw_cli -- --exact raw_cli_dumb_terminal_uses_plain_blocks\n\
        ```";
    let footer = "Commands are suggestions only; nothing was executed automatically.";

    for width in [40, 80, 120] {
        let renderer = RatatuiInlineRenderer::with_width(width);
        let mut output = Vec::new();
        renderer
            .write_agent_response(&mut output, markdown, Some(footer))
            .unwrap();
        let text = String::from_utf8(output).unwrap();

        assert_rendered_width(&text, width as usize);
        assert!(text.contains("red text"));
        assert!(!text.contains("\u{1b}[31m"));
        assert!(text.contains("Commands are suggestions only"));
    }
}

#[test]
fn notice_card_keeps_mode_footer_and_bottom_border() {
    let renderer = RatatuiInlineRenderer::with_width(40);
    let mut output = Vec::new();
    renderer
        .write_notice_panel(
            &mut output,
            NoticePanelModel {
                title: "Approval mode",
                body: vec!["Mode set to auto.".to_string()],
                footer: Some("Only low-risk read-only Bash tools can skip approval; risky requests still ask."),
            },
        )
        .unwrap();
    let text = String::from_utf8(output).unwrap();

    let footer_line = text
        .lines()
        .position(|line| line.contains("still ask."))
        .unwrap_or_else(|| panic!("mode footer should be visible:\n{text}"));
    let bottom_line = text
        .lines()
        .position(|line| line.starts_with('╰'))
        .unwrap_or_else(|| panic!("bottom border should be visible:\n{text}"));

    assert!(text.contains("Mode set to auto."), "{text}");
    assert!(
        text.contains("Only low-risk read-only Bash tools"),
        "{text}"
    );
    assert!(
        footer_line < bottom_line,
        "footer must render before bottom border:\n{text}"
    );
    assert_box_lines_aligned(&text, 40);
}

#[test]
fn renderer_snapshot_matrix_keeps_plain_output_within_width() {
    let body = vec![
        "Phase: requesting".to_string(),
        "中文状态 with emoji ✅ and \u{1b}[32mgreen text\u{1b}[0m should wrap".to_string(),
        "path: /very/long/path/that/should/wrap/without/drifting".to_string(),
    ];

    for width in [40, 80, 120] {
        let renderer = RatatuiInlineRenderer::plain_with_width(width);
        let mut output = Vec::new();
        renderer
            .write_notice_panel(
                &mut output,
                NoticePanelModel {
                    title: "Agent status",
                    body: body.clone(),
                    footer: Some(
                        "Commands are suggestions only; nothing was executed automatically.",
                    ),
                },
            )
            .unwrap();
        let text = String::from_utf8(output).unwrap();

        assert_rendered_width(&text, width as usize);
        assert!(text.contains("green text"));
        assert!(!text.contains("\u{1b}[32m"));
        assert!(!text.contains('╭'));
        assert!(!text.contains('│'));
        assert!(!text.contains('╰'));
    }
}

#[test]
fn streaming_snapshot_keeps_footer_within_width() {
    for width in [40, 80, 120] {
        let renderer = RatatuiInlineRenderer::with_width(width);
        let mut stream = renderer.stream_agent();
        let mut output = Vec::new();

        stream
            .write_delta(
                &mut output,
                "streaming 中文 token with \u{1b}[33mcolored text\u{1b}[0m and a long path /tmp/cosh-shell/snapshot/matrix",
            )
            .unwrap();
        stream
            .finish(
                &mut output,
                Some("Commands are suggestions only; nothing was executed automatically."),
            )
            .unwrap();
        let text = String::from_utf8(output).unwrap();

        assert_rendered_width(&text, width as usize);
        assert!(text.contains("colored"));
        assert!(!text.contains("\u{1b}[33m"));
    }
}

fn assert_rendered_width(output: &str, max_width: usize) {
    for line in output.lines() {
        let width = snapshot_width(line);
        assert!(
            width <= max_width,
            "line width {width} exceeds {max_width}: {line:?}\n{output}"
        );
    }
}

fn assert_box_lines_aligned(output: &str, expected_width: usize) {
    for line in output.lines() {
        let width = snapshot_width(line);
        assert_eq!(
            width, expected_width,
            "box line width {width} differs from {expected_width}: {line:?}\n{output}"
        );
    }
}

fn assert_box_lines_same_width(output: &str) -> usize {
    let expected_width = output
        .lines()
        .find(|line| !line.is_empty())
        .map(snapshot_width)
        .expect("at least one rendered line");
    assert_box_lines_aligned(output, expected_width);
    expected_width
}

fn snapshot_width(line: &str) -> usize {
    line.chars()
        .map(|ch| match ch {
            '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘' | '─' | '│' => 1,
            '•' | '◦' => 1,
            ch if ch.is_control() => 0,
            ch if ch.is_ascii() => 1,
            _ => 2,
        })
        .sum()
}

fn line_index(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("missing {needle:?} in {lines:?}"))
}
