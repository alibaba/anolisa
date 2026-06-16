use std::collections::{HashMap, HashSet};

use cosh_shell::{
    agent_render::{NoticePanelModel, QuestionPanelModel, RatatuiInlineRenderer},
    raw_input::RawInputCapture,
    types::{AgentEvent, GovernedEvent, QuestionSelectionMode, ShellEvent, ShellEventKind},
    AuthProviderInfo, AuthResponse,
};

use crate::auth::providers::{builtin_auth_providers, builtin_base_url_for_provider, default_model_for_provider};
use crate::runtime::dispatcher::stable_event_key;
use crate::runtime::state::InlineState;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeAuthState {
    pub(crate) id: String,
    #[allow(dead_code)]
    pub(crate) request_id: String,
    pub(crate) phase: AuthPhase,
    pub(crate) providers: Vec<AuthProviderInfo>,
    pub(crate) selected_provider: usize,
    pub(crate) current_field: usize,
    pub(crate) collected_values: HashMap<String, String>,
    pub(crate) field_input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthPhase {
    SelectingProvider,
    FillingField,
}

impl RuntimeAuthState {
    fn current_provider(&self) -> &AuthProviderInfo {
        &self.providers[self.selected_provider]
    }

    fn current_field_info(&self) -> Option<&cosh_shell::AuthFieldInfo> {
        self.current_provider().fields.get(self.current_field)
    }

    fn all_fields_collected(&self) -> bool {
        self.current_field >= self.current_provider().fields.len()
    }
}

#[derive(Debug, Default)]
pub(crate) struct AuthState {
    pub(crate) state: Option<RuntimeAuthState>,
    pub(crate) handled_card_events: HashSet<String>,
    pub(crate) completed_ids: HashSet<String>,
}

pub(crate) fn record_auth_required(
    state: &mut InlineState,
    governed_events: &[GovernedEvent],
) -> Vec<String> {
    let mut ids = Vec::new();
    for event in governed_events {
        if let AgentEvent::AuthRequired {
            request_id,
            providers,
            ..
        } = &event.event
        {
            if state.auth.state.is_some() {
                continue;
            }
            let id = format!("auth-{request_id}");
            if state.auth.completed_ids.contains(&id) {
                continue;
            }
            state.auth.state = Some(RuntimeAuthState {
                id: id.clone(),
                request_id: request_id.clone(),
                phase: AuthPhase::SelectingProvider,
                providers: providers.clone(),
                selected_provider: 0,
                current_field: 0,
                collected_values: HashMap::new(),
                field_input: String::new(),
            });
            ids.push(id);
        }
    }
    ids
}

pub(crate) fn render_auth_panel<W: std::io::Write>(
    state: &mut InlineState,
    ids: &[String],
    output: &mut W,
) -> std::io::Result<()> {
    for id in ids {
        let Some(auth) = &state.auth.state else {
            continue;
        };
        if auth.id != *id {
            continue;
        }
        render_current_auth_panel(state, output)?;
    }
    Ok(())
}

pub(crate) fn pending_auth_capture(state: &InlineState) -> Option<RawInputCapture> {
    let auth = state.auth.state.as_ref()?;
    match auth.phase {
        AuthPhase::SelectingProvider => Some(RawInputCapture::Question {
            id: auth.id.clone(),
            option_count: auth.providers.len(),
            allow_free_text: false,
            multiple: false,
        }),
        AuthPhase::FillingField => Some(RawInputCapture::Question {
            id: auth.id.clone(),
            option_count: 0,
            allow_free_text: true,
            multiple: false,
        }),
    }
}

pub(crate) fn has_pending_auth(state: &InlineState) -> bool {
    state.auth.state.is_some()
}

/// Trigger auth panel from `/auth` slash command.
/// Constructs builtin provider templates directly in cosh-shell.
pub(crate) fn trigger_auth_from_slash<W: std::io::Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if state.auth.state.is_some() {
        return Ok(());
    }

    let providers = builtin_auth_providers();
    let request_id = format!(
        "slash-auth-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let id = format!("auth-{request_id}");

    state.auth.state = Some(RuntimeAuthState {
        id: id.clone(),
        request_id,
        phase: AuthPhase::SelectingProvider,
        providers,
        selected_provider: 0,
        current_field: 0,
        collected_values: HashMap::new(),
        field_input: String::new(),
    });

    render_current_auth_panel(state, output)?;
    Ok(())
}

fn handle_auth_focus<W: std::io::Write>(
    state: &mut InlineState,
    id: &str,
    selected: usize,
    output: &mut W,
) -> std::io::Result<bool> {
    let Some(auth) = state.auth.state.as_mut() else {
        return Ok(false);
    };
    if auth.id != id {
        return Ok(false);
    }
    if auth.phase == AuthPhase::SelectingProvider {
        auth.selected_provider = selected.min(auth.providers.len().saturating_sub(1));
        clear_active_auth_panel(state, output)?;
        render_current_auth_panel(state, output)?;
    }
    Ok(true)
}

fn handle_auth_input<W: std::io::Write>(
    state: &mut InlineState,
    id: &str,
    text: &str,
    output: &mut W,
) -> std::io::Result<bool> {
    let Some(auth) = state.auth.state.as_mut() else {
        return Ok(false);
    };
    if auth.id != id {
        return Ok(false);
    }
    if auth.phase == AuthPhase::FillingField {
        auth.field_input = text.to_string();
        clear_active_auth_panel(state, output)?;
        render_current_auth_panel(state, output)?;
    }
    Ok(true)
}

fn handle_auth_answer<W: std::io::Write>(
    state: &mut InlineState,
    id: &str,
    raw_answer: &str,
    output: &mut W,
) -> std::io::Result<bool> {
    let Some(auth) = state.auth.state.as_mut() else {
        return Ok(false);
    };
    if auth.id != id {
        return Ok(false);
    }

    match auth.phase {
        AuthPhase::SelectingProvider => {
            auth.phase = AuthPhase::FillingField;
            auth.current_field = 0;
            auth.collected_values.clear();
            auth.field_input.clear();
            clear_active_auth_panel(state, output)?;
            render_current_auth_panel(state, output)?;
            Ok(true)
        }
        AuthPhase::FillingField => {
            let value = if raw_answer.is_empty() {
                auth.field_input.clone()
            } else {
                raw_answer.to_string()
            };
            if let Some(field) = auth.current_field_info().cloned() {
                auth.collected_values.insert(field.name.clone(), value);
            }
            auth.current_field += 1;
            auth.field_input.clear();

            if auth.all_fields_collected() {
                clear_active_auth_panel(state, output)?;
                send_auth_response(state, output)?;
                Ok(true)
            } else {
                clear_active_auth_panel(state, output)?;
                render_current_auth_panel(state, output)?;
                Ok(true)
            }
        }
    }
}

fn send_auth_response<W: std::io::Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let auth = state.auth.state.take().expect("auth state present");
    state.auth.completed_ids.insert(auth.id.clone());
    let provider = &auth.providers[auth.selected_provider];
    let response = AuthResponse {
        provider_id: provider.id.clone(),
        values: auth.collected_values,
        persist: true,
    };

    if let Some(active_run) = state.agent_run.active.as_ref() {
        let _ = active_run.handle.respond_auth(response);
    } else {
        persist_auth_credentials(&response);
    }

    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
    renderer.write_notice_panel(
        output,
        NoticePanelModel {
            title: "Auth configured",
            body: vec![format!("Provider: {} \u{2014} credentials saved.", provider.label)],
            footer: None,
        },
    )?;

    if std::env::var("COSH_SHELL_ISOLATED").is_ok() {
        writeln!(output)?;
        write!(output, "cosh-osc$ ")?;
    } else {
        state.trigger_pty_prompt = true;
    }

    output.flush()?;
    Ok(())
}

/// Directly persist auth credentials to cosh-tui's config file.
/// Used when `/auth` is triggered without an active cosh-tui process.
fn persist_auth_credentials(response: &AuthResponse) {
    let home = match std::env::var("HOME") {
        Ok(h) => std::path::PathBuf::from(h),
        Err(_) => return,
    };
    let config_dir = home.join(".copilot-shell");
    if std::fs::create_dir_all(&config_dir).is_err() {
        return;
    }
    let config_path = config_dir.join("config.toml");

    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();

    let base_url = response.values.get("base_url").cloned();
    let api_key = response.values.get("api_key").cloned().unwrap_or_default();
    let provider_type = match response.provider_id.as_str() {
        "dashscope" => "dashscope",
        _ => "openai",
    };
    let default_model = match response.provider_id.as_str() {
        "dashscope" => "qwen3.7-plus",
        _ => "gpt-4o",
    };
    let user_model = response.values.get("model").filter(|m| !m.is_empty());
    let final_model = user_model
        .map(|m| m.as_str())
        .or_else(|| default_model_for_provider(&response.provider_id))
        .unwrap_or(default_model);
    let builtin_base_url = builtin_base_url_for_provider(&response.provider_id)
        .map(str::to_string);
    let final_base_url = base_url.or(builtin_base_url).unwrap_or_default();

    let mut content = String::new();
    let mut in_ai_section = false;
    for line in existing.lines() {
        if line.trim().starts_with("[ai") {
            in_ai_section = true;
            continue;
        }
        if in_ai_section && line.trim().starts_with('[') && !line.trim().starts_with("[ai") {
            in_ai_section = false;
        }
        if !in_ai_section {
            content.push_str(line);
            content.push('\n');
        }
    }

    content.push_str("[ai]\n");
    content.push_str(&format!(
        "active_provider = \"{}\"\n",
        response.provider_id
    ));
    content.push_str(&format!("active_model = \"{}\"\n\n", final_model));
    content.push_str(&format!(
        "[ai.providers.{}]\n",
        response.provider_id
    ));
    content.push_str(&format!("type = \"{}\"\n", provider_type));
    content.push_str(&format!("base_url = \"{}\"\n", final_base_url));
    content.push_str(&format!("api_key = \"{}\"\n", api_key));
    content.push_str(&format!("model = \"{}\"\n", final_model));

    let pid = std::process::id();
    let tmp_path = config_dir.join(format!("config.toml.tmp.{pid}"));
    if std::fs::write(&tmp_path, &content).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&tmp_path, perms);
    }
    if std::fs::rename(&tmp_path, &config_path).is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
}

fn render_current_auth_panel<W: std::io::Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let Some(auth) = &state.auth.state else {
        return Ok(());
    };
    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);

    match auth.phase {
        AuthPhase::SelectingProvider => {
            let options: Vec<String> = auth.providers.iter().map(|p| p.label.clone()).collect();
            let model = QuestionPanelModel {
                id: &auth.id,
                question: "\u{1f511} Authentication Required \u{2014} Select your AI provider:",
                options: &options,
                selected_option: auth.selected_provider,
                selected_options: &[],
                custom_answer: "",
                allow_free_text: false,
                selection_mode: QuestionSelectionMode::Single,
            };
            let height = renderer.write_question_panel(output, model)?;
            state.questions.active_panel_height = height;
            state.questions.active_panel_id = Some(auth.id.clone());
        }
        AuthPhase::FillingField => {
            let field = auth.current_field_info();
            let label = field.map(|f| f.label.as_str()).unwrap_or("Value");
            let is_secret = field.map(|f| f.secret).unwrap_or(false);
            let hint_text = field.and_then(|f| f.hint.as_deref()).unwrap_or("");
            let provider = auth.current_provider();
            let mut question = format!(
                "\u{1f511} {} \u{2014} Enter {}:",
                provider.label, label
            );
            if !hint_text.is_empty() {
                question.push_str(&format!("\n  hint: {}", hint_text));
            }
            if !auth.field_input.is_empty() {
                let display = if is_secret {
                    "\u{2022}".repeat(auth.field_input.len())
                } else {
                    auth.field_input.clone()
                };
                question.push_str(&format!("\n  > {}", display));
            }
            let model = QuestionPanelModel {
                id: &auth.id,
                question: &question,
                options: &[],
                selected_option: 0,
                selected_options: &[],
                custom_answer: "",
                allow_free_text: true,
                selection_mode: QuestionSelectionMode::Single,
            };
            let height = renderer.write_question_panel(output, model)?;
            state.questions.active_panel_height = height;
            state.questions.active_panel_id = Some(auth.id.clone());
        }
    }
    output.flush()?;
    Ok(())
}

fn clear_active_auth_panel<W: std::io::Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let height = state.questions.active_panel_height;
    if height == 0 {
        state.questions.active_panel_id = None;
        return Ok(());
    }
    write!(output, "\x1b[{height}A")?;
    for row in 0..height {
        write!(output, "\r\x1b[2K")?;
        if row + 1 < height {
            write!(output, "\x1b[1B")?;
        }
    }
    if height > 1 {
        write!(output, "\x1b[{}A", height - 1)?;
    }
    write!(output, "\r")?;
    state.questions.active_panel_id = None;
    state.questions.active_panel_height = 0;
    Ok(())
}

fn cancel_auth_panel<W: std::io::Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    clear_active_auth_panel(state, output)?;
    if let Some(auth) = state.auth.state.as_ref() {
        state.auth.completed_ids.insert(auth.id.clone());
    }
    state.auth.state = None;

    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
    renderer.write_notice_panel(
        output,
        NoticePanelModel {
            title: "Auth cancelled",
            body: vec!["Authentication skipped.".to_string()],
            footer: None,
        },
    )?;

    if std::env::var("COSH_SHELL_ISOLATED").is_ok() {
        writeln!(output)?;
        write!(output, "cosh-osc$ ")?;
    } else {
        state.trigger_pty_prompt = true;
    }
    output.flush()?;
    Ok(())
}

pub(crate) fn render_auth_card_actions<W: std::io::Write>(
    events: &[ShellEvent],
    state: &mut InlineState,
    output: &mut W,
    event_index_base: usize,
) -> std::io::Result<()> {
    if !has_pending_auth(state) {
        return Ok(());
    }
    for (idx, event) in events.iter().enumerate() {
        let event_index = event_index_base + idx;
        if event.kind != ShellEventKind::UserInputIntercepted {
            continue;
        }
        if event.component.as_deref() != Some("card") {
            continue;
        }
        let dedup_key = stable_event_key("auth-card", event_index, event);
        if !state.auth.handled_card_events.insert(dedup_key) {
            continue;
        }
        match event.message.as_deref() {
            Some("focus") => {
                if let Some((id, selected)) = parse_card_id_usize(event) {
                    handle_auth_focus(state, &id, selected, output)?;
                }
            }
            Some("input") => {
                if let Some((id, text)) = parse_card_id_text(event) {
                    handle_auth_input(state, &id, &text, output)?;
                }
            }
            Some("answer") => {
                if let Some(answer) = event.input.as_deref() {
                    let auth_id = state.auth.state.as_ref().map(|a| a.id.clone());
                    if let Some(id) = auth_id {
                        handle_auth_answer(state, &id, answer, output)?;
                        let key = stable_event_key("question-answer", event_index, event);
                        state.questions.handled_answers.insert(key);
                    }
                }
            }
            Some("cancel") | Some("question_cancel") => {
                if let Some(cancel_id) = event.input.as_deref() {
                    let auth_id = state.auth.state.as_ref().map(|a| a.id.clone());
                    if auth_id.as_deref() == Some(cancel_id.trim()) {
                        cancel_auth_panel(state, output)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_card_id_usize(event: &ShellEvent) -> Option<(String, usize)> {
    let (id, val) = event.input.as_deref()?.split_once(':')?;
    let val = val.trim().parse::<usize>().ok()?;
    Some((id.trim().to_string(), val))
}

fn parse_card_id_text(event: &ShellEvent) -> Option<(String, String)> {
    let (id, text) = event.input.as_deref()?.split_once(':')?;
    Some((id.trim().to_string(), text.to_string()))
}
