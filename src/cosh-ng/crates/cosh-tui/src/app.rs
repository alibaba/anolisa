//! Application state and logic for cosh-tui.

use std::sync::mpsc;
use std::time::Instant;

use cosh_platform::checkpoint::CkptClient;
use cosh_platform::detect::Distro;
use cosh_platform::{pkg, svc};
use serde_json::Value;

use crate::commands::{CommandRegistry, CommandResult};
use crate::config;
use crate::llm;
use crate::theme;
use crate::tools;

/// Application mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    Normal,
    CommandPalette,
    /// Slash command auto-complete menu (shown when input starts with '/').
    SlashMenu,
}

/// A single command history entry.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub command: String,
    pub success: bool,
    pub timestamp: String,
}

/// Metadata about a known command for the palette.
#[derive(Debug, Clone)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    #[allow(dead_code)]
    pub domain: String,
}

/// A slash command entry for the auto-complete menu.
#[derive(Debug, Clone)]
pub struct SlashCmdInfo {
    pub name: String,
    pub description: String,
}

/// Maximum scrollback size in bytes. When exceeded, the oldest content is
/// trimmed at a newline boundary so the user never sees a partial line.
const MAX_SCROLLBACK_BYTES: usize = 512 * 1024; // 512 KiB

/// Main application state.
pub struct App {
    pub input: String,
    pub history: Vec<HistoryEntry>,
    pub output: String,
    pub distro: Distro,
    pub running: bool,
    pub mode: AppMode,
    pub commands: Vec<CommandInfo>,
    pub selected_cmd: usize,
    pub filter: String,
    pub history_index: usize,
    /// Registered slash commands for the auto-complete menu.
    pub slash_commands: Vec<SlashCmdInfo>,
    /// Currently selected index in the slash menu.
    pub slash_selected: usize,
    /// Active color theme.
    pub theme: &'static theme::Theme,
    /// Unique session identifier.
    pub session_id: String,
    /// Session start time.
    pub start_time: Instant,
    /// Number of commands executed in this session.
    pub command_count: usize,
    /// Number of successful commands.
    pub success_count: usize,
    /// Number of failed commands.
    pub error_count: usize,
    /// Settings loaded from ~/.copilot-shell/settings.json
    pub config: config::Settings,
    /// UI settings (same object as config; kept for convenience)
    pub settings: config::Settings,
    /// LLM client (None if not configured).
    pub llm_client: Option<llm::LlmClient>,
    /// Conversation history with the LLM.
    pub messages: Vec<llm::Message>,
    /// Flag: terminal needs full clear/redraw after shell suspend/resume.
    pub needs_redraw: bool,
    /// Streaming state: currently receiving tokens from LLM.
    pub streaming: bool,
    /// Buffer accumulating streaming tokens for the current response.
    pub streaming_buffer: String,
    /// Channel receiver for streaming tokens from background thread.
    pub stream_rx: Option<mpsc::Receiver<StreamToken>>,
    /// Approval mode: ask, auto, yolo.
    pub approval_mode: ApprovalMode,
    /// Session display name (user-set or auto-generated).
    pub session_name: Option<String>,
    /// Registry of tools exposed to the LLM (cosh-cli wrappers + shell).
    pub tool_registry: tools::ToolRegistry,
    /// Queue of tool calls pending execution in the agentic loop.
    pub pending_tool_calls: Vec<llm::ToolCall>,
    /// Tool call currently awaiting user approval (Ask / Auto-unsafe modes).
    pub awaiting_approval: Option<llm::ToolCall>,
    /// Number of LLM-initiated tool turns in the current agentic loop.
    /// Reset when the user sends a new message.
    agent_turn_count: usize,
}

/// Token message from the streaming background thread.
#[derive(Debug)]
pub enum StreamToken {
    /// A content chunk from the LLM.
    Content(String),
    /// Stream finished successfully, optionally with tool calls to execute.
    Done { tool_calls: Vec<llm::ToolCall> },
    /// Stream encountered an error.
    Error(String),
}

/// Approval mode for tool/command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Always ask before executing commands.
    Ask,
    /// Auto-approve safe operations, ask for dangerous ones.
    Auto,
    /// Never ask, execute everything immediately.
    Yolo,
}

impl App {
    /// Create a new App instance with distro detection.
    pub fn new() -> Self {
        let distro = Distro::detect();
        let commands = Self::build_commands();
        let session_id = format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let settings = config::load_settings();
        let theme_name = settings.ui.theme.as_deref().unwrap_or("dark");
        let theme = theme::get_theme(theme_name).unwrap_or(&theme::DARK);
        let llm_client = llm::LlmClient::from_config(&settings);

        Self {
            input: String::new(),
            history: Vec::new(),
            output: String::new(),
            distro,
            running: true,
            mode: AppMode::Normal,
            commands,
            selected_cmd: 0,
            filter: String::new(),
            history_index: 0,
            slash_commands: Vec::new(),
            slash_selected: 0,
            theme,
            session_id,
            start_time: Instant::now(),
            command_count: 0,
            success_count: 0,
            error_count: 0,
            config: settings.clone(),
            settings,
            llm_client,
            messages: Vec::new(),
            needs_redraw: false,
            streaming: false,
            streaming_buffer: String::new(),
            stream_rx: None,
            approval_mode: ApprovalMode::Auto,
            session_name: None,
            tool_registry: tools::ToolRegistry::new(),
            pending_tool_calls: Vec::new(),
            awaiting_approval: None,
            agent_turn_count: 0,
        }
    }

    fn build_commands() -> Vec<CommandInfo> {
        vec![
            CommandInfo {
                name: "pkg install <package>".into(),
                description: "Install a package".into(),
                domain: "pkg".into(),
            },
            CommandInfo {
                name: "pkg remove <package>".into(),
                description: "Remove a package".into(),
                domain: "pkg".into(),
            },
            CommandInfo {
                name: "pkg search <query>".into(),
                description: "Search available packages".into(),
                domain: "pkg".into(),
            },
            CommandInfo {
                name: "pkg list".into(),
                description: "List installed packages".into(),
                domain: "pkg".into(),
            },
            CommandInfo {
                name: "svc status <service>".into(),
                description: "Check service status".into(),
                domain: "svc".into(),
            },
            CommandInfo {
                name: "svc start <service>".into(),
                description: "Start a service".into(),
                domain: "svc".into(),
            },
            CommandInfo {
                name: "svc stop <service>".into(),
                description: "Stop a service".into(),
                domain: "svc".into(),
            },
            CommandInfo {
                name: "svc restart <service>".into(),
                description: "Restart a service".into(),
                domain: "svc".into(),
            },
            CommandInfo {
                name: "svc enable <service>".into(),
                description: "Enable a service on boot".into(),
                domain: "svc".into(),
            },
            CommandInfo {
                name: "svc disable <service>".into(),
                description: "Disable a service on boot".into(),
                domain: "svc".into(),
            },
            CommandInfo {
                name: "svc list".into(),
                description: "List all services".into(),
                domain: "svc".into(),
            },
            CommandInfo {
                name: "checkpoint init".into(),
                description: "Initialize workspace for checkpointing".into(),
                domain: "checkpoint".into(),
            },
            CommandInfo {
                name: "checkpoint create [name]".into(),
                description: "Create workspace snapshot".into(),
                domain: "checkpoint".into(),
            },
            CommandInfo {
                name: "checkpoint list".into(),
                description: "List available snapshots".into(),
                domain: "checkpoint".into(),
            },
            CommandInfo {
                name: "checkpoint restore <id>".into(),
                description: "Restore a snapshot".into(),
                domain: "checkpoint".into(),
            },
            CommandInfo {
                name: "checkpoint status".into(),
                description: "Check daemon status".into(),
                domain: "checkpoint".into(),
            },
            CommandInfo {
                name: "checkpoint delete <id>".into(),
                description: "Delete a snapshot".into(),
                domain: "checkpoint".into(),
            },
            CommandInfo {
                name: "checkpoint diff <from> <to>".into(),
                description: "Show diff between snapshots".into(),
                domain: "checkpoint".into(),
            },
            CommandInfo {
                name: "checkpoint cleanup [keep]".into(),
                description: "Cleanup old snapshots".into(),
                domain: "checkpoint".into(),
            },
            CommandInfo {
                name: "checkpoint recover".into(),
                description: "Recover workspace after crash".into(),
                domain: "checkpoint".into(),
            },
        ]
    }

    /// Toggle between Normal and CommandPalette mode.
    pub fn toggle_palette(&mut self) {
        match self.mode {
            AppMode::Normal | AppMode::SlashMenu => {
                self.mode = AppMode::CommandPalette;
                self.filter = String::new();
                self.selected_cmd = 0;
            }
            AppMode::CommandPalette => {
                self.mode = AppMode::Normal;
                self.filter = String::new();
            }
        }
    }

    /// Populate the slash command list from the registry.
    /// Should be called once after construction.
    pub fn load_slash_commands(&mut self, registry: &crate::commands::CommandRegistry) {
        self.slash_commands = registry
            .list()
            .into_iter()
            .map(|(name, desc)| SlashCmdInfo {
                name: name.to_string(),
                description: desc.to_string(),
            })
            .collect();
    }

    /// Return slash commands matching the current input filter.
    /// The filter is whatever the user typed after '/'.
    pub fn filtered_slash_commands(&self) -> Vec<&SlashCmdInfo> {
        let filter = self.input.strip_prefix('/').unwrap_or("").to_lowercase();
        if filter.is_empty() {
            self.slash_commands.iter().collect()
        } else {
            self.slash_commands
                .iter()
                .filter(|c| {
                    c.name.to_lowercase().contains(&filter)
                        || c.description.to_lowercase().contains(&filter)
                })
                .collect()
        }
    }

    /// Move selection up in the slash menu.
    pub fn slash_menu_up(&mut self) {
        let len = self.filtered_slash_commands().len();
        if len > 0 {
            if self.slash_selected == 0 {
                self.slash_selected = len - 1;
            } else {
                self.slash_selected -= 1;
            }
        }
    }

    /// Move selection down in the slash menu.
    pub fn slash_menu_down(&mut self) {
        let len = self.filtered_slash_commands().len();
        if len > 0 {
            self.slash_selected = (self.slash_selected + 1) % len;
        }
    }

    /// Accept the currently selected slash command into input.
    pub fn slash_menu_accept(&mut self) {
        let filtered = self.filtered_slash_commands();
        if let Some(cmd) = filtered.get(self.slash_selected) {
            self.input = format!("/{}", cmd.name);
        }
        self.mode = AppMode::Normal;
        self.slash_selected = 0;
    }

    /// Return commands matching the current filter (case-insensitive substring).
    pub fn filtered_commands(&self) -> Vec<&CommandInfo> {
        if self.filter.is_empty() {
            self.commands.iter().collect()
        } else {
            let filter_lower = self.filter.to_lowercase();
            self.commands
                .iter()
                .filter(|c| {
                    c.name.to_lowercase().contains(&filter_lower)
                        || c.description.to_lowercase().contains(&filter_lower)
                })
                .collect()
        }
    }

    /// Move selection up in the command palette.
    pub fn palette_up(&mut self) {
        let len = self.filtered_commands().len();
        if len > 0 {
            if self.selected_cmd == 0 {
                self.selected_cmd = len - 1;
            } else {
                self.selected_cmd -= 1;
            }
        }
    }

    /// Move selection down in the command palette.
    pub fn palette_down(&mut self) {
        let len = self.filtered_commands().len();
        if len > 0 {
            self.selected_cmd = (self.selected_cmd + 1) % len;
        }
    }

    /// Navigate history up.
    pub fn history_up(&mut self) {
        if !self.history.is_empty() && self.history_index < self.history.len() {
            self.history_index += 1;
            let idx = self.history.len().saturating_sub(self.history_index);
            self.input = self.history[idx].command.clone();
        }
    }

    /// Navigate history down.
    pub fn history_down(&mut self) {
        if self.history_index > 1 {
            self.history_index -= 1;
            let idx = self.history.len().saturating_sub(self.history_index);
            self.input = self.history[idx].command.clone();
        } else {
            self.history_index = 0;
            self.input.clear();
        }
    }

    /// Tab-complete: fill input with the currently selected palette command.
    pub fn tab_complete(&mut self) {
        if self.mode == AppMode::CommandPalette {
            let filtered = self.filtered_commands();
            if let Some(cmd) = filtered.get(self.selected_cmd) {
                self.input = cmd.name.clone();
                self.mode = AppMode::Normal;
                self.filter = String::new();
            }
        }
    }

    /// Append a single line (or multi-line block) to the scrollback output.
    /// Ensures a single newline separator between previous and new content.
    /// Automatically trims the oldest content when the buffer exceeds
    /// `MAX_SCROLLBACK_BYTES`.
    pub fn append_output(&mut self, text: &str) {
        if !self.output.is_empty() && !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.output.push_str(text);
        self.trim_scrollback();
    }

    /// Trim the scrollback buffer if it exceeds `MAX_SCROLLBACK_BYTES`,
    /// cutting at a newline boundary so partial lines are never shown.
    /// The byte-offset cut is rounded forward to a UTF-8 char boundary
    /// so that multi-byte content (CJK, emoji, accented Latin) does not
    /// trigger a `byte index is not a char boundary` panic in the
    /// downstream slice/drain.
    fn trim_scrollback(&mut self) {
        if self.output.len() <= MAX_SCROLLBACK_BYTES {
            return;
        }
        let mut cut = self.output.len() - MAX_SCROLLBACK_BYTES;
        while cut < self.output.len() && !self.output.is_char_boundary(cut) {
            cut += 1;
        }
        let boundary = self.output[cut..]
            .find('\n')
            .map(|pos| cut + pos + 1)
            .unwrap_or(cut);
        self.output.drain(..boundary);
    }

    /// Echo the user's input into the output area as a prompt line, so the
    /// user immediately sees that their message was submitted.
    pub fn echo_input(&mut self, input: &str) {
        if !self.output.is_empty() {
            if !self.output.ends_with('\n') {
                self.output.push('\n');
            }
            self.output.push('\n');
        }
        self.output.push_str("cosh> ");
        self.output.push_str(input);
        self.trim_scrollback();
    }

    /// Execute a command string. The caller is responsible for clearing
    /// `self.input` and echoing the prompt to the output area before calling
    /// this (so the user sees instant feedback even when this call blocks).
    pub fn execute_command(&mut self, registry: &CommandRegistry, cmd: &str) {
        let cmd = cmd.trim().to_string();
        if cmd.is_empty() {
            return;
        }

        // Route slash commands (starting with /)
        if cmd.starts_with('/') {
            self.execute_slash_command(&cmd, registry);
            return;
        }

        // Legacy exit/quit handling (also available as /quit)
        if cmd == "quit" || cmd == "exit" {
            self.running = false;
            return;
        }

        // Route known cosh commands to the platform layer
        if self.is_cosh_command(&cmd) {
            let start = Instant::now();
            let result = self.dispatch(&cmd);
            let elapsed = start.elapsed().as_millis();

            let success = result.0;
            let output = result.1;

            self.command_count += 1;
            if success {
                self.success_count += 1;
            } else {
                self.error_count += 1;
            }

            self.append_output(&format!("{}\n[{}ms]", output, elapsed));

            self.history.push(HistoryEntry {
                command: cmd,
                success,
                timestamp: format!("{}ms", elapsed),
            });
            self.history_index = 0;
        } else {
            self.handle_natural_language(&cmd);
        }
    }

    /// Check whether the input is a known cosh command.
    fn is_cosh_command(&self, cmd: &str) -> bool {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return false;
        }
        matches!(parts[0], "pkg" | "svc" | "checkpoint" | "ckpt" | "audit" | "help")
    }

    /// Handle natural-language input by routing it to the LLM (streaming).
    fn handle_natural_language(&mut self, input: &str) {
        // Reset agentic loop turn counter for the new user turn.
        self.agent_turn_count = 0;

        if self.llm_client.is_none() {
            self.append_output(
                "LLM not configured. Set apiKey in ~/.copilot-shell/settings.json (security.auth.apiKey)",
            );
            self.error_count += 1;
            self.command_count += 1;
            self.history.push(HistoryEntry {
                command: input.to_string(),
                success: false,
                timestamp: "0ms".to_string(),
            });
            self.history_index = 0;
            return;
        }

        // Auto-name session from first user message
        if self.session_name.is_none() {
            let auto_name = input.chars().take(40).collect::<String>();
            self.session_name = Some(auto_name);
        }

        // Initialize system prompt on first use
        if self.messages.is_empty() {
            let prompt = self.build_system_prompt();
            self.messages.push(llm::Message::system(prompt));
        }

        // Add user message to LLM context
        self.messages.push(llm::Message::user(input));

        // Keep conversation window bounded
        self.trim_messages();

        // Launch the streaming request (with tool specs).
        self.start_llm_stream();

        self.command_count += 1;
        self.history.push(HistoryEntry {
            command: input.to_string(),
            success: true,
            timestamp: "0ms".to_string(),
        });
        self.history_index = 0;
    }

    /// Spawn a streaming LLM request using the current `self.messages` and
    /// the registered tool specs. Used both for the initial user turn and
    /// for continuation after tool execution.
    fn start_llm_stream(&mut self) {
        let client = match self.llm_client.as_ref() {
            Some(c) => c,
            None => return,
        };

        let messages_clone = self.messages.clone();
        let base_url = client.base_url.clone();
        let api_key = client.api_key.clone();
        let model = client.model.clone();
        let timeout_secs = client.timeout_secs;
        let max_retries = client.max_retries;
        let tools_specs = self.tool_registry.to_openai_tools();

        let (tx, rx) = mpsc::channel();
        self.stream_rx = Some(rx);
        self.streaming = true;
        self.streaming_buffer.clear();

        std::thread::spawn(move || {
            let tmp_client = llm::LlmClient {
                base_url,
                api_key,
                model,
                timeout_secs,
                max_retries,
            };
            match tmp_client.chat_stream(&messages_clone, Some(tools_specs)) {
                Ok(mut reader) => loop {
                    match reader.next_token() {
                        Ok(Some(token)) => {
                            if tx.send(StreamToken::Content(token)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {
                            let tool_calls = reader.take_tool_calls();
                            let _ = tx.send(StreamToken::Done { tool_calls });
                            break;
                        }
                        Err(e) => {
                            let _ = tx.send(StreamToken::Error(e));
                            break;
                        }
                    }
                },
                Err(e) => {
                    let _ = tx.send(StreamToken::Error(e));
                }
            }
        });
    }

    /// Poll streaming tokens from the background thread. Returns true if still streaming.
    pub fn poll_stream(&mut self) -> bool {
        if !self.streaming {
            return false;
        }
        let rx = match self.stream_rx.as_ref() {
            Some(r) => r,
            None => return false,
        };

        // Drain all available tokens without blocking
        loop {
            match rx.try_recv() {
                Ok(StreamToken::Content(token)) => {
                    self.streaming_buffer.push_str(&token);
                }
                Ok(StreamToken::Done { tool_calls }) => {
                    self.finish_stream(true, tool_calls);
                    return false;
                }
                Ok(StreamToken::Error(e)) => {
                    self.streaming_buffer.push_str(&format!("\n[Error: {}]", e));
                    self.finish_stream(false, Vec::new());
                    return false;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // No more tokens available right now
                    return true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Thread ended without Done signal
                    self.finish_stream(true, Vec::new());
                    return false;
                }
            }
        }
    }

    /// Finalize a streaming response.
    fn finish_stream(&mut self, success: bool, tool_calls: Vec<llm::ToolCall>) {
        self.streaming = false;
        self.stream_rx = None;

        let response = self.streaming_buffer.clone();
        self.streaming_buffer.clear();

        // Persist the assistant turn (content + any tool_calls it issued).
        if !tool_calls.is_empty() {
            self.messages.push(llm::Message::assistant_tool_calls(
                response.clone(),
                tool_calls.clone(),
            ));
        } else if !response.is_empty() {
            self.messages.push(llm::Message::assistant(response.clone()));
        }

        // Render assistant content (if any) into the scrollback.
        if !response.is_empty() {
            let mut block = String::new();
            for line in response.lines() {
                block.push_str("[AI] ");
                block.push_str(line);
                block.push('\n');
            }
            let block = block.trim_end().to_string();
            self.append_output(&block);
        }

        if success {
            self.success_count += 1;
        } else {
            self.error_count += 1;
        }

        // Queue tool calls for the agentic loop with turn-limit guard.
        if !tool_calls.is_empty() {
            let max_turns = self
                .config
                .model
                .max_session_turns
                .unwrap_or(25)
                .max(1) as usize;
            self.agent_turn_count += 1;
            if self.agent_turn_count > max_turns {
                self.append_output(&format!(
                    "[System] Agentic loop stopped: reached {} tool turns (limit: {}). \
                     Send a new message to continue.",
                    self.agent_turn_count, max_turns
                ));
                self.pending_tool_calls.clear();
                return;
            }
            self.pending_tool_calls = tool_calls;
            self.process_pending_tools();
        }
    }

    /// Drain pending tool calls: execute safe ones under the current approval
    /// mode, or stop at the first tool that requires user consent. When the
    /// queue is fully drained and at least one tool result was recorded, we
    /// re-launch the stream so the LLM can react to the outputs.
    fn process_pending_tools(&mut self) {
        loop {
            if self.awaiting_approval.is_some() {
                return;
            }
            if self.pending_tool_calls.is_empty() {
                // If we just produced tool results, continue the loop by
                // asking the model for the next step.
                let last_is_tool = self
                    .messages
                    .last()
                    .map(|m| m.role == "tool")
                    .unwrap_or(false);
                if last_is_tool {
                    self.start_llm_stream();
                }
                return;
            }

            let call = self.pending_tool_calls.remove(0);
            let args = match serde_json::from_str::<Value>(&call.function.arguments) {
                Ok(v) => v,
                Err(_) => {
                    // Most providers accept empty-object defaults.
                    if call.function.arguments.trim().is_empty() {
                        Value::Object(Default::default())
                    } else {
                        Value::Null
                    }
                }
            };

            let inspection = self
                .tool_registry
                .find(&call.function.name)
                .map(|t| (t.safety_class(&args), t.preview(&args)));

            let (class, preview) = match inspection {
                Some(info) => info,
                None => {
                    let msg = format!("unknown tool: {}", call.function.name);
                    self.append_output(&format!("[tool] {}", msg));
                    self.record_tool_result(&call, msg);
                    continue;
                }
            };

            // Approval matrix (audit-design.md §9.3):
            //   Class\Mode   | Ask | Auto | Yolo
            //   Safe         | ask | run  | run
            //   NeedsApproval| ask | ask  | run
            //   Forbidden    | ask | ask  | ask  ← Yolo's safety net
            //
            // Forbidden NEVER auto-runs. Yolo means "less prompts", not "I
            // accept any consequence" — `rm -rf /` style commands must still
            // require explicit user consent, even under Yolo.
            let allow_auto = match (self.approval_mode.clone(), class) {
                (_, tools::SafetyClass::Forbidden) => false,
                (ApprovalMode::Yolo, _) => true,
                (ApprovalMode::Auto, tools::SafetyClass::Safe) => true,
                (ApprovalMode::Auto, _) => false,
                (ApprovalMode::Ask, _) => false,
            };

            if allow_auto {
                let output = self
                    .tool_registry
                    .find(&call.function.name)
                    .map(|t| t.execute(&args).unwrap_or_else(|e| format!("[error] {}", e)))
                    .unwrap_or_else(|| format!("unknown tool: {}", call.function.name));
                self.render_tool_output(&preview, &output);
                self.record_tool_result(&call, output);
                continue;
            }

            // Needs user approval — park this call and return.
            let banner = match class {
                tools::SafetyClass::Forbidden => {
                    "[approval needed — DANGEROUS] {}, audit policy classified this as Deny"
                }
                _ => "[approval needed] {}",
            };
            self.append_output(&banner.replace("{}", &preview));
            self.append_output("[approval] Press Y to approve, N to deny, Esc to cancel.");
            self.awaiting_approval = Some(call);
            return;
        }
    }

    /// Handle the user's answer to the current approval prompt.
    pub fn approve_pending(&mut self, approved: bool) {
        let call = match self.awaiting_approval.take() {
            Some(c) => c,
            None => return,
        };

        if approved {
            let args = serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or_else(|_| Value::Object(Default::default()));
            let pair = self
                .tool_registry
                .find(&call.function.name)
                .map(|t| {
                    (
                        t.preview(&args),
                        t.execute(&args).unwrap_or_else(|e| format!("[error] {}", e)),
                    )
                });
            match pair {
                Some((preview, output)) => {
                    self.render_tool_output(&preview, &output);
                    self.record_tool_result(&call, output);
                }
                None => {
                    let msg = format!("unknown tool: {}", call.function.name);
                    self.append_output(&format!("[tool] {}", msg));
                    self.record_tool_result(&call, msg);
                }
            }
        } else {
            self.append_output(&format!("[approval] Denied: {}", call.function.name));
            self.record_tool_result(&call, "User denied execution.".to_string());
        }

        // Continue draining the queue (and possibly re-stream).
        self.process_pending_tools();
    }

    /// Append a tool execution trace to the scrollback (preview + truncated output).
    fn render_tool_output(&mut self, preview: &str, output: &str) {
        self.append_output(&format!("[tool] {}", preview));
        const MAX_LINES: usize = 30;
        for (i, line) in output.lines().enumerate() {
            if i >= MAX_LINES {
                self.append_output("[tool] ... (output truncated)");
                break;
            }
            self.append_output(&format!("[tool] {}", line));
        }
    }

    /// Push a tool-result message into the LLM context.
    fn record_tool_result(&mut self, call: &llm::ToolCall, content: String) {
        self.messages.push(llm::Message::tool_result(
            call.id.clone(),
            call.function.name.clone(),
            content,
        ));
    }

    /// Build a comprehensive system prompt.
    fn build_system_prompt(&self) -> String {
        let os_info = self.distro.display_name();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string());
        let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| "localhost".to_string());

        let mut prompt = format!(
            r#"You are cosh (Copilot Shell), a proactive AI agent embedded in a terminal.

Environment:
- OS: {}
- Shell: {}
- User: {}@{}
- Working directory: {}
- Approval mode: {:?}

Agent behavior:
- You have a set of TOOLS (function calls). When the user asks about system
  state, packages, services, workspace, load, processes, files, etc., CALL
  the appropriate tool directly. DO NOT merely suggest a shell command for
  the user to run — execute it via the tool.
- Prefer the structured `cosh_*` tools (cosh_pkg_*, cosh_svc_*,
  cosh_checkpoint_*) over the generic `run_shell_command` whenever both
  apply. The `cosh_*` tools return deterministic JSON.
- Use `run_shell_command` as a fallback for read-only diagnostics not
  covered by cosh_* (e.g. uptime, ps, df, free, cat /etc/..., git status).
- After receiving a tool result, summarize the findings concisely in plain
  language. Do not re-print raw JSON unless the user asks. Chain multiple
  tool calls when needed to answer the question fully.
- For write/destructive operations (pkg install, svc start/stop, checkpoint
  restore), briefly state what you are about to do before calling the tool.
  The host may require explicit user approval regardless.
- Approval modes: `Ask` = every tool needs Y/N; `Auto` = safe read-only
  operations run automatically, mutating ones still ask; `Yolo` = run safe
  and mutating ones automatically, but EXPLICITLY DANGEROUS commands
  (rm -rf, sudo, dd of=/dev/*, mkfs.*, shell metas, etc.) still require
  user approval — Yolo means "fewer prompts", not "no safety net".
- Be concise and direct; avoid narrating obvious steps.
"#,
            os_info, shell, user, hostname, cwd, self.approval_mode
        );

        // Inject memory context if available
        let project_mem = std::fs::read_to_string(".copilot-shell/MEMORY.md").unwrap_or_default();
        if !project_mem.is_empty() {
            prompt.push_str("\nProject Memory:\n");
            prompt.push_str(&project_mem);
            prompt.push('\n');
        }

        prompt
    }

    /// Trim message history to the most recent 20 entries, keeping system prompt.
    /// Also triggers auto-compression when approaching token limits.
    /// After trimming, removes orphaned tool-result messages whose matching
    /// assistant+tool_calls message was already evicted — OpenAI rejects
    /// tool-result messages that reference unknown tool_call IDs.
    fn trim_messages(&mut self) {
        let token_limit = self
            .config
            .model
            .session_token_limit
            .unwrap_or(12000)
            .max(1024) as usize;
        // Auto-compress if token count exceeds threshold
        let token_estimate = self.estimate_tokens();
        if token_estimate > token_limit && self.messages.len() > 6 {
            let has_system = self
                .messages
                .first()
                .map(|m| m.role == "system")
                .unwrap_or(false);
            if has_system && self.messages.len() > 11 {
                let to_remove = self.messages.len() - 11;
                self.messages.drain(1..=to_remove);
            }
            self.fix_orphaned_tool_messages();
            return;
        }

        if self.messages.len() > 20 {
            let has_system = self
                .messages
                .first()
                .map(|m| m.role == "system")
                .unwrap_or(false);
            if has_system && self.messages.len() > 21 {
                let to_remove = self.messages.len() - 21;
                self.messages.drain(1..=to_remove);
            } else if !has_system && self.messages.len() > 20 {
                let to_remove = self.messages.len() - 20;
                self.messages.drain(0..to_remove);
            }
            self.fix_orphaned_tool_messages();
        }
    }

    /// Remove leading tool-result messages that lost their matching
    /// assistant+tool_calls message after a trim operation.
    fn fix_orphaned_tool_messages(&mut self) {
        let start = if self.messages.first().map(|m| m.role == "system").unwrap_or(false) {
            1
        } else {
            0
        };
        while self.messages.len() > start {
            if self.messages[start].role == "tool" {
                self.messages.remove(start);
            } else {
                break;
            }
        }
    }

    /// Estimate total token count of conversation (rough: ~4 chars per token).
    pub fn estimate_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|m| {
                let mut chars = m.content.len();
                if let Some(calls) = &m.tool_calls {
                    for tc in calls {
                        chars += tc.function.name.len() + tc.function.arguments.len();
                    }
                }
                chars / 4 + 1
            })
            .sum()
    }

    /// Execute a slash command (input starting with /).
    fn execute_slash_command(&mut self, cmd: &str, registry: &CommandRegistry) {
        // Strip the leading '/' and split into name + args
        let without_slash = &cmd[1..];
        let parts: Vec<&str> = without_slash.splitn(2, ' ').collect();
        let name = parts[0];
        let args = parts.get(1).copied().unwrap_or("");

        match registry.find(name) {
            Some(command) => {
                let result = command.execute(args, self);
                self.command_count += 1;

                match result {
                    CommandResult::Output(msg) => {
                        self.success_count += 1;
                        self.append_output(&msg);
                    }
                    CommandResult::Error(msg) => {
                        self.error_count += 1;
                        self.append_output(&msg);
                    }
                    CommandResult::Clear => {
                        self.success_count += 1;
                        self.output.clear();
                    }
                    CommandResult::Quit => {
                        self.running = false;
                    }
                    CommandResult::EnterShell => {
                        self.success_count += 1;
                        // Spawn interactive shell (suspend TUI → shell → resume TUI)
                        if let Err(e) = crate::commands::shell::spawn_interactive_shell() {
                            self.append_output(&format!("Shell error: {}", e));
                        } else {
                            self.append_output("Returned from shell.");
                        }
                        self.needs_redraw = true;
                    }
                }

                self.history.push(HistoryEntry {
                    command: cmd.to_string(),
                    success: true,
                    timestamp: "0ms".to_string(),
                });
                self.history_index = 0;
            }
            None => {
                self.error_count += 1;
                self.command_count += 1;
                self.append_output(&format!(
                    "Unknown slash command: /{}. Type /help for available commands.",
                    name
                ));
            }
        }
    }

    /// Dispatch a command string to the appropriate cosh-platform function.
    /// Returns (success, formatted_output).
    fn dispatch(&self, cmd: &str) -> (bool, String) {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return (false, "Empty command".into());
        }

        match parts[0] {
            "pkg" => self.dispatch_pkg(&parts[1..]),
            "svc" => self.dispatch_svc(&parts[1..]),
            "checkpoint" | "ckpt" => self.dispatch_checkpoint(&parts[1..]),
            "audit" => (true, "[audit] The audit subsystem is only available via the CLI.\n  Run: cosh audit check --action <command>\n  Run: cosh audit log".into()),
            "help" => (true, self.format_help()),
            _ => (false, format!("Unknown command: {}. Type 'help' for available commands.", parts[0])),
        }
    }

    fn dispatch_pkg(&self, args: &[&str]) -> (bool, String) {
        if args.is_empty() {
            return (false, "Usage: pkg <install|remove|search|list> [args]".into());
        }

        match args[0] {
            "install" => {
                if args.len() < 2 {
                    return (false, "Usage: pkg install <package>".into());
                }
                let package = args[1];
                // MVP: dry-run mode for safety
                match pkg::pkg_install(&self.distro, package, true) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (
                            true,
                            format!(
                                "[DRY-RUN] Would install package: {}\n\nExpected response:\n{}",
                                package, json
                            ),
                        )
                    }
                    Err(e) => (false, format!("[pkg] Error: {}", e)),
                }
            }
            "remove" => {
                if args.len() < 2 {
                    return (false, "Usage: pkg remove <package>".into());
                }
                let package = args[1];
                match pkg::pkg_remove(&self.distro, package, true) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (
                            true,
                            format!(
                                "[DRY-RUN] Would remove package: {}\n\nExpected response:\n{}",
                                package, json
                            ),
                        )
                    }
                    Err(e) => (false, format!("[pkg] Error: {}", e)),
                }
            }
            "search" => {
                if args.len() < 2 {
                    return (false, "Usage: pkg search <query>".into());
                }
                let query = args[1];
                match pkg::pkg_search(&self.distro, query) {
                    Ok(result) => {
                        if result.packages.is_empty() {
                            (true, format!("No packages found matching '{}'", query))
                        } else {
                            let mut out = format!("Found {} package(s):\n", result.total);
                            for p in &result.packages {
                                let installed_marker = if p.installed { " [installed]" } else { "" };
                                out.push_str(&format!("  {} - {}{}\n", p.name, p.summary, installed_marker));
                            }
                            (true, out)
                        }
                    }
                    Err(e) => (false, format!("[pkg] Error: {}", e)),
                }
            }
            "list" => match pkg::pkg_list(&self.distro, true) {
                Ok(result) => {
                    if result.packages.is_empty() {
                        (true, "No installed packages found".into())
                    } else {
                        let mut out = format!("Installed packages ({} total):\n", result.total);
                        for p in result.packages.iter().take(50) {
                            if p.version.is_empty() {
                                out.push_str(&format!("  {}\n", p.name));
                            } else {
                                out.push_str(&format!("  {} {}\n", p.name, p.version));
                            }
                        }
                        if result.total > 50 {
                            out.push_str(&format!("  ... and {} more\n", result.total - 50));
                        }
                        (true, out)
                    }
                }
                Err(e) => (false, format!("[pkg] Error: {}", e)),
            }
            other => (false, format!("Unknown pkg subcommand: {}. Valid: install, remove, search, list", other)),
        }
    }

    fn dispatch_svc(&self, args: &[&str]) -> (bool, String) {
        if args.is_empty() {
            return (false, "Usage: svc <status|start|stop|restart|enable|disable|list> [args]".into());
        }

        match args[0] {
            "status" => {
                if args.len() < 2 {
                    return (false, "Usage: svc status <service>".into());
                }
                let name = args[1];
                match svc::svc_status(name) {
                    Ok(status) => {
                        let state_str = match &status.state {
                            cosh_types::svc::SvcState::Running => "running".to_string(),
                            cosh_types::svc::SvcState::Stopped => "stopped".to_string(),
                            cosh_types::svc::SvcState::Failed => "failed".to_string(),
                            cosh_types::svc::SvcState::Activating => "activating".to_string(),
                            cosh_types::svc::SvcState::Deactivating => "deactivating".to_string(),
                            cosh_types::svc::SvcState::Unknown(s) => format!("unknown({})", s),
                        };
                        let mut out = format!("Service: {}\n", status.name);
                        out.push_str(&format!("  State:    {}\n", state_str));
                        out.push_str(&format!("  Active:   {}\n", if status.active { "yes" } else { "no" }));
                        out.push_str(&format!("  Enabled:  {}\n", if status.enabled { "yes" } else { "no" }));
                        if let Some(pid) = status.pid {
                            out.push_str(&format!("  PID:      {}\n", pid));
                        }
                        if let Some(desc) = &status.description {
                            out.push_str(&format!("  Desc:     {}\n", desc));
                        }
                        if let Some(mem) = status.memory_bytes {
                            out.push_str(&format!("  Memory:   {} bytes\n", mem));
                        }
                        if !status.recent_logs.is_empty() {
                            out.push_str("  Recent logs:\n");
                            for log in &status.recent_logs {
                                out.push_str(&format!("    {}\n", log));
                            }
                        }
                        (true, out)
                    }
                    Err(e) => (false, format!("[svc] Error: {}", e)),
                }
            }
            "start" | "stop" | "restart" | "enable" | "disable" => {
                if args.len() < 2 {
                    return (false, format!("Usage: svc {} <service>", args[0]));
                }
                let name = args[1];
                let action = args[0];
                match svc::svc_action(name, action, true) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (
                            true,
                            format!(
                                "[DRY-RUN] Would {} service: {}\n\nExpected response:\n{}",
                                action, name, json
                            ),
                        )
                    }
                    Err(e) => (false, format!("[svc] Error: {}", e)),
                }
            }
            "list" => match svc::svc_list(None) {
                Ok(result) => {
                    if result.services.is_empty() {
                        (true, "No services found".into())
                    } else {
                        let mut out = format!("Services ({} total):\n", result.total);
                        for s in &result.services {
                            let state_str = match &s.state {
                                cosh_types::svc::SvcState::Running => "running",
                                cosh_types::svc::SvcState::Stopped => "stopped",
                                cosh_types::svc::SvcState::Failed => "failed",
                                _ => "other",
                            };
                            out.push_str(&format!("  {:30} {:10}\n", s.name, state_str));
                        }
                        (true, out)
                    }
                }
                Err(e) => (false, format!("[svc] Error: {}", e)),
            },
            other => (false, format!("Unknown svc subcommand: {}. Valid: status, start, stop, restart, enable, disable, list", other)),
        }
    }

    fn dispatch_checkpoint(&self, args: &[&str]) -> (bool, String) {
        if args.is_empty() {
            return (false, "Usage: checkpoint <init|create|list|restore|status|delete|diff|cleanup|recover> [args]".into());
        }

        let client = CkptClient::default_path();

        match args[0] {
            "create" => {
                let id = args.get(1).copied().unwrap_or("default");
                let message = args.get(2).copied();
                if !client.is_available() {
                    return (
                        false,
                        "[checkpoint] ws-ckpt daemon is not available.\n  Hint: Run 'systemctl start ws-ckpt' to enable workspace checkpoints".into(),
                    );
                }
                let workspace = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/tmp/workspace".into());
                match client.create(&workspace, id, message, None, false) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (true, format!("Checkpoint created!\n{}", json))
                    }
                    Err(e) => (false, format!("[checkpoint] Error: {}", e)),
                }
            }
            "list" => {
                if !client.is_available() {
                    return (
                        false,
                        "[checkpoint] ws-ckpt daemon is not available.\n  Hint: Run 'systemctl start ws-ckpt' to enable workspace checkpoints".into(),
                    );
                }
                let workspace = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/tmp/workspace".into());
                match client.list(Some(&workspace)) {
                    Ok(result) => {
                        if result.snapshots.is_empty() {
                            (true, "No checkpoints found for current workspace".into())
                        } else {
                            let mut out = format!("Checkpoints ({} total):\n", result.total);
                            for ckpt in &result.snapshots {
                                let pinned = if ckpt.pinned { " [pinned]" } else { "" };
                                let msg = ckpt.message.as_deref().unwrap_or("(no message)");
                                out.push_str(&format!(
                                    "  {} {} {}{}\n",
                                    ckpt.id, ckpt.created_at, msg, pinned
                                ));
                            }
                            (true, out)
                        }
                    }
                    Err(e) => (false, format!("[checkpoint] Error: {}", e)),
                }
            }
            "restore" => {
                if args.len() < 2 {
                    return (false, "Usage: checkpoint restore <id>".into());
                }
                if !client.is_available() {
                    return (
                        false,
                        "[checkpoint] ws-ckpt daemon is not available.\n  Hint: Run 'systemctl start ws-ckpt' to enable workspace checkpoints".into(),
                    );
                }
                let checkpoint_id = args[1];
                let workspace = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/tmp/workspace".into());
                match client.restore(&workspace, checkpoint_id) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (true, format!("Checkpoint restored!\n{}", json))
                    }
                    Err(e) => (false, format!("[checkpoint] Error: {}", e)),
                }
            }
            "status" => {
                if client.is_available() {
                    match client.status(None) {
                        Ok(result) => {
                            let mut out = "Daemon: available\n".to_string();
                            out.push_str(&format!("  Uptime:        {} secs\n", result.uptime_secs));
                            out.push_str(&format!("  Workspaces:    {}\n", result.workspaces.len()));
                            out.push_str(&format!("  FS total:      {} bytes\n", result.fs_total_bytes));
                            out.push_str(&format!("  FS used:       {} bytes\n", result.fs_used_bytes));
                            for ws in &result.workspaces {
                                out.push_str(&format!("    {} ({}) — {} snapshots\n", ws.ws_id, ws.path, ws.snapshot_count));
                            }
                            (true, out)
                        }
                        Err(e) => (false, format!("[checkpoint] Error: {}", e)),
                    }
                } else {
                    (
                        true,
                        "Daemon: unavailable\n  The ws-ckpt daemon is not running.\n  Hint: Run 'systemctl start ws-ckpt' to enable workspace checkpoints".into(),
                    )
                }
            }
            "init" => {
                if !client.is_available() {
                    return (
                        false,
                        "[checkpoint] ws-ckpt daemon is not available.\n  Hint: Run 'systemctl start ws-ckpt' to enable workspace checkpoints".into(),
                    );
                }
                let workspace = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/tmp/workspace".into());
                match client.init(&workspace) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (true, format!("Workspace initialized!\n{}", json))
                    }
                    Err(e) => (false, format!("[checkpoint] Error: {}", e)),
                }
            }
            "recover" => {
                if !client.is_available() {
                    return (
                        false,
                        "[checkpoint] ws-ckpt daemon is not available.\n  Hint: Run 'systemctl start ws-ckpt' to enable workspace checkpoints".into(),
                    );
                }
                let workspace = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/tmp/workspace".into());
                match client.recover(&workspace) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (true, format!("Workspace recovered!\n{}", json))
                    }
                    Err(e) => (false, format!("[checkpoint] Error: {}", e)),
                }
            }
            "delete" => {
                if args.len() < 2 {
                    return (false, "Usage: checkpoint delete <snapshot-id>".into());
                }
                if !client.is_available() {
                    return (
                        false,
                        "[checkpoint] ws-ckpt daemon is not available.\n  Hint: Run 'systemctl start ws-ckpt' to enable workspace checkpoints".into(),
                    );
                }
                let snapshot_id = args[1];
                let workspace = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/tmp/workspace".into());
                match client.delete(Some(&workspace), snapshot_id, true) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (true, format!("Snapshot deleted!\n{}", json))
                    }
                    Err(e) => (false, format!("[checkpoint] Error: {}", e)),
                }
            }
            "diff" => {
                if args.len() < 3 {
                    return (false, "Usage: checkpoint diff <from-id> <to-id>".into());
                }
                if !client.is_available() {
                    return (
                        false,
                        "[checkpoint] ws-ckpt daemon is not available.\n  Hint: Run 'systemctl start ws-ckpt' to enable workspace checkpoints".into(),
                    );
                }
                let from_id = args[1];
                let to_id = args[2];
                let workspace = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/tmp/workspace".into());
                match client.diff(&workspace, from_id, to_id) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (true, format!("Checkpoint diff:\n{}", json))
                    }
                    Err(e) => (false, format!("[checkpoint] Error: {}", e)),
                }
            }
            "cleanup" => {
                if !client.is_available() {
                    return (
                        false,
                        "[checkpoint] ws-ckpt daemon is not available.\n  Hint: Run 'systemctl start ws-ckpt' to enable workspace checkpoints".into(),
                    );
                }
                let workspace = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/tmp/workspace".into());
                let keep = args.get(1).and_then(|s| s.parse::<u32>().ok());
                match client.cleanup(&workspace, keep) {
                    Ok(result) => {
                        let json = serde_json::to_string_pretty(&result).unwrap_or_default();
                        (true, format!("Cleanup complete!\n{}", json))
                    }
                    Err(e) => (false, format!("[checkpoint] Error: {}", e)),
                }
            }
            other => (false, format!("Unknown checkpoint subcommand: {}. Valid: init, create, list, restore, status, delete, diff, cleanup, recover", other)),
        }
    }

    fn format_help(&self) -> String {
        let mut out = String::from("cosh-tui — Available Commands\n\n");
        out.push_str("  pkg install <package>  — Install a package (dry-run)\n");
        out.push_str("  pkg remove <package>   — Remove a package (dry-run)\n");
        out.push_str("  pkg search <query>     — Search available packages\n");
        out.push_str("  pkg list               — List installed packages\n");
        out.push_str("  svc status <service>   — Check service status\n");
        out.push_str("  svc start <service>    — Start a service (dry-run)\n");
        out.push_str("  svc stop <service>     — Stop a service (dry-run)\n");
        out.push_str("  svc restart <service>  — Restart a service (dry-run)\n");
        out.push_str("  svc enable <service>   — Enable a service (dry-run)\n");
        out.push_str("  svc disable <service>  — Disable a service (dry-run)\n");
        out.push_str("  svc list               — List all services\n");
        out.push_str("  checkpoint init        — Initialize workspace for checkpointing\n");
        out.push_str("  checkpoint create [name] — Create workspace snapshot\n");
        out.push_str("  checkpoint list        — List available snapshots\n");
        out.push_str("  checkpoint restore <id> — Restore a snapshot\n");
        out.push_str("  checkpoint status      — Check daemon status\n");
        out.push_str("  checkpoint delete <id> — Delete a snapshot\n");
        out.push_str("  checkpoint diff <a> <b> — Show diff between snapshots\n");
        out.push_str("  checkpoint cleanup [n] — Cleanup old snapshots (keep n)\n");
        out.push_str("  checkpoint recover     — Recover workspace after crash\n");
        out.push_str("  help                   — Show this help\n");
        out.push_str("  quit / exit            — Exit cosh-tui\n\n");
        out.push_str("Key bindings:\n");
        out.push_str("  Ctrl+P  — Toggle command palette\n");
        out.push_str("  Up/Down — History (Normal) / Navigate (Palette)\n");
        out.push_str("  Tab     — Complete from palette\n");
        out.push_str("  Esc     — Close palette\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{FunctionCall, ToolCall};

    /// Build an App with llm_client forced off so the agentic loop never
    /// triggers a real HTTP call in tests.
    fn test_app() -> App {
        let mut app = App::new();
        app.llm_client = None;
        app
    }

    fn mk_call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{}", name),
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    #[test]
    fn process_auto_mode_executes_safe_shell_tool() {
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Auto;
        app.pending_tool_calls.push(mk_call(
            "run_shell_command",
            r#"{"command":"echo cosh-auto-test"}"#,
        ));
        app.process_pending_tools();

        assert!(app.pending_tool_calls.is_empty());
        assert!(app.awaiting_approval.is_none());
        let last = app.messages.last().expect("tool result recorded");
        assert_eq!(last.role, "tool");
        assert_eq!(last.name.as_deref(), Some("run_shell_command"));
        assert!(last.content.contains("cosh-auto-test"));
    }

    #[test]
    fn process_auto_mode_parks_unsafe_shell_tool() {
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Auto;
        app.pending_tool_calls.push(mk_call(
            "run_shell_command",
            r#"{"command":"touch /tmp/cosh-never-created-xyz"}"#,
        ));
        app.process_pending_tools();

        assert!(app.awaiting_approval.is_some());
        assert!(app.pending_tool_calls.is_empty());
        // No tool result recorded yet — waiting for user.
        assert!(
            app.messages
                .last()
                .map(|m| m.role != "tool")
                .unwrap_or(true)
        );
    }

    #[test]
    fn process_ask_mode_parks_every_call() {
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Ask;
        app.pending_tool_calls.push(mk_call(
            "run_shell_command",
            r#"{"command":"uptime"}"#,
        ));
        app.process_pending_tools();

        assert!(app.awaiting_approval.is_some());
    }

    #[test]
    fn process_yolo_mode_executes_without_approval() {
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Yolo;
        app.pending_tool_calls.push(mk_call(
            "run_shell_command",
            r#"{"command":"echo cosh-yolo-ok"}"#,
        ));
        app.process_pending_tools();

        assert!(app.awaiting_approval.is_none());
        let last = app.messages.last().expect("tool result recorded");
        assert_eq!(last.role, "tool");
        assert!(last.content.contains("cosh-yolo-ok"));
    }

    #[test]
    fn approve_pending_executes_queued_tool() {
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Ask;
        app.awaiting_approval = Some(mk_call(
            "run_shell_command",
            r#"{"command":"echo cosh-approved"}"#,
        ));
        app.approve_pending(true);

        assert!(app.awaiting_approval.is_none());
        let last = app.messages.last().expect("tool result recorded");
        assert_eq!(last.role, "tool");
        assert!(last.content.contains("cosh-approved"));
    }

    #[test]
    fn approve_pending_deny_records_denial() {
        let mut app = test_app();
        app.awaiting_approval = Some(mk_call(
            "run_shell_command",
            r#"{"command":"rm -rf /"}"#,
        ));
        app.approve_pending(false);

        assert!(app.awaiting_approval.is_none());
        let last = app.messages.last().expect("tool result recorded");
        assert_eq!(last.role, "tool");
        assert!(last.content.to_lowercase().contains("deni"));
    }

    #[test]
    fn approve_pending_is_noop_without_call() {
        let mut app = test_app();
        let before = app.messages.len();
        app.approve_pending(true);
        assert_eq!(app.messages.len(), before);
    }

    #[test]
    fn yolo_still_blocks_forbidden_shell_command() {
        // Audit-design.md §9.3: Yolo means "fewer prompts", not "I accept any
        // consequence". `rm -rf /` is classified Outcome::Deny by the built-in
        // balanced policy → SafetyClass::Forbidden → must NOT auto-run even
        // when ApprovalMode is Yolo. The call should park in awaiting_approval
        // exactly as it would under Ask, NOT be silently executed.
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Yolo;
        app.pending_tool_calls.push(mk_call(
            "run_shell_command",
            r#"{"command":"rm -rf /"}"#,
        ));
        app.process_pending_tools();

        assert!(
            app.awaiting_approval.is_some(),
            "Yolo must NOT auto-run a Forbidden shell command"
        );
        assert!(
            app.pending_tool_calls.is_empty(),
            "the call should be moved into awaiting_approval, not left queued"
        );
        // No tool result should be recorded yet — execution is blocked
        // pending the user's explicit consent.
        let last_role = app.messages.last().map(|m| m.role.as_str());
        assert_ne!(last_role, Some("tool"));
    }

    #[test]
    fn yolo_runs_safe_and_needsapproval_shell_commands() {
        // The flip side: Yolo SHOULD auto-run anything that's not Forbidden.
        // `uptime` is SafetyClass::Safe under balanced; it must run without
        // prompting in Yolo mode.
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Yolo;
        app.pending_tool_calls.push(mk_call(
            "run_shell_command",
            r#"{"command":"uptime"}"#,
        ));
        app.process_pending_tools();

        assert!(app.awaiting_approval.is_none(), "Safe command should run under Yolo");
        assert!(app.pending_tool_calls.is_empty());
        let last = app.messages.last().expect("tool result recorded");
        assert_eq!(last.role, "tool");
    }

    #[test]
    fn auto_blocks_needsapproval_but_runs_safe() {
        // Auto runs Safe, asks on NeedsApproval, asks on Forbidden.
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Auto;
        app.pending_tool_calls.push(mk_call(
            "run_shell_command",
            // `apt update` is not in any allow rule but parses cleanly →
            // default RequireApproval → SafetyClass::NeedsApproval.
            r#"{"command":"apt update"}"#,
        ));
        app.process_pending_tools();
        assert!(
            app.awaiting_approval.is_some(),
            "Auto must prompt for NeedsApproval"
        );
    }

    #[test]
    fn unknown_tool_produces_error_and_drains_queue() {
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Yolo;
        app.pending_tool_calls
            .push(mk_call("nonexistent_tool_xyz", "{}"));
        app.process_pending_tools();

        assert!(app.pending_tool_calls.is_empty());
        assert!(app.awaiting_approval.is_none());
        let last = app.messages.last().expect("tool result recorded");
        assert_eq!(last.role, "tool");
        assert!(last.content.contains("unknown tool"));
    }

    #[test]
    fn finish_stream_persists_tool_calls_message() {
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Auto;
        app.streaming = true;
        app.streaming_buffer = "Checking load...".to_string();

        let call = mk_call(
            "run_shell_command",
            r#"{"command":"echo finish-stream-test"}"#,
        );
        app.finish_stream(true, vec![call]);

        assert!(!app.streaming);
        // Expect assistant-with-tool_calls + tool result in history.
        assert!(app.messages.len() >= 2);
        let has_tc = app
            .messages
            .iter()
            .any(|m| m.tool_calls.as_ref().map(|v| !v.is_empty()).unwrap_or(false));
        assert!(has_tc, "assistant message with tool_calls should be persisted");
        let last = app.messages.last().unwrap();
        assert_eq!(last.role, "tool");
        assert!(last.content.contains("finish-stream-test"));
    }

    #[test]
    fn finish_stream_without_tool_calls_keeps_assistant_only() {
        let mut app = test_app();
        app.streaming = true;
        app.streaming_buffer = "Hello there".to_string();

        app.finish_stream(true, Vec::new());

        let last = app.messages.last().expect("assistant msg recorded");
        assert_eq!(last.role, "assistant");
        assert_eq!(last.content, "Hello there");
        assert!(last.tool_calls.is_none());
    }

    #[test]
    fn stream_token_done_variant_is_struct() {
        // Compile-time check that Done carries tool_calls.
        let t = StreamToken::Done {
            tool_calls: Vec::new(),
        };
        match t {
            StreamToken::Done { tool_calls } => assert!(tool_calls.is_empty()),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn system_prompt_instructs_tool_use() {
        let app = test_app();
        let prompt = app.build_system_prompt();
        // Must steer the model toward tool calls, not command suggestions.
        assert!(prompt.to_lowercase().contains("tool"));
        assert!(prompt.contains("cosh_"));
        assert!(!prompt.contains("suggest shell commands"));
    }

    #[test]
    fn agent_turn_limit_stops_loop() {
        let mut app = test_app();
        app.approval_mode = ApprovalMode::Auto;
        app.config.model.max_session_turns = Some(2);
        app.streaming = true;
        app.streaming_buffer.clear();

        // Simulate 2 tool turns reaching the limit.
        let call = mk_call("run_shell_command", r#"{"command":"echo turn1"}"#);
        app.finish_stream(true, vec![call]);
        assert_eq!(app.agent_turn_count, 1);

        app.streaming = true;
        app.streaming_buffer.clear();
        let call = mk_call("run_shell_command", r#"{"command":"echo turn2"}"#);
        app.finish_stream(true, vec![call]);
        assert_eq!(app.agent_turn_count, 2);

        // 3rd turn should be blocked.
        app.streaming = true;
        app.streaming_buffer.clear();
        let call = mk_call("run_shell_command", r#"{"command":"echo turn3"}"#);
        app.finish_stream(true, vec![call]);
        assert!(app.pending_tool_calls.is_empty());
        assert!(app.output.contains("Agentic loop stopped"));
    }

    #[test]
    fn agent_turn_count_resets_on_user_message() {
        let mut app = test_app();
        app.agent_turn_count = 10;
        let registry = crate::commands::CommandRegistry::new();
        app.execute_command(&registry, "hello");
        assert_eq!(app.agent_turn_count, 0);
    }

    #[test]
    fn history_down_from_index_1_does_not_panic() {
        let mut app = test_app();
        app.history.push(HistoryEntry {
            command: "pkg list".into(),
            success: true,
            timestamp: "0ms".into(),
        });
        app.history_up();
        assert_eq!(app.history_index, 1);
        assert_eq!(app.input, "pkg list");
        // This used to panic with index out of bounds
        app.history_down();
        assert_eq!(app.history_index, 0);
        assert!(app.input.is_empty());
    }

    #[test]
    fn history_navigation_round_trip() {
        let mut app = test_app();
        app.history.push(HistoryEntry {
            command: "first".into(),
            success: true,
            timestamp: "0ms".into(),
        });
        app.history.push(HistoryEntry {
            command: "second".into(),
            success: true,
            timestamp: "0ms".into(),
        });
        // Go up twice
        app.history_up();
        assert_eq!(app.input, "second");
        app.history_up();
        assert_eq!(app.input, "first");
        // Come back down
        app.history_down();
        assert_eq!(app.input, "second");
        app.history_down();
        assert!(app.input.is_empty());
        // Down again is a no-op
        app.history_down();
        assert!(app.input.is_empty());
    }

    #[test]
    fn scrollback_truncation_respects_limit() {
        let mut app = test_app();
        // Fill output with more than MAX_SCROLLBACK_BYTES.
        // Each line is 200 A's + newline = 201 bytes.
        let line = "A".repeat(200);
        let lines_needed = (MAX_SCROLLBACK_BYTES / 201) + 10;
        for _ in 0..lines_needed {
            app.append_output(&line);
        }
        assert!(
            app.output.len() <= MAX_SCROLLBACK_BYTES + 300,
            "output should be trimmed near the limit, got {}",
            app.output.len()
        );
        // Verify no partial lines: every line boundary should be clean.
        // After trim_scrollback, the first byte should not be in the middle
        // of a line (i.e. previous content before the first newline should be
        // a full line of A's, not a truncated fragment).
        let first_newline = app.output.find('\n').unwrap_or(app.output.len());
        let first_line = &app.output[..first_newline];
        assert_eq!(first_line.len(), 200, "first line should be a complete 200-char line");
    }

    #[test]
    fn scrollback_under_limit_is_untouched() {
        let mut app = test_app();
        app.append_output("line one");
        app.append_output("line two");
        assert_eq!(app.output, "line one\nline two");
    }

    #[test]
    fn scrollback_truncation_handles_multibyte_utf8() {
        // Each CJK char is 3 bytes in UTF-8; this 30-byte chunk forces
        // trim_scrollback's `cut` offset to land mid-character on most
        // iterations. The previous byte-offset slice would panic with
        // "byte index N is not a char boundary".
        let mut app = test_app();
        let chunk = "中文之乎哉也焉而则若"; // 30 bytes, no newlines
        // Push enough copies (with newlines between) to exceed the
        // 512 KiB scrollback limit and drive many trim cycles.
        for _ in 0..(MAX_SCROLLBACK_BYTES / chunk.len() + 200) {
            app.append_output(chunk);
        }
        assert!(app.output.len() <= MAX_SCROLLBACK_BYTES + chunk.len() + 1);
        // String guarantees valid UTF-8 — but explicitly check that the
        // start sits on a char boundary, since the bug was that drain
        // could land mid-codepoint.
        assert!(app.output.is_char_boundary(0));
        assert!(app.output.is_char_boundary(app.output.len()));
    }

    #[test]
    fn scrollback_truncation_handles_long_unbroken_multibyte_line() {
        // A single very long multi-byte string with no newlines — exactly
        // the case that drove the original PoC: cut falls deep inside
        // the string, almost certainly mid-codepoint.
        let mut app = test_app();
        let long = "中文测试内容".repeat(100_000); // ~1.7 MB, no '\n'
        app.append_output(&long); // Must not panic.
        assert!(app.output.len() <= MAX_SCROLLBACK_BYTES + 4);
        assert!(app.output.is_char_boundary(0));
    }

    #[test]
    fn fix_orphaned_tool_messages_removes_leading_tool_msgs() {
        let mut app = test_app();
        app.messages.push(llm::Message::system("sys"));
        app.messages.push(llm::Message::tool_result("call_1", "run_shell_command", "output"));
        app.messages.push(llm::Message::tool_result("call_2", "cosh_pkg_list", "output2"));
        app.messages.push(llm::Message::user("hello"));

        app.fix_orphaned_tool_messages();

        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[0].role, "system");
        assert_eq!(app.messages[1].role, "user");
    }

    #[test]
    fn fix_orphaned_tool_messages_preserves_valid_sequence() {
        let mut app = test_app();
        app.messages.push(llm::Message::system("sys"));
        app.messages.push(llm::Message::user("hello"));
        app.messages.push(llm::Message::assistant("response"));

        app.fix_orphaned_tool_messages();

        assert_eq!(app.messages.len(), 3);
    }

    #[test]
    fn audit_command_dispatches_to_info_message() {
        let app = test_app();
        let (success, output) = app.dispatch("audit check --action test");
        assert!(success);
        assert!(output.contains("CLI"));
    }
}
