use super::*;
use crate::agent_render::markdown::MarkdownRenderModel;
use ratatui::style::{Color, Modifier};

#[test]
fn markdown_text_does_not_insert_wide_char_placeholders() {
    let lines =
        RatatuiInlineRenderer::with_width(40).markdown_text_lines("你好！我是 Shell 助手。");

    assert!(lines.iter().any(|line| line.contains("你好")));
    assert!(!lines.iter().any(|line| line.contains("你 好")));
}

#[test]
fn markdown_text_joins_soft_wrapped_paragraph_lines() {
    let lines = RatatuiInlineRenderer::with_width(90).markdown_text_lines(
        "The Agent answer spans\nmultiple source lines with 中文内容\nbut should render as one paragraph.",
    );
    let text = lines.join("\n");

    assert!(
        text.contains(
            "The Agent answer spans multiple source lines with 中文内容 but should render as one"
        ),
        "{text}"
    );
    assert!(text.contains("paragraph."), "{text}");
    assert!(!text.contains("spans\nmultiple"), "{text}");
    assert_rendered_width(&text, 86);
}

#[test]
fn markdown_text_renders_fenced_code_as_indented_commands() {
    let lines = RatatuiInlineRenderer::with_width(60).markdown_text_lines(
        "可以运行：\n\n```bash\nif test -d crates; then\n  cargo test --package cosh-shell\nfi\ngit status\n```",
    );

    assert!(lines.iter().any(|line| line == "可以运行："));
    assert!(lines.iter().any(|line| line.starts_with("┌ code: bash")));
    assert!(lines
        .iter()
        .any(|line| line.contains("│ if test -d crates; then")));
    assert!(lines
        .iter()
        .any(|line| line.contains("│   cargo test --package cosh-shell")));
    assert!(lines.iter().any(|line| line.contains("│ fi")));
    assert!(lines.iter().any(|line| line.contains("│ git status")));
    assert!(lines.iter().any(|line| line.starts_with("└")));
    assert!(!lines.iter().any(|line| line.contains("```")));
}

#[test]
fn markdown_text_renders_indented_code_as_code_block() {
    let lines = RatatuiInlineRenderer::with_width(58).markdown_text_lines(
        "建议执行：\n\n    cargo test --package cosh-shell\n    git status --short\n\n完成后继续分析。",
    );
    let text = lines.join("\n");

    assert!(text.contains("建议执行："), "{text}");
    assert!(text.contains("┌ code "), "{text}");
    assert!(text.contains("│ cargo test --package cosh-shell"), "{text}");
    assert!(text.contains("│ git status --short"), "{text}");
    assert!(text.contains("完成后继续分析。"), "{text}");
    assert!(!text.contains("    cargo test"), "{text}");
    assert_rendered_width(&text, 54);
}

#[test]
fn markdown_text_wraps_indented_code_without_dropping_indent() {
    let lines = RatatuiInlineRenderer::with_width(40)
        .markdown_text_lines("```bash\n  echo very-long-shell-token-for-wrapping\n```");
    let text = lines.join("\n");

    assert!(
        text.contains("│   echo very-long-shell-token-for"),
        "{text}"
    );
    assert!(
        text.lines().any(|line| line.contains("│   -wrapping")),
        "{text}"
    );
    assert_rendered_width(&text, 36);
}

#[test]
fn markdown_text_keeps_pipe_tables_readable_in_narrow_width() {
    let markdown = "内存占用 Top 10 分析:\n\
        | 排名 | 进程 | RSS (MB) | 说明 |\n\
        | --- | --- | --- | --- |\n\
        | 1 | Virtualization.VirtualMachine | ~1470 MB | 虚拟机进程，最大内存消耗者 |\n\
        | 2 | Qoder Helper Renderer | ~408 MB | Qoder 渲染进程 |\n\
        关键发现：Qoder 占用最多。";
    let lines = RatatuiInlineRenderer::with_width(54).markdown_text_lines(markdown);
    let text = lines.join("\n");

    assert!(text.contains("内存占用 Top 10 分析:"), "{text}");
    assert!(text.contains("┌ table"), "{text}");
    assert!(text.contains("│排名  进程"), "{text}");
    assert!(text.contains("Virtualizatio"), "{text}");
    assert!(text.contains("VirtualMachine"), "{text}");
    assert!(text.contains("Qoder Helper"), "{text}");
    assert!(text.contains("nderer"), "{text}");
    assert!(text.contains("关键发现：Qoder 占用最多。"), "{text}");
    assert!(
        !text.contains(
            "| 1 | Virtualization.VirtualMachine | ~1470 MB | 虚拟机进程，最大内存消耗者 |"
        ),
        "{text}"
    );
    assert!(!text.contains("| --- | --- | --- | --- |"), "{text}");
    assert_rendered_width(&text, 50);
}

#[test]
fn markdown_agent_table_does_not_overexpand_outer_card_width() {
    let renderer = RatatuiInlineRenderer::with_width(200);
    let mut output = Vec::new();

    renderer
        .write_markdown_text(
            &mut output,
            "内存占用 Top 10 分析:\n\n\
             | 排名 | 进程 | RSS (MB) | 说明 |\n\
             | --- | --- | --- | --- |\n\
             | 1 | Virtualization.VirtualMachine | ~1470 MB | 虚拟机进程，最大内存消耗者 |\n\
             | 2 | ps aux \\| grep cosh | ~42 MB | escaped pipe 应保留在单元格中 |\n\n\
             关键发现：Qoder 占用最多。",
        )
        .expect("render markdown table");

    let text = String::from_utf8(output).expect("utf8 output");
    assert!(text.contains("│ ┌ table"), "{text}");
    assert_rendered_width(&text, 100);
}

#[test]
fn markdown_agent_table_keeps_nested_borders_aligned_with_cjk_and_emoji() {
    let renderer = RatatuiInlineRenderer::with_width(74);
    let mut output = Vec::new();

    renderer
        .write_markdown_text(
            &mut output,
            "系统分析结果:\n\n\
             | 项目 | 状态 | 说明 |\n\
             | --- | --- | --- |\n\
             | CPU 🧪 | 正常 | 负载较低，继续观察 |\n\
             | 中文路径 | /tmp/cosh-shell-中文.md | escaped pipe: ps aux \\| grep cosh |\n\n\
             结论：表格应稳定嵌入 Agent 卡片。",
        )
        .expect("render markdown table");

    let text = String::from_utf8(output).expect("utf8 output");
    assert!(text.contains("│ ┌ table"), "{text}");
    assert!(text.contains("CPU 🧪"), "{text}");
    assert!(text.contains("/tmp/cosh-shell-中文.md"), "{text}");
    assert!(text.contains("ps aux | grep cosh"), "{text}");
    assert!(text.contains("结论：表格应稳定嵌入 Agent 卡片。"), "{text}");
    assert_rendered_width(&text, 74);
    assert!(assert_box_lines_same_width(&text) <= 74, "{text}");
}

#[test]
fn markdown_text_keeps_pipe_output_without_separator_as_text() {
    let markdown = "内存占用 Top 10 分析:\n\
        | 1 | Virtualization.VirtualMachine | ~1470 MB |\n\
        | 2 | Node | ~572 MB |\n\
        关键发现：这些行是 shell 输出，不是 Markdown 表格。";
    let lines = RatatuiInlineRenderer::with_width(100).markdown_text_lines(markdown);
    let text = lines.join("\n");

    assert!(text.contains("内存占用 Top 10 分析:"), "{text}");
    assert!(
        text.contains("| 1 | Virtualization.VirtualMachine | ~1470 MB |"),
        "{text}"
    );
    assert!(text.contains("| 2 | Node | ~572 MB |"), "{text}");
    assert!(
        text.contains("关键发现：这些行是 shell 输出，不是 Markdown 表格。"),
        "{text}"
    );
    assert!(!text.contains("┌ table"), "{text}");
    assert_rendered_width(&text, 96);
}

#[test]
fn markdown_text_table_keeps_escaped_pipes_inside_cells() {
    let markdown = "| 命令 | 说明 |\n\
        | --- | --- |\n\
        | ps aux \\| grep cosh | 管道只作为文本展示 |";
    let lines = RatatuiInlineRenderer::with_width(64).markdown_text_lines(markdown);
    let text = lines.join("\n");

    assert!(text.contains("ps aux | grep cosh"), "{text}");
    assert!(text.contains("管道只作为文本展示"), "{text}");
    assert_eq!(text.matches("ps aux").count(), 1, "{text}");
    assert_rendered_width(&text, 60);
}

#[test]
fn plain_markdown_text_keeps_ascii_table_fallback() {
    let markdown = "| 命令 | 说明 |\n\
        | --- | --- |\n\
        | ps aux \\| grep cosh | 管道只作为文本展示 |";
    let lines = RatatuiInlineRenderer::plain_with_width(64).markdown_text_lines(markdown);
    let text = lines.join("\n");

    assert!(text.contains("+"), "{text}");
    assert!(text.contains("| 命令"), "{text}");
    assert!(text.contains("ps aux | grep cosh"), "{text}");
    assert!(!text.contains("┌ table"), "{text}");
    assert_rendered_width(&text, 60);
}

#[test]
fn plain_markdown_text_keeps_quote_marker_fallback() {
    let lines = RatatuiInlineRenderer::plain_with_width(40)
        .markdown_text_lines("> **注意**: `ls ccc` 没有匹配项");
    let text = lines.join("\n");

    assert!(text.contains("> 注意: ls ccc 没有匹配项"), "{text}");
    assert!(!text.contains("│"), "{text}");
    assert_rendered_width(&text, 40);
}

#[test]
fn markdown_text_compacts_blank_lines_for_shell_scrollback() {
    let lines = RatatuiInlineRenderer::with_width(60)
        .markdown_text_lines("# Title\n\n- first\n\n```bash\necho ok\n```\n\nDone");

    assert_eq!(
        lines,
        vec![
            "Title".to_string(),
            "────────────────────────────────────────────────────────".to_string(),
            "• first".to_string(),
            "┌ code: bash ──────────────────────────────────────────┐".to_string(),
            "│ echo ok                                              │".to_string(),
            "└──────────────────────────────────────────────────────┘".to_string(),
            "Done".to_string(),
        ]
    );
}

#[test]
fn markdown_stream_renders_stable_blocks_before_finish() {
    let renderer = RatatuiInlineRenderer::with_width(80);
    let mut stream = renderer.stream_markdown_agent();
    let mut output = Vec::new();

    stream
        .write_delta(&mut output, "# Project check\n\n- Run `git status`")
        .unwrap();
    let first = String::from_utf8(output.clone()).unwrap();
    assert!(first.contains("╭ Agent"), "{first}");
    assert!(first.contains("│ Project check"), "{first}");
    assert!(first.contains("│ ─────────────────"), "{first}");
    assert!(!first.contains("git status"), "{first}");
    assert!(!first.contains("╰─"), "{first}");

    stream
        .write_delta(
            &mut output,
            "\n\n```bash\ncargo test --package cosh-shell\n```\n\n",
        )
        .unwrap();
    stream.finish(&mut output, None).unwrap();
    let text = String::from_utf8(output).unwrap();

    assert!(text.contains("│ • Run git status"), "{text}");
    assert!(text.contains("│ ┌ code: bash"), "{text}");
    assert!(
        text.contains("│ │ cargo test --package cosh-shell"),
        "{text}"
    );
    assert!(!text.contains("# Project check"), "{text}");
    assert!(!text.contains("```"), "{text}");
    assert!(text.contains("╰─"), "{text}");
}

#[test]
fn markdown_stream_flushes_complete_list_lines_before_finish() {
    let renderer = RatatuiInlineRenderer::with_width(80);
    let mut stream = renderer.stream_markdown_agent();
    let mut output = Vec::new();

    stream
        .write_delta(&mut output, "- `cargo build` — 编译项目\n- `cargo test`")
        .unwrap();
    let first = String::from_utf8(output.clone()).unwrap();

    assert!(first.contains("│ • cargo build — 编译项目"), "{first}");
    assert!(!first.contains("cargo test"), "{first}");
    assert!(!first.contains('`'), "{first}");
    assert!(!first.contains("╰─"), "{first}");
}

#[test]
fn markdown_stream_buffers_soft_wrapped_paragraph_until_blank_or_finish() {
    let renderer = RatatuiInlineRenderer::with_width(90);
    let mut stream = renderer.stream_markdown_agent();
    let mut output = Vec::new();

    stream
        .write_delta(&mut output, "# Streaming paragraph\n\nThis answer starts\n")
        .unwrap();
    let first = String::from_utf8(output.clone()).unwrap();
    assert!(first.contains("│ Streaming paragraph"), "{first}");
    assert!(!first.contains("This answer starts"), "{first}");

    stream
        .write_delta(
            &mut output,
            "and continues on another source line with 中文内容.\n\nDone.",
        )
        .unwrap();
    stream.finish(&mut output, None).unwrap();
    let text = String::from_utf8(output).unwrap();

    assert!(
        text.contains("│ This answer starts and continues on another source line with 中文内容."),
        "{text}"
    );
    assert!(!text.contains("starts\n│ and continues"), "{text}");
    assert!(text.contains("│ Done."), "{text}");
}

#[test]
fn markdown_stream_buffers_table_until_blank_line_or_finish() {
    let renderer = RatatuiInlineRenderer::with_width(90);
    let mut stream = renderer.stream_markdown_agent();
    let mut output = Vec::new();

    stream.write_delta(&mut output, "# Memory\n\n").unwrap();
    stream
        .write_delta(&mut output, "| 排名 | 进程 | RSS |\n")
        .unwrap();
    let first = String::from_utf8(output.clone()).unwrap();
    assert!(first.contains("│ Memory"), "{first}");
    assert!(!first.contains("│ | 排名"), "{first}");
    assert!(!first.contains("│ +"), "{first}");

    stream
        .write_delta(
            &mut output,
            "| --- | --- | --- |\n| 1 | ps aux \\| grep cosh | ~42 MB |\n\nDone.",
        )
        .unwrap();
    stream.finish(&mut output, None).unwrap();
    let text = String::from_utf8(output).unwrap();

    assert!(text.contains("│ ┌ table"), "{text}");
    assert!(text.contains("│ │排名"), "{text}");
    assert!(text.contains("ps aux | grep cosh"), "{text}");
    assert!(text.contains("│ Done."), "{text}");
    assert!(!text.contains("| --- | --- | --- |"), "{text}");
}

#[test]
fn markdown_stream_releases_pipe_output_when_separator_is_missing() {
    let renderer = RatatuiInlineRenderer::with_width(90);
    let mut stream = renderer.stream_markdown_agent();
    let mut output = Vec::new();

    stream
        .write_delta(
            &mut output,
            "Shell output:\n\n| 1 | Virtualization.VirtualMachine | ~1470 MB |\n",
        )
        .unwrap();
    let first = String::from_utf8(output.clone()).unwrap();
    assert!(first.contains("│ Shell output:"), "{first}");
    assert!(!first.contains("Virtualization.VirtualMachine"), "{first}");

    stream
        .write_delta(&mut output, "| 2 | Node | ~572 MB |\n\nDone.")
        .unwrap();
    stream.finish(&mut output, None).unwrap();
    let text = String::from_utf8(output).unwrap();

    assert!(
        text.contains("│ | 1 | Virtualization.VirtualMachine | ~1470 MB |"),
        "{text}"
    );
    assert!(text.contains("│ | 2 | Node | ~572 MB |"), "{text}");
    assert!(text.contains("│ Done."), "{text}");
    assert!(!text.contains("│ ┌ table"), "{text}");
}

#[test]
fn markdown_text_renders_headings_and_block_quotes() {
    let lines = RatatuiInlineRenderer::with_width(34)
        .markdown_text_lines("### 诊断\n> **注意**: `ls ccc` 没有匹配项");

    assert!(lines.iter().any(|line| line == "诊断"));
    assert!(lines.iter().any(|line| line.starts_with("─")));
    assert!(lines
        .iter()
        .any(|line| line.contains("│ 注意: ls ccc 没有匹配项")));
    assert!(!lines.iter().any(|line| line.starts_with("> 注意")));
    assert!(!lines.iter().any(|line| line.contains("###")));
    assert!(!lines.iter().any(|line| line.contains("**")));
}

#[test]
fn markdown_text_prefers_word_boundaries_when_wrapping() {
    let lines = RatatuiInlineRenderer::with_width(42)
        .markdown_text_lines("The workspace command should not split ordinary words.");

    assert!(!lines.iter().any(|line| line.ends_with("wo")));
    assert!(!lines.iter().any(|line| line.starts_with("rkspace")));
}

#[test]
fn markdown_text_strips_simple_inline_markers() {
    let lines = RatatuiInlineRenderer::with_width(60).markdown_text_lines("**原因**: `ccc` 不存在");

    assert!(lines.iter().any(|line| line.contains("原因: ccc 不存在")));
    assert!(!lines.iter().any(|line| line.contains("**")));
    assert!(!lines.iter().any(|line| line.contains('`')));
}

#[test]
fn markdown_render_model_projects_plain_rich_and_styled_outputs() {
    let model = MarkdownRenderModel::parse("**原因**: `ccc` 不存在", 60);

    let rich = model.rich_text_lines().join("\n");
    let plain = model.plain_text_lines().join("\n");
    let styled = model.styled_lines();

    assert!(rich.contains("原因: ccc 不存在"), "{rich}");
    assert!(plain.contains("原因: ccc 不存在"), "{plain}");
    assert!(!rich.contains("**"), "{rich}");
    assert!(!plain.contains('`'), "{plain}");
    assert_eq!(styled.len(), 1, "{styled:?}");

    let spans = &styled[0].spans;
    assert_eq!(spans[0].content.as_ref(), "原因");
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(spans[2].content.as_ref(), "ccc");
    assert_eq!(spans[2].style.fg, Some(Color::Yellow));
    assert!(spans[2].style.add_modifier.contains(Modifier::REVERSED));
}

#[test]
fn markdown_agent_response_styles_inline_bold_and_code_for_terminal_output() {
    let renderer = RatatuiInlineRenderer {
        width: 80,
        plain: false,
        styled: true,
    };
    let mut output = Vec::new();

    renderer
        .write_agent_response(&mut output, "**原因**: `ccc` 不存在", None)
        .expect("render styled markdown");

    let text = String::from_utf8(output).expect("utf8 output");
    let clean = strip_ansi_escape(&text);
    assert!(clean.contains("原因: ccc 不存在"), "{clean}");
    assert!(!clean.contains("**"), "{clean}");
    assert!(!clean.contains('`'), "{clean}");
    assert!(text.contains("\x1b[0;1m原因"), "{text:?}");
    assert!(text.contains("\x1b[0;7;33mccc"), "{text:?}");
}

#[test]
fn markdown_agent_response_keeps_styled_cjk_list_card_aligned() {
    let renderer = RatatuiInlineRenderer {
        width: 118,
        plain: false,
        styled: true,
    };
    let mut output = Vec::new();

    renderer
        .write_agent_response(
            &mut output,
            "看起来你输入的是随意的文字，没有明确的指令。\n\
             没关系！\n\
             你可以告诉我你想做什么，比如：\n\n\
             - \"查看当前目录有什么文件\" - 我会推荐 `ls` 相关命令\n\
             - \"查找某个文件\" - 我会推荐 `find` 或 `fd` 命令\n\
             - \"看看 git 最近的提交\" - 我会推荐 `git log` 命令\n\
             - \"这个项目是做什么的\" - 我可以帮你浏览项目结构\n\n\
             请用自然语言描述你想完成的任务，我来帮你找到合适的命令。",
            None,
        )
        .expect("render styled markdown list");

    let text = String::from_utf8(output).expect("utf8 output");
    let clean = strip_ansi_escape(&text);
    assert!(
        clean.contains("• \"查看当前目录有什么文件\" - 我会推荐 ls 相关命令"),
        "{clean}"
    );
    assert!(
        clean.contains("• \"查找某个文件\" - 我会推荐 find 或 fd 命令"),
        "{clean}"
    );
    assert!(
        clean.contains("• \"看看 git 最近的提交\" - 我会推荐 git log 命令"),
        "{clean}"
    );
    assert!(text.contains("\x1b[0;7;33mls"), "{text:?}");
    assert!(text.contains("\x1b[0;7;33mfind"), "{text:?}");
    assert!(text.contains("\x1b[0;7;33mgit log"), "{text:?}");
    assert_rendered_width(&clean, 118);
    assert!(assert_box_lines_same_width(&clean) <= 118, "{clean}");
}

#[test]
fn markdown_agent_response_keeps_styled_inline_code_and_table_borders_aligned() {
    let renderer = RatatuiInlineRenderer {
        width: 96,
        plain: false,
        styled: true,
    };
    let mut output = Vec::new();

    renderer
        .write_agent_response(
            &mut output,
            "我和普通终端的主要区别：`ls`、`find`、`git log` 会作为建议展示。\n\n\
             | 能力 | 普通终端 | Shell 助手 |\n\
             | --- | --- | --- |\n\
             | 命令执行 | 直接执行，无确认 | 需要你审批后才执行 |\n\
             | 结果分析 | 输出原始文本 | 自动分析输出并给出建议 |\n\n\
             简单说：你告诉我想做什么，我帮你想怎么做、做完帮你看结果。",
            None,
        )
        .expect("render styled markdown table");

    let text = String::from_utf8(output).expect("utf8 output");
    let clean = strip_ansi_escape(&text);
    assert!(clean.contains("│ ┌ table"), "{clean}");
    assert!(clean.contains("普通终端"), "{clean}");
    assert!(clean.contains("Shell 助手"), "{clean}");
    assert!(clean.contains("需要你审批后才执行"), "{clean}");
    assert!(text.contains("\x1b[0;7;33mls"), "{text:?}");
    assert_rendered_width(&clean, 96);
    assert!(assert_box_lines_same_width(&clean) <= 96, "{clean}");
}

#[test]
fn markdown_stream_styles_inline_bold_and_code_for_terminal_output() {
    let renderer = RatatuiInlineRenderer {
        width: 90,
        plain: false,
        styled: true,
    };
    let mut stream = renderer.stream_markdown_agent();
    let mut output = Vec::new();

    stream
        .write_delta(&mut output, "- **Build** with `cargo test`\n\n")
        .expect("write styled stream");
    stream.finish(&mut output, None).expect("finish stream");

    let text = String::from_utf8(output).expect("utf8 output");
    let clean = strip_ansi_escape(&text);
    assert!(clean.contains("• Build with cargo test"), "{clean}");
    assert!(!clean.contains("**"), "{clean}");
    assert!(!clean.contains('`'), "{clean}");
    assert!(text.contains("\x1b[0;1;36m• "), "{text:?}");
    assert!(text.contains("\x1b[0;1mBuild"), "{text:?}");
    assert!(text.contains("\x1b[0;7;33mcargo test"), "{text:?}");
}

#[test]
fn markdown_text_preserves_nested_list_indentation() {
    let lines = RatatuiInlineRenderer::with_width(38).markdown_text_lines(
        "- Check memory\n  - Review resident set size and pressure\n  1. Capture top processes with ps\n1. Summarize findings for the user",
    );
    let text = lines.join("\n");

    assert!(text.contains("• Check memory"), "{text}");
    assert!(
        text.contains("  ◦ Review resident set size") || text.contains("  ◦ Review resident set"),
        "{text}"
    );
    assert!(text.contains("  1. Capture top processes"), "{text}");
    assert!(text.contains("1. Summarize findings"), "{text}");
    assert!(
        text.lines()
            .any(|line| line.starts_with("    ") && line.contains("pressure")),
        "{text}"
    );
    assert_rendered_width(&text, 38);
}

#[test]
fn plain_markdown_text_preserves_markdown_list_markers() {
    let lines = RatatuiInlineRenderer::plain_with_width(38).markdown_text_lines(
        "- Check memory\n  - Review resident set size and pressure\n1. Summarize findings",
    );
    let text = lines.join("\n");

    assert!(text.contains("- Check memory"), "{text}");
    assert!(text.contains("  - Review resident set size"), "{text}");
    assert!(text.contains("1. Summarize findings"), "{text}");
    assert!(!text.contains("• Check memory"), "{text}");
    assert_rendered_width(&text, 38);
}
