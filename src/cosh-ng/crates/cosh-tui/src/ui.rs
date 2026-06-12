//! UI rendering for cosh-tui using ratatui.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::app::{App, AppMode};

/// Render the full TUI layout.
pub fn draw(f: &mut Frame, app: &App) {
    if app.mode == AppMode::CommandPalette {
        draw_palette(f, app);
    } else {
        draw_normal(f, app);
        // Draw slash menu overlay if in SlashMenu mode
        if app.mode == AppMode::SlashMenu {
            draw_slash_menu(f, app);
        }
        // Draw approval banner when a tool call is pending user consent.
        if app.awaiting_approval.is_some() {
            draw_approval_banner(f, app);
        }
    }
}

fn draw_normal(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title bar
            Constraint::Min(5),     // Output area
            Constraint::Length(3),  // History bar
            Constraint::Length(3),  // Input prompt
            Constraint::Length(3),  // Status bar
        ])
        .split(f.area());

    draw_title_bar(f, chunks[0], app);
    draw_output_area(f, chunks[1], app);
    draw_history_bar(f, chunks[2], app);
    draw_input(f, chunks[3], app);
    draw_status_bar(f, chunks[4], app);
}

fn draw_title_bar(f: &mut Frame, area: Rect, app: &App) {
    let status = if app.running { "running" } else { "exiting" };
    let title = Line::from(vec![
        Span::styled(
            " cosh-tui v0.2.0",
            Style::default().add_modifier(Modifier::BOLD).fg(app.theme.info),
        ),
        Span::raw("  "),
        Span::styled(
            format!("[{}]", status),
            Style::default().fg(app.theme.success),
        ),
    ]);
    let block = Block::default().borders(Borders::ALL).style(Style::default().fg(app.theme.info));
    let paragraph = Paragraph::new(title).block(block);
    f.render_widget(paragraph, area);
}

fn draw_output_area(f: &mut Frame, area: Rect, app: &App) {
    let output_text = if app.output.is_empty() && !app.streaming {
        "Welcome to cosh-tui! Type a command or press Ctrl+P for the command palette.\nType 'help' for available commands. Type /help for slash commands.\n\nNatural language is also supported when an LLM API key is configured."
            .to_string()
    } else {
        let mut text = app.output.clone();
        // Append streaming buffer in real-time
        if app.streaming && !app.streaming_buffer.is_empty() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            // Show streaming indicator
            text.push_str("[AI] ");
            text.push_str(&app.streaming_buffer);
            text.push_str(" ▋"); // blinking cursor indicator
        } else if app.streaming {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("[AI] … thinking");
        }
        text
    };

    // Color the output using theme colors with basic markdown support
    let lines: Vec<Line> = output_text
        .lines()
        .map(|line| {
            if line.starts_with("cosh> ") {
                Line::from(Span::styled(line.to_string(), Style::default().fg(app.theme.prompt).add_modifier(Modifier::BOLD)))
            } else if line.starts_with("[AI] ```") || line == "[AI] ```" {
                // Code block delimiter - render dimmed
                Line::from(Span::styled(line.to_string(), Style::default().fg(app.theme.muted)))
            } else if line.starts_with("[AI] # ") {
                // Markdown heading
                let content = &line[5..]; // skip "[AI] "
                Line::from(Span::styled(
                    content.to_string(),
                    Style::default().fg(app.theme.info).add_modifier(Modifier::BOLD),
                ))
            } else if line.starts_with("[AI] ## ") || line.starts_with("[AI] ### ") {
                let content = &line[5..];
                Line::from(Span::styled(
                    content.to_string(),
                    Style::default().fg(app.theme.info).add_modifier(Modifier::BOLD),
                ))
            } else if line.starts_with("[AI] - ") || line.starts_with("[AI] * ") {
                // List items
                let content = &line[5..];
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(content.to_string(), Style::default().fg(app.theme.success)),
                ])
            } else if let Some(content) = line.strip_prefix("[AI] ") {
                // Regular AI response with inline formatting
                let spans = parse_inline_markdown(content, app);
                Line::from(spans)
            } else if line.starts_with("[AI]") {
                Line::from(Span::styled(line.to_string(), Style::default().fg(app.theme.success)))
            } else if line.starts_with("[approval needed] ") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(app.theme.warning).add_modifier(Modifier::BOLD),
                ))
            } else if line.starts_with("[approval]") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(app.theme.warning),
                ))
            } else if line.starts_with("[tool] ") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(app.theme.muted),
                ))
            } else if line.starts_with("[System]") {
                Line::from(Span::styled(line.to_string(), Style::default().fg(app.theme.info)))
            } else if line.starts_with("[pkg] Error") || line.starts_with("[svc] Error") || line.starts_with("[checkpoint] Error") || line.starts_with("Unknown slash command") || line.starts_with("LLM not configured") {
                Line::from(Span::styled(line.to_string(), Style::default().fg(app.theme.error)))
            } else if line.starts_with("[DRY-RUN]") || line.starts_with("Daemon: unavailable") || line.contains("not available") {
                Line::from(Span::styled(line.to_string(), Style::default().fg(app.theme.warning)))
            } else if line.starts_with("Checkpoint created") || line.starts_with("Checkpoint restored") || line.starts_with("Created session directory") || line.starts_with("Theme changed") || line.starts_with("Session renamed") || line.starts_with("Approval mode set") {
                Line::from(Span::styled(line.to_string(), Style::default().fg(app.theme.success)))
            } else if line.starts_with("cosh-tui") || line.starts_with("Slash commands") || line.starts_with("cosh commands") {
                Line::from(Span::styled(line.to_string(), Style::default().fg(app.theme.info).add_modifier(Modifier::BOLD)))
            } else {
                Line::from(Span::raw(line.to_string()))
            }
        })
        .collect();

    // Keep corners (┌┐└┘) but remove left/right vertical bars (│) so the
    // output area is a plain whitespace region — easier to copy text from.
    let output_border_set = border::Set {
        top_left: "┌",
        top_right: "┐",
        bottom_left: "└",
        bottom_right: "┘",
        vertical_left: " ",
        vertical_right: " ",
        horizontal_top: "─",
        horizontal_bottom: "─",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(output_border_set)
        .title(" Output ")
        .style(Style::default().fg(app.theme.highlight));

    // Auto-scroll: keep the most recent content in view.
    // area.height includes the two horizontal borders, so the visible
    // content rows = area.height - 2.
    let visible_rows = area.height.saturating_sub(2);
    let total_lines = lines.len() as u16;
    let scroll_y = total_lines.saturating_sub(visible_rows);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y, 0));
    f.render_widget(paragraph, area);
}

fn draw_history_bar(f: &mut Frame, area: Rect, app: &App) {
    let spans: Vec<Span> = if app.history.is_empty() {
        vec![Span::styled(" History: (empty)", Style::default().fg(app.theme.muted))]
    } else {
        let mut spans = vec![Span::styled(" History: ", Style::default().fg(app.theme.muted))];
        let display_history: Vec<_> = app.history.iter().rev().take(10).collect();
        let total = display_history.len();
        for (i, entry) in display_history.iter().enumerate() {
            let symbol = if entry.success { " ✓" } else { " ✗" };
            let color = if entry.success { app.theme.success } else { app.theme.error };
            spans.push(Span::styled(
                format!("{}{}", entry.command, symbol),
                Style::default().fg(color),
            ));
            if i < total - 1 {
                spans.push(Span::styled(" | ", Style::default().fg(app.theme.muted)));
            }
        }
        spans
    };

    let block = Block::default().borders(Borders::ALL).style(Style::default().fg(app.theme.highlight));
    let paragraph = Paragraph::new(Line::from(spans)).block(block);
    f.render_widget(paragraph, area);
}

fn draw_input(f: &mut Frame, area: Rect, app: &App) {
    let prompt = Line::from(vec![
        Span::styled(" cosh> ", Style::default().fg(app.theme.prompt).add_modifier(Modifier::BOLD)),
        Span::raw(&app.input),
        Span::styled("_", Style::default().fg(app.theme.highlight).add_modifier(Modifier::SLOW_BLINK)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(app.theme.highlight));
    let paragraph = Paragraph::new(prompt).block(block);
    f.render_widget(paragraph, area);
}

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let distro_name = app.distro.display_name();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let home_short = home.replace("/home/", "~/");
    let llm_status_text;
    let llm_status_color;
    if app.llm_client.is_some() {
        llm_status_text = " ⚡ online ";
        llm_status_color = app.theme.success;
    } else {
        llm_status_text = " 🔌 offline (no API key) ";
        llm_status_color = app.theme.warning;
    }

    let auth_type = crate::config::resolve_auth_type(&app.config);
    let model_name = crate::config::resolve_model_name(&app.config);

    let os_icon = if distro_name.starts_with("macOS") { " 🍎 " } else { " 🐧 " };

    let line = Line::from(vec![
        Span::styled(os_icon, Style::default().fg(app.theme.highlight)),
        Span::styled(
            distro_name,
            Style::default().fg(app.theme.success).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(app.theme.muted)),
        Span::styled(home_short, Style::default().fg(app.theme.highlight)),
        Span::styled(" | ", Style::default().fg(app.theme.muted)),
        Span::styled(format!("auth: {}", auth_type), Style::default().fg(app.theme.info)),
        Span::styled(" | ", Style::default().fg(app.theme.muted)),
        Span::styled(format!("model: {}", model_name), Style::default().fg(app.theme.info)),
        Span::styled(" | ", Style::default().fg(app.theme.muted)),
        Span::styled(format!("theme: {}", app.theme.name), Style::default().fg(app.theme.info)),
        Span::styled(" | ", Style::default().fg(app.theme.muted)),
        Span::styled(llm_status_text, Style::default().fg(llm_status_color)),
        Span::styled(" | ", Style::default().fg(app.theme.muted)),
        Span::styled("Ctrl+P: palette", Style::default().fg(app.theme.muted)),
    ]);

    let block = Block::default().borders(Borders::ALL).style(Style::default().fg(app.theme.highlight));
    let paragraph = Paragraph::new(line).block(block);
    f.render_widget(paragraph, area);
}

fn draw_palette(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Filter input
            Constraint::Min(3),     // Command list
        ])
        .split(f.area());

    // Filter input
    let filter_line = Line::from(vec![
        Span::styled(" > ", Style::default().fg(app.theme.prompt).add_modifier(Modifier::BOLD)),
        Span::raw(&app.filter),
        Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
    ]);
    let filter_block = Block::default()
        .borders(Borders::ALL)
        .title(" Command Palette ")
        .style(Style::default().fg(app.theme.info));
    let filter_paragraph = Paragraph::new(filter_line).block(filter_block);
    f.render_widget(filter_paragraph, chunks[0]);

    // Command list
    let filtered = app.filtered_commands();
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, cmd)| {
            let is_selected = i == app.selected_cmd;
            let style = if is_selected {
                Style::default().fg(app.theme.warning).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.highlight)
            };
            let prefix = if is_selected { " ▶ " } else { "   " };

            let line = Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(format!("{:20}", cmd.name), style),
                Span::styled(" - ", Style::default().fg(app.theme.muted)),
                Span::styled(&cmd.description, Style::default().fg(app.theme.muted)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(app.theme.highlight)),
    );

    // We need a mutable ListState for scrolling, but we don't have one in App.
    // For simplicity, render without state-based scrolling.
    f.render_widget(list, chunks[1]);
}

/// Draw the slash command auto-complete menu as a floating popup above the input.
fn draw_slash_menu(f: &mut Frame, app: &App) {
    let filtered = app.filtered_slash_commands();
    if filtered.is_empty() {
        return;
    }

    // Determine popup size: height = number of visible items + 2 (borders), capped at 12
    let max_visible = 12usize;
    let item_count = filtered.len().min(max_visible) as u16;
    let popup_height = item_count + 2;
    let popup_width = 50u16.min(f.area().width.saturating_sub(4));

    // Position the popup just above the input area (input is 3rd from bottom:
    // status=3, input=3, history=3 → input top = area.height - 9).
    let area = f.area();
    let input_top = area.height.saturating_sub(9);
    let popup_y = input_top.saturating_sub(popup_height);
    let popup_x = 1; // left-aligned with a small margin

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area behind the popup
    f.render_widget(Clear, popup_area);

    // Calculate scroll offset to keep selected item visible
    let scroll_offset = if app.slash_selected >= max_visible {
        app.slash_selected - max_visible + 1
    } else {
        0
    };

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(max_visible)
        .map(|(i, cmd)| {
            let is_selected = i == app.slash_selected;
            let style = if is_selected {
                Style::default()
                    .fg(app.theme.warning)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.highlight)
            };
            let prefix = if is_selected { " \u{25b6} " } else { "   " };
            let line = Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(format!("/{}", cmd.name), style),
                Span::styled(" \u{2014} ", Style::default().fg(app.theme.muted)),
                Span::styled(&cmd.description, Style::default().fg(app.theme.muted)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let can_scroll_up = scroll_offset > 0;
    let can_scroll_down = scroll_offset + max_visible < filtered.len();

    let title = if filtered.len() > max_visible {
        format!(" / Commands ({}/{}) ", app.slash_selected + 1, filtered.len())
    } else {
        " / Commands ".to_string()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().fg(app.theme.info));
    let list = List::new(items).block(block);
    f.render_widget(list, popup_area);

    // Draw scroll indicators when there are more items above/below
    if filtered.len() > max_visible {
        let indicator_style = Style::default().fg(app.theme.muted);
        let right_x = popup_area.x + popup_area.width - 2;

        // ▲ arrow at top-right inside border
        if can_scroll_up {
            let up_area = Rect::new(right_x, popup_area.y + 1, 1, 1);
            f.render_widget(
                Paragraph::new("\u{25b2}").style(indicator_style),
                up_area,
            );
        }

        // ▼ arrow at bottom-right inside border
        if can_scroll_down {
            let down_area = Rect::new(right_x, popup_area.y + popup_area.height - 2, 1, 1);
            f.render_widget(
                Paragraph::new("\u{25bc}").style(indicator_style),
                down_area,
            );
        }

        // Scrollbar on the rightmost column (over the border)
        let track_x = popup_area.x + popup_area.width - 1;
        let track_height = item_count as usize;
        if track_height > 2 {
            let total = filtered.len();
            let thumb_size = (track_height * max_visible / total).max(1);
            let thumb_pos = if total <= max_visible {
                0
            } else {
                scroll_offset * (track_height - thumb_size) / (total - max_visible)
            };

            for row in 0..track_height {
                let ch = if row >= thumb_pos && row < thumb_pos + thumb_size {
                    "\u{2588}" // █ full block = thumb
                } else {
                    "\u{2591}" // ░ light shade = track
                };
                let cell_area = Rect::new(track_x, popup_area.y + 1 + row as u16, 1, 1);
                f.render_widget(
                    Paragraph::new(ch).style(Style::default().fg(app.theme.muted)),
                    cell_area,
                );
            }
        }
    }
}

/// Parse inline markdown formatting: **bold**, `code`
fn parse_inline_markdown<'a>(text: &'a str, app: &'a App) -> Vec<Span<'a>> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut plain_start = 0;

    while let Some(&(i, ch)) = chars.peek() {
        if ch == '`' {
            // Inline code
            if i > plain_start {
                spans.push(Span::styled(
                    text[plain_start..i].to_string(),
                    Style::default().fg(app.theme.success),
                ));
            }
            chars.next();
            let code_start = i + 1;
            let mut code_end = code_start;
            while let Some(&(j, c)) = chars.peek() {
                if c == '`' {
                    code_end = j;
                    chars.next();
                    break;
                }
                code_end = j + c.len_utf8();
                chars.next();
            }
            spans.push(Span::styled(
                text[code_start..code_end].to_string(),
                Style::default().fg(app.theme.warning).add_modifier(Modifier::BOLD),
            ));
            plain_start = code_end + 1;
        } else if ch == '*' && text[i..].starts_with("**") {
            // Bold **text**
            if i > plain_start {
                spans.push(Span::styled(
                    text[plain_start..i].to_string(),
                    Style::default().fg(app.theme.success),
                ));
            }
            chars.next(); chars.next(); // skip **
            let bold_start = i + 2;
            let mut bold_end = bold_start;
            while let Some(&(j, c)) = chars.peek() {
                if c == '*' && text[j..].starts_with("**") {
                    bold_end = j;
                    chars.next(); chars.next();
                    break;
                }
                bold_end = j + c.len_utf8();
                chars.next();
            }
            spans.push(Span::styled(
                text[bold_start..bold_end].to_string(),
                Style::default().fg(app.theme.highlight).add_modifier(Modifier::BOLD),
            ));
            plain_start = bold_end + 2;
        } else {
            chars.next();
        }
    }

    // Remaining plain text
    if plain_start < text.len() {
        spans.push(Span::styled(
            text[plain_start..].to_string(),
            Style::default().fg(app.theme.success),
        ));
    }

    if spans.is_empty() {
        spans.push(Span::styled(
            text.to_string(),
            Style::default().fg(app.theme.success),
        ));
    }

    spans
}

/// Render a small floating banner near the bottom of the screen asking the
/// user to approve/deny a pending tool call.
fn draw_approval_banner(f: &mut Frame, app: &App) {
    let call = match app.awaiting_approval.as_ref() {
        Some(c) => c,
        None => return,
    };

    // Resolve a human-readable preview via the registry if possible.
    let args_val = serde_json::from_str::<serde_json::Value>(&call.function.arguments)
        .unwrap_or(serde_json::Value::Null);
    let preview = app
        .tool_registry
        .find(&call.function.name)
        .map(|t| t.preview(&args_val))
        .unwrap_or_else(|| format!("{}({})", call.function.name, call.function.arguments));

    let area = f.area();
    // Width: ~80% of screen, height: 4 rows (2 for content + 2 borders).
    let popup_width = ((area.width as f32) * 0.8) as u16;
    let popup_width = popup_width.clamp(40, area.width.saturating_sub(4));
    let popup_height: u16 = 4;
    // Anchor just above the status bar (status=3, input=3 → 9 rows from bottom).
    let popup_y = area.height.saturating_sub(9 + popup_height);
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);

    let title = format!(" Approve tool call: {} ", call.function.name);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().fg(app.theme.warning));

    let body = vec![
        Line::from(Span::styled(
            preview,
            Style::default().fg(app.theme.highlight).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                "[Y]",
                Style::default().fg(app.theme.success).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" approve   "),
            Span::styled(
                "[N]",
                Style::default().fg(app.theme.error).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" deny   "),
            Span::styled(
                "[Esc]",
                Style::default().fg(app.theme.muted).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]),
    ];

    let paragraph = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
    f.render_widget(paragraph, popup_area);
}
