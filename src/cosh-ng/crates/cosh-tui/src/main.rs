#![forbid(unsafe_code)]
//! cosh-tui — Interactive TUI for the cosh deterministic interaction layer.
//!
//! Human-facing terminal interface that directly links cosh-platform
//! as a library (no subprocess calls).

mod app;
mod commands;
mod config;
mod llm;
mod logger;
mod session;
mod theme;
mod tools;
mod ui;

use std::io;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use app::{App, AppMode};
use commands::CommandRegistry;

fn main() -> Result<(), io::Error> {
    // Install panic hook so the terminal is restored even on unwind.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and command registry
    let mut app = App::new();
    let registry = CommandRegistry::new();
    app.load_slash_commands(&registry);

    // Run event loop
    let result = run_event_loop(&mut terminal, &mut app, &registry);

    // Auto-save session before exiting
    save_session_on_exit(&app);

    // Restore terminal
    disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    registry: &CommandRegistry,
) -> Result<(), io::Error> {
    while app.running {
        // Check if terminal needs full redraw (e.g. after shell suspend/resume)
        if app.needs_redraw {
            terminal.clear()?;
            app.needs_redraw = false;
        }

        // Poll streaming tokens (non-blocking)
        if app.streaming {
            app.poll_stream();
        }

        terminal.draw(|f| ui::draw(f, app))?;

        // Poll for events with short timeout to keep streaming responsive
        let poll_duration = if app.streaming {
            std::time::Duration::from_millis(16) // ~60fps during streaming
        } else {
            std::time::Duration::from_millis(100)
        };

        if !event::poll(poll_duration)? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            // Only process key press events (ignore release/repeat on some platforms)
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // During streaming, only allow Ctrl+C to abort
            if app.streaming {
                if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
                    app.streaming = false;
                    app.stream_rx = None;
                    app.append_output("\n[Interrupted]");
                    let response = app.streaming_buffer.clone();
                    if !response.is_empty() {
                        app.messages.push(llm::Message::assistant(response));
                    }
                    app.streaming_buffer.clear();
                    // Drop any queued tool work so we don't resume it.
                    app.pending_tool_calls.clear();
                    app.awaiting_approval = None;
                }
                continue;
            }

            // When a tool call is waiting for approval, capture Y/N/Esc only.
            if app.awaiting_approval.is_some() {
                match (key.modifiers, key.code) {
                    (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                        app.approve_pending(false);
                        app.running = false;
                    }
                    (_, KeyCode::Char('y')) | (_, KeyCode::Char('Y')) => {
                        app.approve_pending(true);
                    }
                    (_, KeyCode::Char('n')) | (_, KeyCode::Char('N')) | (_, KeyCode::Esc) => {
                        app.approve_pending(false);
                    }
                    _ => {}
                }
                continue;
            }

            match app.mode {
                AppMode::CommandPalette => handle_palette_key(app, key, registry),
                AppMode::SlashMenu => handle_slash_menu_key(terminal, app, key, registry)?,
                AppMode::Normal => handle_normal_key(terminal, app, key, registry)?,
            }
        }
    }

    Ok(())
}

fn handle_normal_key(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    key: event::KeyEvent,
    registry: &CommandRegistry,
) -> Result<(), io::Error> {
    match (key.modifiers, key.code) {
        // Ctrl+C: exit
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            app.running = false;
        }
        // Ctrl+P: toggle palette
        (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
            app.toggle_palette();
        }
        // Ctrl+O: toggle debug logging
        (KeyModifiers::CONTROL, KeyCode::Char('o')) => {
            let enabled = logger::toggle_debug();
            app.append_output(&format!(
                "[System] Debug logging {}: {}",
                if enabled { "enabled" } else { "disabled" },
                logger::log_dir().display()
            ));
        }
        // Enter: submit input
        //   1. snapshot input, clear input box, echo prompt to scrollback
        //   2. force a redraw so the user sees instant feedback even if the
        //      command (e.g. an LLM call) blocks the event loop afterwards
        //   3. execute the command
        (_, KeyCode::Enter) => {
            // Shift+Enter: insert newline for multi-line input
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.input.push('\n');
                return Ok(());
            }
            let input = app.input.trim().to_string();
            if input.is_empty() {
                return Ok(());
            }
            app.input.clear();
            app.echo_input(&input);
            // Best-effort redraw — ignore draw errors here, the next loop
            // iteration will redraw anyway.
            let _ = terminal.draw(|f| ui::draw(f, app));
            app.execute_command(registry, &input);
        }
        // Up: history up
        (_, KeyCode::Up) => {
            app.history_up();
        }
        // Down: history down
        (_, KeyCode::Down) => {
            app.history_down();
        }
        // Backspace: delete char
        (_, KeyCode::Backspace) => {
            app.input.pop();
        }
        // Regular character input
        (_, KeyCode::Char(c)) => {
            app.input.push(c);
            // When user types '/' as the first character, open the slash menu
            if c == '/' && app.input == "/" {
                app.mode = AppMode::SlashMenu;
                app.slash_selected = 0;
            }
        }
        // Tab: if in normal mode, open palette for auto-complete
        (_, KeyCode::Tab) => {
            app.toggle_palette();
        }
        // Esc: no-op in normal mode
        (_, KeyCode::Esc) => {}
        _ => {}
    }
    Ok(())
}

/// Save the current session to disk before exiting.
fn save_session_on_exit(app: &app::App) {
    if app.history.is_empty() && app.messages.is_empty() {
        return; // Nothing to save
    }

    let metadata = session::SessionMetadata {
        id: app.session_id.clone(),
        name: format!("session-{}", &app.session_id[..8.min(app.session_id.len())]),
        created_at: chrono::Local::now().to_rfc3339(),
        working_dir: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string()),
        command_count: app.command_count,
    };

    let history = session::SessionHistory {
        entries: app.history.iter().map(|h| session::SessionHistoryEntry {
            command: h.command.clone(),
            output: String::new(),
            success: h.success,
            timestamp: h.timestamp.clone(),
        }).collect(),
    };

    if let Err(e) = session::save_session(&metadata, &history) {
        eprintln!("[cosh-tui] Warning: failed to save session: {}", e);
    }
}

fn handle_palette_key(app: &mut App, key: event::KeyEvent, _registry: &CommandRegistry) {
    match (key.modifiers, key.code) {
        // Esc: close palette
        (_, KeyCode::Esc) => {
            app.mode = AppMode::Normal;
            app.filter.clear();
        }
        // Enter: select and execute
        (_, KeyCode::Enter) => {
            let filtered = app.filtered_commands();
            if let Some(cmd) = filtered.get(app.selected_cmd) {
                // Strip template args like <package>, <service>, etc.
                let command_base = cmd
                    .name
                    .split('<')
                    .next()
                    .unwrap_or(&cmd.name)
                    .trim();
                app.input = command_base.to_string();
                app.mode = AppMode::Normal;
                app.filter.clear();
                // Don't auto-execute since templates need args
            }
        }
        // Up: palette up
        (_, KeyCode::Up) => {
            app.palette_up();
        }
        // Down: palette down
        (_, KeyCode::Down) => {
            app.palette_down();
        }
        // Tab: auto-complete (fill input, return to normal)
        (_, KeyCode::Tab) => {
            app.tab_complete();
        }
        // Backspace: delete filter char
        (_, KeyCode::Backspace) => {
            app.filter.pop();
            app.selected_cmd = 0;
        }
        // Ctrl+C: exit
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            app.running = false;
        }
        // Ctrl+P: close palette
        (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
            app.mode = AppMode::Normal;
            app.filter.clear();
        }
        // Regular character: append to filter
        (_, KeyCode::Char(c)) => {
            app.filter.push(c);
            app.selected_cmd = 0;
        }
        _ => {}
    }
}

fn handle_slash_menu_key(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    key: event::KeyEvent,
    registry: &CommandRegistry,
) -> Result<(), io::Error> {
    match (key.modifiers, key.code) {
        // Esc: close slash menu, clear input
        (_, KeyCode::Esc) => {
            app.mode = AppMode::Normal;
            app.input.clear();
            app.slash_selected = 0;
        }
        // Enter: accept selected slash command and execute immediately
        (_, KeyCode::Enter) => {
            app.slash_menu_accept();
            // Execute the slash command now
            let input = app.input.trim().to_string();
            if !input.is_empty() {
                app.input.clear();
                app.echo_input(&input);
                let _ = terminal.draw(|f| ui::draw(f, app));
                app.execute_command(registry, &input);
            }
        }
        // Tab: accept selected slash command into input (don't execute)
        (_, KeyCode::Tab) => {
            app.slash_menu_accept();
        }
        // Up: navigate up
        (_, KeyCode::Up) => {
            app.slash_menu_up();
        }
        // Down: navigate down
        (_, KeyCode::Down) => {
            app.slash_menu_down();
        }
        // Backspace: delete char; if input becomes empty, exit menu
        (_, KeyCode::Backspace) => {
            app.input.pop();
            app.slash_selected = 0;
            if app.input.is_empty() {
                app.mode = AppMode::Normal;
            }
        }
        // Ctrl+C: exit
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            app.running = false;
        }
        // Regular character: append to input to filter commands
        (_, KeyCode::Char(c)) => {
            app.input.push(c);
            app.slash_selected = 0;
            // If filtered list is empty and input is a full match, auto-close
            // (user is typing args); but usually we just stay in menu.
        }
        _ => {}
    }
    Ok(())
}
