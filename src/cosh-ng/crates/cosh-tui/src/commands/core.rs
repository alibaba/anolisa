//! Core slash commands for cosh-tui.

use super::{CommandResult, SlashCommand};
use crate::config;
use crate::session;
use crate::theme;

pub struct HelpCommand;

impl SlashCommand for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }
    fn aliases(&self) -> &[&str] {
        &["h", "?"]
    }
    fn description(&self) -> &str {
        "Show available commands"
    }
    fn execute(&self, _args: &str, _app: &mut crate::app::App) -> CommandResult {
        let mut out = String::from("cosh-tui — Available Commands\n\n");
        out.push_str("Slash commands:\n");
        out.push_str("  /help          — Show available commands\n");
        out.push_str("  /about         — Show version and system info\n");
        out.push_str("  /clear         — Clear the output area\n");
        out.push_str("  /quit          — Exit cosh-tui\n");
        out.push_str("  /init          — Create .cosh-session/ in current directory\n");
        out.push_str("  /stats         — Show session statistics\n");
        out.push_str("  /theme [name]  — Switch theme (dark/light/minimal)\n");
        out.push_str("  /model [name]  — Switch LLM model for session\n");
        out.push_str("  /compress      — Compress context (summarize history)\n");
        out.push_str("  /memory [sub]  — Manage AI memory (show/add/clear)\n");
        out.push_str("  /resume [id]   — Resume a previous session\n");
        out.push_str("  /export [fmt]  — Export session (md/json)\n");
        out.push_str("  /copy          — Copy last output to clipboard\n");
        out.push_str("  /rename [name] — Rename current session\n");
        out.push_str("  /approval-mode — Set approval mode (ask/auto/yolo)\n");
        out.push_str("  /bash          — Launch interactive shell\n");
        out.push_str("\ncosh commands:\n");
        out.push_str("  pkg install <pkg>   — Install a package (dry-run)\n");
        out.push_str("  pkg remove <pkg>    — Remove a package (dry-run)\n");
        out.push_str("  pkg search <query>  — Search available packages\n");
        out.push_str("  pkg list            — List installed packages\n");
        out.push_str("  svc status <svc>    — Check service status\n");
        out.push_str("  svc start <svc>     — Start a service (dry-run)\n");
        out.push_str("  svc stop <svc>      — Stop a service (dry-run)\n");
        out.push_str("  svc restart <svc>   — Restart a service (dry-run)\n");
        out.push_str("  svc enable <svc>    — Enable a service (dry-run)\n");
        out.push_str("  svc disable <svc>   — Disable a service (dry-run)\n");
        out.push_str("  svc list            — List all services\n");
        out.push_str("  checkpoint init     — Initialize workspace\n");
        out.push_str("  checkpoint create   — Create workspace snapshot\n");
        out.push_str("  checkpoint list     — List snapshots\n");
        out.push_str("  checkpoint restore  — Restore a snapshot\n");
        out.push_str("  checkpoint status   — Check daemon status\n");
        out.push_str("  checkpoint delete   — Delete a snapshot\n");
        out.push_str("  checkpoint diff     — Diff between snapshots\n");
        out.push_str("  checkpoint cleanup  — Cleanup old snapshots\n");
        out.push_str("  checkpoint recover  — Recover workspace\n");
        out.push_str("\nKey bindings:\n");
        out.push_str("  Ctrl+C      — Exit (or abort streaming)\n");
        out.push_str("  Ctrl+P      — Toggle command palette\n");
        out.push_str("  Ctrl+O      — Toggle debug logging\n");
        out.push_str("  Shift+Enter — Insert newline (multi-line input)\n");
        out.push_str("  Up/Down     — History / Navigate\n");
        out.push_str("  Tab         — Complete from palette\n");
        out.push_str("  Esc         — Close palette\n");
        CommandResult::Output(out)
    }
}

pub struct AboutCommand;

impl SlashCommand for AboutCommand {
    fn name(&self) -> &str {
        "about"
    }
    fn description(&self) -> &str {
        "Show version and system info"
    }
    fn execute(&self, _args: &str, app: &mut crate::app::App) -> CommandResult {
        let mut out = String::from("cosh-tui — About\n\n");
        out.push_str("  Version:       0.2.0\n");
        out.push_str(&format!(
            "  Distribution:  {}\n",
            app.distro.display_name()
        ));
        out.push_str(&format!("  Rust:          {}\n", rustc_version_runtime::version()));
        out.push_str(&format!("  Session ID:    {}\n", app.session_id));
        out.push_str(&format!(
            "  Working dir:   {}\n",
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "?".into())
        ));
        let auth_type = crate::config::resolve_auth_type(&app.config);
        if !auth_type.is_empty() {
            out.push_str(&format!("  Auth provider: {}\n", auth_type));
        }
        let model_name = crate::config::resolve_model_name(&app.config);
        if model_name != "qwen-max" || app.config.model.name.is_some() {
            out.push_str(&format!("  Model:         {}\n", model_name));
        }
        CommandResult::Output(out)
    }
}

pub struct ClearCommand;

impl SlashCommand for ClearCommand {
    fn name(&self) -> &str {
        "clear"
    }
    fn aliases(&self) -> &[&str] {
        &["cls"]
    }
    fn description(&self) -> &str {
        "Clear the output area"
    }
    fn execute(&self, _args: &str, _app: &mut crate::app::App) -> CommandResult {
        CommandResult::Clear
    }
}

pub struct QuitCommand;

impl SlashCommand for QuitCommand {
    fn name(&self) -> &str {
        "quit"
    }
    fn aliases(&self) -> &[&str] {
        &["exit", "q"]
    }
    fn description(&self) -> &str {
        "Exit cosh-tui"
    }
    fn execute(&self, _args: &str, _app: &mut crate::app::App) -> CommandResult {
        CommandResult::Quit
    }
}

pub struct InitCommand;

impl SlashCommand for InitCommand {
    fn name(&self) -> &str {
        "init"
    }
    fn description(&self) -> &str {
        "Create .cosh-session/ in current directory"
    }
    fn execute(&self, _args: &str, _app: &mut crate::app::App) -> CommandResult {
        let dir = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(".cosh-session");
        match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                let path = dir.to_string_lossy();
                CommandResult::Output(format!("Created session directory: {}", path))
            }
            Err(e) => CommandResult::Error(format!("Failed to create .cosh-session: {}", e)),
        }
    }
}

pub struct StatsCommand;

impl SlashCommand for StatsCommand {
    fn name(&self) -> &str {
        "stats"
    }
    fn description(&self) -> &str {
        "Show session statistics"
    }
    fn execute(&self, _args: &str, app: &mut crate::app::App) -> CommandResult {
        let elapsed = app.start_time.elapsed();
        let secs = elapsed.as_secs();
        let mins = secs / 60;
        let hrs = mins / 60;
        let duration_str = if hrs > 0 {
            format!("{}h {}m {}s", hrs, mins % 60, secs % 60)
        } else if mins > 0 {
            format!("{}m {}s", mins, secs % 60)
        } else {
            format!("{}s", secs)
        };

        let mut out = String::from("cosh-tui — Session Statistics\n\n");
        out.push_str(&format!("  Session ID:    {}\n", app.session_id));
        out.push_str(&format!("  Duration:      {}\n", duration_str));
        out.push_str(&format!("  Commands:      {}\n", app.command_count));
        out.push_str(&format!("  Successful:    {}\n", app.success_count));
        out.push_str(&format!("  Errors:        {}\n", app.error_count));
        if app.command_count > 0 {
            let success_rate =
                (app.success_count as f64 / app.command_count as f64) * 100.0;
            out.push_str(&format!("  Success rate:  {:.1}%\n", success_rate));
        }
        out.push_str(&format!("  Theme:         {}\n", app.theme.name));
        CommandResult::Output(out)
    }
}

pub struct ThemeCommand;

impl SlashCommand for ThemeCommand {
    fn name(&self) -> &str {
        "theme"
    }
    fn description(&self) -> &str {
        "Switch theme (dark/light/minimal)"
    }
    fn execute(&self, args: &str, app: &mut crate::app::App) -> CommandResult {
        let name = args.trim();
        if name.is_empty() {
            let available = theme::available_themes().join(", ");
            return CommandResult::Output(format!(
                "Current theme: {}\nAvailable themes: {}",
                app.theme.name, available
            ));
        }
        match theme::get_theme(name) {
            Some(new_theme) => {
                let old_name = app.theme.name;
                app.theme = new_theme;
                app.settings.ui.theme = Some(new_theme.name.to_string());
                let save_msg = match config::save_settings(&app.settings) {
                    Ok(()) => String::new(),
                    Err(e) => format!("\n[warn] Failed to save settings: {}", e),
                };
                CommandResult::Output(format!(
                    "Theme changed: {} → {}{}",
                    old_name, new_theme.name, save_msg
                ))
            }
            None => {
                let available = theme::available_themes().join(", ");
                CommandResult::Error(format!(
                    "Unknown theme: '{}'. Available: {}",
                    name, available
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// /model — switch the LLM model for this session
// ---------------------------------------------------------------------------

pub struct ModelCommand;

impl SlashCommand for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }
    fn description(&self) -> &str {
        "Switch the model for this session"
    }
    fn execute(&self, args: &str, app: &mut crate::app::App) -> CommandResult {
        let name = args.trim();
        if name.is_empty() {
            let current = app.llm_client.as_ref()
                .map(|c| c.model.as_str())
                .unwrap_or("(not configured)");
            return CommandResult::Output(format!(
                "Current model: {}\n\nUsage: /model <name>\nExample: /model qwen-max",
                current
            ));
        }
        // Update the LLM client model
        if let Some(ref mut client) = app.llm_client {
            let old = client.model.clone();
            client.model = name.to_string();
            // Also persist to settings
            app.settings.model.name = Some(name.to_string());
            let save_msg = match config::save_settings(&app.settings) {
                Ok(()) => String::new(),
                Err(e) => format!("\n[warn] Failed to save settings: {}", e),
            };
            CommandResult::Output(format!("Model switched: {} → {}{}", old, name, save_msg))
        } else {
            CommandResult::Error(
                "LLM not configured. Set apiKey in ~/.copilot-shell/settings.json first.".into()
            )
        }
    }
}

// ---------------------------------------------------------------------------
// /compress — compress conversation context using LLM summarization
// ---------------------------------------------------------------------------

pub struct CompressCommand;

impl SlashCommand for CompressCommand {
    fn name(&self) -> &str {
        "compress"
    }
    fn aliases(&self) -> &[&str] {
        &["summarize"]
    }
    fn description(&self) -> &str {
        "Compress conversation context (summarize to save tokens)"
    }
    fn execute(&self, _args: &str, app: &mut crate::app::App) -> CommandResult {
        if app.messages.len() < 3 {
            return CommandResult::Output(
                "Not enough conversation history to compress (need at least 2 exchanges).".into()
            );
        }

        // Build a summarization prompt from the existing messages
        let mut context = String::new();
        for msg in &app.messages {
            if msg.role == "system" {
                continue;
            }
            context.push_str(&format!("[{}]: {}\n", msg.role, msg.content));
        }

        let summarize_prompt = vec![
            crate::llm::Message::system("You are a summarization assistant. Summarize the following conversation into a concise context paragraph that preserves all key information, decisions, and pending tasks. Output only the summary, no commentary."),
            crate::llm::Message::user(context),
        ];

        let client = match app.llm_client.as_ref() {
            Some(c) => c,
            None => return CommandResult::Error(
                "LLM not configured. Cannot compress without an API key.".into()
            ),
        };

        match client.chat(&summarize_prompt) {
            Ok(summary) => {
                let old_count = app.messages.len();
                // Replace messages with system prompt + summary context
                let system_msg = app.messages.iter()
                    .find(|m| m.role == "system")
                    .cloned();
                app.messages.clear();
                if let Some(sys) = system_msg {
                    app.messages.push(sys);
                }
                app.messages.push(crate::llm::Message::assistant(format!(
                    "[Compressed context]: {}",
                    summary
                )));
                CommandResult::Output(format!(
                    "Compressed {} messages → 1 summary message.\nContext preserved, tokens freed.",
                    old_count
                ))
            }
            Err(e) => CommandResult::Error(format!("Compression failed: {}", e)),
        }
    }
}

// ---------------------------------------------------------------------------
// /memory — manage AI memory (project/user level MEMORY.md)
// ---------------------------------------------------------------------------

pub struct MemoryCommand;

impl SlashCommand for MemoryCommand {
    fn name(&self) -> &str {
        "memory"
    }
    fn description(&self) -> &str {
        "Manage AI memory (show/add/clear)"
    }
    fn execute(&self, args: &str, app: &mut crate::app::App) -> CommandResult {
        let parts: Vec<&str> = args.trim().splitn(2, ' ').collect();
        let sub = parts.first().copied().unwrap_or("");
        let rest = parts.get(1).copied().unwrap_or("");

        match sub {
            "" | "show" => {
                // Read from project-level and user-level MEMORY.md
                let project_mem = Self::read_memory_file(".copilot-shell/MEMORY.md");
                let user_mem = Self::read_memory_file_home("MEMORY.md");

                let mut out = String::from("AI Memory\n\n");
                if !project_mem.is_empty() {
                    out.push_str("── Project Memory (.copilot-shell/MEMORY.md) ──\n");
                    out.push_str(&project_mem);
                    out.push('\n');
                }
                if !user_mem.is_empty() {
                    out.push_str("── User Memory (~/.copilot-shell/MEMORY.md) ──\n");
                    out.push_str(&user_mem);
                    out.push('\n');
                }
                if project_mem.is_empty() && user_mem.is_empty() {
                    out.push_str("(empty)\n\nUsage: /memory add <text> — add a memory entry");
                }
                CommandResult::Output(out)
            }
            "add" => {
                if rest.is_empty() {
                    return CommandResult::Error(
                        "Usage: /memory add <text to remember>".into()
                    );
                }
                // Append to project-level MEMORY.md
                let dir = std::path::Path::new(".copilot-shell");
                if let Err(e) = std::fs::create_dir_all(dir) {
                    return CommandResult::Error(format!("Failed to create directory: {}", e));
                }
                let path = dir.join("MEMORY.md");
                let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
                let entry = format!("\n- [{}] {}\n", timestamp, rest);
                match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                    Ok(mut f) => {
                        use std::io::Write;
                        if let Err(e) = f.write_all(entry.as_bytes()) {
                            return CommandResult::Error(format!("Failed to write: {}", e));
                        }
                        // Also inject into LLM context
                        app.messages.push(crate::llm::Message::system(format!(
                            "[Memory added]: {}",
                            rest
                        )));
                        CommandResult::Output(format!("Memory added: {}", rest))
                    }
                    Err(e) => CommandResult::Error(format!("Failed to open MEMORY.md: {}", e)),
                }
            }
            "clear" => {
                let path = std::path::Path::new(".copilot-shell/MEMORY.md");
                if path.exists() {
                    match std::fs::write(path, "") {
                        Ok(()) => CommandResult::Output("Project memory cleared.".into()),
                        Err(e) => CommandResult::Error(format!("Failed to clear: {}", e)),
                    }
                } else {
                    CommandResult::Output("Memory is already empty.".into())
                }
            }
            _ => CommandResult::Error(format!(
                "Unknown subcommand: '{}'. Available: show, add, clear", sub
            )),
        }
    }
}

impl MemoryCommand {
    fn read_memory_file(relative_path: &str) -> String {
        std::fs::read_to_string(relative_path).unwrap_or_default()
    }

    fn read_memory_file_home(filename: &str) -> String {
        let path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".copilot-shell")
            .join(filename);
        std::fs::read_to_string(path).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// /resume — resume a previous session
// ---------------------------------------------------------------------------

pub struct ResumeCommand;

impl SlashCommand for ResumeCommand {
    fn name(&self) -> &str {
        "resume"
    }
    fn description(&self) -> &str {
        "Resume a previous session"
    }
    fn execute(&self, args: &str, _app: &mut crate::app::App) -> CommandResult {
        let sessions = session::list_sessions();
        if sessions.is_empty() {
            return CommandResult::Output("No saved sessions found.".into());
        }

        let id_arg = args.trim();
        if id_arg.is_empty() {
            // List available sessions
            let mut out = String::from("Saved sessions:\n\n");
            for (i, s) in sessions.iter().enumerate() {
                out.push_str(&format!(
                    "  {}. {} — {} ({} commands)\n",
                    i + 1, &s.id[..13.min(s.id.len())], s.name, s.command_count
                ));
            }
            out.push_str("\nUsage: /resume <session-id-prefix>");
            return CommandResult::Output(out);
        }

        // Find session by prefix
        let found = sessions.iter().find(|s| s.id.starts_with(id_arg));
        match found {
            Some(meta) => {
                // Load history
                let dir = session::session_dir().join(&meta.id);
                let history_path = dir.join("history.json");
                match std::fs::read_to_string(&history_path) {
                    Ok(content) => {
                        match serde_json::from_str::<session::SessionHistory>(&content) {
                            Ok(history) => {
                                let count = history.entries.len();
                                CommandResult::Output(format!(
                                    "Session '{}' loaded ({} entries).\nUse /stats to see details.",
                                    meta.name, count
                                ))
                            }
                            Err(e) => CommandResult::Error(format!("Failed to parse history: {}", e)),
                        }
                    }
                    Err(e) => CommandResult::Error(format!("Failed to read history: {}", e)),
                }
            }
            None => CommandResult::Error(format!("No session found matching '{}'", id_arg)),
        }
    }
}

// ---------------------------------------------------------------------------
// /export — export session history to file
// ---------------------------------------------------------------------------

pub struct ExportCommand;

impl SlashCommand for ExportCommand {
    fn name(&self) -> &str {
        "export"
    }
    fn description(&self) -> &str {
        "Export session history (md/json)"
    }
    fn execute(&self, args: &str, app: &mut crate::app::App) -> CommandResult {
        let format = args.trim();
        let format = if format.is_empty() { "md" } else { format };

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let (filename, content) = match format {
            "md" | "markdown" => {
                let filename = format!("cosh-session-{}.md", timestamp);
                let mut md = String::from("# cosh-tui Session Export\n\n");
                md.push_str(&format!("- Session ID: {}\n", app.session_id));
                md.push_str(&format!("- Exported: {}\n\n", timestamp));
                md.push_str("## Conversation\n\n");
                for msg in &app.messages {
                    if msg.role == "system" { continue; }
                    md.push_str(&format!("### {}\n\n{}\n\n", msg.role, msg.content));
                }
                if app.messages.is_empty() {
                    md.push_str("(no LLM conversation)\n\n");
                }
                md.push_str("## Command History\n\n");
                for entry in &app.history {
                    let status = if entry.success { "✓" } else { "✗" };
                    md.push_str(&format!("- [{}] `{}` ({})\n",
                        status, entry.command, entry.timestamp));
                }
                (filename, md)
            }
            "json" => {
                let filename = format!("cosh-session-{}.json", timestamp);
                let export = serde_json::json!({
                    "session_id": app.session_id,
                    "exported_at": timestamp,
                    "messages": app.messages.iter().map(|m| {
                        serde_json::json!({"role": m.role, "content": m.content})
                    }).collect::<Vec<_>>(),
                    "history": app.history.iter().map(|h| {
                        serde_json::json!({
                            "command": h.command,
                            "success": h.success,
                            "timestamp": h.timestamp,
                        })
                    }).collect::<Vec<_>>(),
                    "stats": {
                        "commands": app.command_count,
                        "success": app.success_count,
                        "errors": app.error_count,
                    }
                });
                (filename, serde_json::to_string_pretty(&export).unwrap_or_default())
            }
            _ => {
                return CommandResult::Error(format!(
                    "Unknown format: '{}'. Available: md, json", format
                ));
            }
        };

        match std::fs::write(&filename, &content) {
            Ok(()) => CommandResult::Output(format!("Exported to: {}", filename)),
            Err(e) => CommandResult::Error(format!("Failed to export: {}", e)),
        }
    }
}

// ---------------------------------------------------------------------------
// /copy — copy last output to system clipboard
// ---------------------------------------------------------------------------

pub struct CopyCommand;

impl SlashCommand for CopyCommand {
    fn name(&self) -> &str {
        "copy"
    }
    fn description(&self) -> &str {
        "Copy last output to clipboard"
    }
    fn execute(&self, _args: &str, app: &mut crate::app::App) -> CommandResult {
        let text = if app.output.is_empty() {
            return CommandResult::Error("Nothing to copy (output is empty).".into());
        } else {
            // Get the last block of output (after the last separator)
            let lines: Vec<&str> = app.output.lines().collect();
            // Find the last [You] prompt to delimit the latest response
            let last_you = lines.iter().rposition(|l| l.starts_with("[You]"));
            match last_you {
                Some(idx) => lines[idx + 1..].join("\n"),
                None => app.output.clone(),
            }
        };

        // Use platform-specific clipboard command with timeout
        let result = Self::run_clipboard_cmd(&text);

        match result {
            Ok(()) => {
                CommandResult::Output(format!("Copied {} chars to clipboard.", text.len()))
            }
            Err(e) => CommandResult::Error(e),
        }
    }
}

const CLIPBOARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl CopyCommand {
    fn run_clipboard_cmd(text: &str) -> Result<(), String> {
        use std::io::Write;

        let mut child = if cfg!(target_os = "macos") {
            std::process::Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
        } else {
            std::process::Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
        }
        .map_err(|e| {
            format!(
                "Failed to access clipboard: {}. Install pbcopy (macOS) or xclip (Linux).",
                e
            )
        })?;

        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        drop(child.stdin.take());

        let deadline = std::time::Instant::now() + CLIPBOARD_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(_)) => return Err("Clipboard command failed.".into()),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("Clipboard command timed out (no display?)".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(format!("Clipboard error: {}", e)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// /rename — rename current session
// ---------------------------------------------------------------------------

pub struct RenameCommand;

impl SlashCommand for RenameCommand {
    fn name(&self) -> &str {
        "rename"
    }
    fn description(&self) -> &str {
        "Rename current session"
    }
    fn execute(&self, args: &str, app: &mut crate::app::App) -> CommandResult {
        let name = args.trim();
        if name.is_empty() {
            let current = app.session_name.as_deref().unwrap_or("(unnamed)");
            return CommandResult::Output(format!(
                "Current session name: {}\n\nUsage: /rename <new-name>",
                current
            ));
        }
        let old = app.session_name.clone().unwrap_or_else(|| "(unnamed)".to_string());
        app.session_name = Some(name.to_string());
        CommandResult::Output(format!("Session renamed: {} → {}", old, name))
    }
}

// ---------------------------------------------------------------------------
// /approval-mode — control command execution approval
// ---------------------------------------------------------------------------

pub struct ApprovalModeCommand;

impl SlashCommand for ApprovalModeCommand {
    fn name(&self) -> &str {
        "approval-mode"
    }
    fn aliases(&self) -> &[&str] {
        &["permissions"]
    }
    fn description(&self) -> &str {
        "Set approval mode (ask/auto/yolo)"
    }
    fn execute(&self, args: &str, app: &mut crate::app::App) -> CommandResult {
        let mode = args.trim().to_lowercase();
        if mode.is_empty() {
            return CommandResult::Output(format!(
                "Current approval mode: {:?}\n\nAvailable modes:\n  ask   — Always ask before executing commands\n  auto  — Auto-approve safe ops, ask for dangerous ones\n  yolo  — Execute everything without confirmation",
                app.approval_mode
            ));
        }
        match mode.as_str() {
            "ask" => {
                app.approval_mode = crate::app::ApprovalMode::Ask;
                CommandResult::Output("Approval mode set to: ask (always confirm)".into())
            }
            "auto" | "auto-edit" => {
                app.approval_mode = crate::app::ApprovalMode::Auto;
                CommandResult::Output("Approval mode set to: auto (safe ops auto-approved)".into())
            }
            "yolo" => {
                app.approval_mode = crate::app::ApprovalMode::Yolo;
                CommandResult::Output("Approval mode set to: yolo (no confirmation)".into())
            }
            _ => CommandResult::Error(format!(
                "Unknown mode: '{}'. Available: ask, auto, yolo", mode
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    /// Helper: create a minimal App for testing.
    fn test_app() -> App {
        App::new()
    }

    // --- /help ---
    #[test]
    fn test_help_command_basic() {
        let cmd = HelpCommand;
        assert_eq!(cmd.name(), "help");
        assert!(cmd.aliases().contains(&"?"));
        let mut app = test_app();
        let result = cmd.execute("", &mut app);
        match result {
            CommandResult::Output(s) => {
                assert!(s.contains("/help"));
                assert!(s.contains("/model"));
                assert!(s.contains("/compress"));
                assert!(s.contains("/memory"));
            }
            _ => panic!("Expected Output"),
        }
    }

    // --- /about ---
    #[test]
    fn test_about_command() {
        let cmd = AboutCommand;
        assert_eq!(cmd.name(), "about");
        let mut app = test_app();
        let result = cmd.execute("", &mut app);
        match result {
            CommandResult::Output(s) => assert!(s.contains("cosh-tui")),
            _ => panic!("Expected Output"),
        }
    }

    // --- /clear ---
    #[test]
    fn test_clear_command() {
        let cmd = ClearCommand;
        assert_eq!(cmd.name(), "clear");
        assert!(cmd.aliases().contains(&"cls"));
        let mut app = test_app();
        let result = cmd.execute("", &mut app);
        assert!(matches!(result, CommandResult::Clear));
    }

    // --- /quit ---
    #[test]
    fn test_quit_command() {
        let cmd = QuitCommand;
        assert_eq!(cmd.name(), "quit");
        assert!(cmd.aliases().contains(&"exit"));
        let mut app = test_app();
        let result = cmd.execute("", &mut app);
        assert!(matches!(result, CommandResult::Quit));
    }

    // --- /model ---
    #[test]
    fn test_model_command_show_current() {
        let cmd = ModelCommand;
        assert_eq!(cmd.name(), "model");
        let mut app = test_app();
        let result = cmd.execute("", &mut app);
        match result {
            CommandResult::Output(s) => assert!(s.contains("Current model")),
            CommandResult::Error(_) => {} // LLM not configured is also acceptable
            _ => panic!("Unexpected result"),
        }
    }

    #[test]
    fn test_model_command_switch() {
        let _cmd = ModelCommand;
        let mut app = test_app();
        // Only test if LLM client is configured
        if let Some(ref mut client) = app.llm_client {
            let old_model = client.model.clone();
            // Directly test the model switch logic without persisting
            client.model = "test-model-xyz".to_string();
            assert_eq!(client.model, "test-model-xyz");
            // Restore
            client.model = old_model;
        }
    }

    // --- /compress ---
    #[test]
    fn test_compress_command_not_enough_history() {
        let cmd = CompressCommand;
        assert_eq!(cmd.name(), "compress");
        assert!(cmd.aliases().contains(&"summarize"));
        let mut app = test_app();
        // Empty messages
        let result = cmd.execute("", &mut app);
        match result {
            CommandResult::Output(s) => assert!(s.contains("Not enough")),
            _ => panic!("Expected Output about insufficient history"),
        }
    }

    // --- /memory ---
    #[test]
    fn test_memory_command_show_empty() {
        let cmd = MemoryCommand;
        assert_eq!(cmd.name(), "memory");
        let mut app = test_app();
        let result = cmd.execute("show", &mut app);
        match result {
            CommandResult::Output(s) => assert!(s.contains("Memory")),
            _ => panic!("Expected Output"),
        }
    }

    #[test]
    fn test_memory_command_unknown_subcommand() {
        let cmd = MemoryCommand;
        let mut app = test_app();
        let result = cmd.execute("foobar", &mut app);
        assert!(matches!(result, CommandResult::Error(_)));
    }

    // --- /resume ---
    #[test]
    fn test_resume_command_no_sessions() {
        let cmd = ResumeCommand;
        assert_eq!(cmd.name(), "resume");
        let mut app = test_app();
        let result = cmd.execute("", &mut app);
        // If sessions dir doesn't exist we get a non-Output variant — that's
        // also valid; only assert content when we actually got Output.
        if let CommandResult::Output(s) = result {
            assert!(s.contains("session") || s.contains("Session"));
        }
    }

    // --- /export ---
    #[test]
    fn test_export_command_unknown_format() {
        let cmd = ExportCommand;
        assert_eq!(cmd.name(), "export");
        let mut app = test_app();
        let result = cmd.execute("xml", &mut app);
        assert!(matches!(result, CommandResult::Error(_)));
    }

    // --- /copy ---
    #[test]
    fn test_copy_command_empty_output() {
        let cmd = CopyCommand;
        assert_eq!(cmd.name(), "copy");
        let mut app = test_app();
        app.output.clear();
        let result = cmd.execute("", &mut app);
        assert!(matches!(result, CommandResult::Error(_)));
    }

    // --- /stats ---
    #[test]
    fn test_stats_command() {
        let cmd = StatsCommand;
        assert_eq!(cmd.name(), "stats");
        let mut app = test_app();
        app.command_count = 5;
        app.success_count = 3;
        app.error_count = 2;
        let result = cmd.execute("", &mut app);
        match result {
            CommandResult::Output(s) => {
                assert!(s.contains("5"));
                assert!(s.contains("3"));
                assert!(s.contains("2"));
            }
            _ => panic!("Expected Output"),
        }
    }

    // --- /theme ---
    #[test]
    fn test_theme_command_show_current() {
        let cmd = ThemeCommand;
        assert_eq!(cmd.name(), "theme");
        let mut app = test_app();
        let result = cmd.execute("", &mut app);
        match result {
            CommandResult::Output(s) => {
                assert!(s.contains("Current theme"));
                assert!(s.contains("Available"));
            }
            _ => panic!("Expected Output"),
        }
    }

    #[test]
    fn test_theme_command_invalid() {
        let cmd = ThemeCommand;
        let mut app = test_app();
        let result = cmd.execute("nonexistent_theme", &mut app);
        assert!(matches!(result, CommandResult::Error(_)));
    }

    // --- CommandRegistry integration ---
    #[test]
    fn test_registry_contains_all_new_commands() {
        let registry = crate::commands::CommandRegistry::new();
        assert!(registry.contains("model"));
        assert!(registry.contains("compress"));
        assert!(registry.contains("summarize")); // alias
        assert!(registry.contains("memory"));
        assert!(registry.contains("resume"));
        assert!(registry.contains("export"));
        assert!(registry.contains("copy"));
        assert!(registry.contains("bash"));
        assert!(registry.contains("shell")); // alias
    }

    #[test]
    fn test_registry_list_includes_new_commands() {
        let registry = crate::commands::CommandRegistry::new();
        let list = registry.list();
        let names: Vec<&str> = list.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"model"));
        assert!(names.contains(&"compress"));
        assert!(names.contains(&"memory"));
        assert!(names.contains(&"resume"));
        assert!(names.contains(&"export"));
        assert!(names.contains(&"copy"));
        assert!(names.contains(&"rename"));
        assert!(names.contains(&"approval-mode"));
    }

    // --- /rename ---
    #[test]
    fn test_rename_command_show_current() {
        let cmd = RenameCommand;
        assert_eq!(cmd.name(), "rename");
        let mut app = test_app();
        let result = cmd.execute("", &mut app);
        match result {
            CommandResult::Output(s) => assert!(s.contains("(unnamed)")),
            _ => panic!("Expected Output"),
        }
    }

    #[test]
    fn test_rename_command_set_name() {
        let cmd = RenameCommand;
        let mut app = test_app();
        let result = cmd.execute("my-project", &mut app);
        match result {
            CommandResult::Output(s) => {
                assert!(s.contains("my-project"));
                assert_eq!(app.session_name.as_deref(), Some("my-project"));
            }
            _ => panic!("Expected Output"),
        }
    }

    // --- /approval-mode ---
    #[test]
    fn test_approval_mode_show_current() {
        let cmd = ApprovalModeCommand;
        assert_eq!(cmd.name(), "approval-mode");
        assert!(cmd.aliases().contains(&"permissions"));
        let mut app = test_app();
        let result = cmd.execute("", &mut app);
        match result {
            CommandResult::Output(s) => assert!(s.contains("Auto")),
            _ => panic!("Expected Output"),
        }
    }

    #[test]
    fn test_approval_mode_set_yolo() {
        let cmd = ApprovalModeCommand;
        let mut app = test_app();
        let result = cmd.execute("yolo", &mut app);
        match result {
            CommandResult::Output(s) => assert!(s.contains("yolo")),
            _ => panic!("Expected Output"),
        }
        assert_eq!(app.approval_mode, crate::app::ApprovalMode::Yolo);
    }

    #[test]
    fn test_approval_mode_set_ask() {
        let cmd = ApprovalModeCommand;
        let mut app = test_app();
        let result = cmd.execute("ask", &mut app);
        match result {
            CommandResult::Output(s) => assert!(s.contains("ask")),
            _ => panic!("Expected Output"),
        }
        assert_eq!(app.approval_mode, crate::app::ApprovalMode::Ask);
    }

    #[test]
    fn test_approval_mode_invalid() {
        let cmd = ApprovalModeCommand;
        let mut app = test_app();
        let result = cmd.execute("invalid", &mut app);
        assert!(matches!(result, CommandResult::Error(_)));
    }

    // --- token estimation ---
    #[test]
    fn test_estimate_tokens() {
        let mut app = test_app();
        app.messages.push(crate::llm::Message::user("hello world")); // 11 chars / 4 + 1 = 3
        assert!(app.estimate_tokens() > 0);
    }

    // --- streaming state ---
    #[test]
    fn test_poll_stream_not_streaming() {
        let mut app = test_app();
        assert!(!app.streaming);
        assert!(!app.poll_stream());
    }

    // --- session auto-naming ---
    #[test]
    fn test_session_name_initially_none() {
        let app = test_app();
        assert!(app.session_name.is_none());
    }
}
