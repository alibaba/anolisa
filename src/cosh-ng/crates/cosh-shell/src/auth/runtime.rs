use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use serde::Deserialize;

use crate::auth::ecs;
use crate::auth::providers::{
    builtin_auth_providers, builtin_base_url_for_provider, default_model_for_provider,
};
use crate::runtime::dispatcher::stable_event_key;
use crate::runtime::prelude::{
    AgentEvent, AuthFieldInfo, AuthProviderInfo, AuthResponse, GovernedEvent, NoticePanelModel,
    QuestionPanelModel, QuestionSelectionMode, RatatuiInlineRenderer, RawInputCapture, ShellEvent,
    ShellEventKind,
};
use crate::runtime::state::InlineState;

// ─── Minimal config parsing for reading existing providers ───

#[derive(Debug, Deserialize, Default)]
struct MiniConfig {
    #[serde(default)]
    ai: MiniAiConfig,
}

#[derive(Debug, Deserialize, Default)]
struct MiniAiConfig {
    active_provider: Option<String>,
    #[allow(dead_code)]
    active_model: Option<String>,
    #[serde(default)]
    providers: HashMap<String, MiniProviderConfig>,
    // Preserve these fields during incremental writes
    output_language: Option<String>,
    thinking: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct MiniProviderConfig {
    #[serde(rename = "type")]
    provider_type: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    access_key_id: Option<String>,
    access_key_secret: Option<String>,
    security_token: Option<String>,
}

/// An existing provider loaded from config.toml for the ManagingProviders phase.
#[derive(Debug, Clone)]
pub(crate) struct ExistingProvider {
    pub(crate) name: String,          // section name (e.g. "default")
    pub(crate) provider_type: String, // type field value
    pub(crate) label: String,         // display name based on type
    pub(crate) model: String,         // current model
    pub(crate) is_active: bool,       // whether this is the active_provider
}

fn label_for_provider_type(provider_type: &str) -> &'static str {
    match provider_type {
        "dashscope" => "DashScope (\u{767e}\u{70bc})",
        "aliyun" => "Aliyun Authentication",
        _ => "OpenAI Compatible",
    }
}

fn load_existing_providers() -> (Vec<ExistingProvider>, String) {
    let config_path = config_file_path();
    let content = std::fs::read_to_string(&config_path).unwrap_or_default();
    let config: MiniConfig = toml::from_str(&content).unwrap_or_default();
    let active = config.ai.active_provider.unwrap_or_default();

    let mut providers: Vec<ExistingProvider> = config
        .ai
        .providers
        .iter()
        .map(|(name, p)| {
            let ptype = p.provider_type.as_deref().unwrap_or("openai");
            ExistingProvider {
                name: name.clone(),
                provider_type: ptype.to_string(),
                label: label_for_provider_type(ptype).to_string(),
                model: p.model.clone().unwrap_or_default(),
                is_active: name == &active,
            }
        })
        .collect();

    // Sort: active first, then alphabetical
    providers.sort_by(|a, b| b.is_active.cmp(&a.is_active).then(a.name.cmp(&b.name)));

    (providers, active)
}

fn config_file_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".copilot-shell")
        .join("config.toml")
}

/// Set a provider as active without editing its configuration.
fn activate_provider(provider_name: &str) {
    let config_path = config_file_path();
    let existing_content = std::fs::read_to_string(&config_path).unwrap_or_default();
    let mut config: MiniConfig = toml::from_str(&existing_content).unwrap_or_default();

    // Look up the model from the provider being activated
    let model = config
        .ai
        .providers
        .get(provider_name)
        .and_then(|p| p.model.clone());

    config.ai.active_provider = Some(provider_name.to_string());
    if let Some(m) = model {
        config.ai.active_model = Some(m);
    }

    let config_dir = config_path.parent().unwrap().to_path_buf();
    write_config_incremental(&config_path, &config_dir, &existing_content, &config.ai);
}

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
    /// Existing providers loaded from config.toml (for ManagingProviders phase)
    pub(crate) existing_providers: Vec<ExistingProvider>,
    /// The section name of the provider being edited (None = new provider)
    pub(crate) editing_provider_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthPhase {
    /// Show existing providers list + "Add new" option
    ManagingProviders,
    /// Action menu after selecting an existing provider
    ProviderAction { provider_idx: usize },
    SelectingProvider,
    FillingField,
    /// Aliyun: ECS detected, showing console URL + QR code, polling in background
    AliyunPolling {
        instance_id: String,
        console_url: String,
    },
}

impl RuntimeAuthState {
    fn current_provider(&self) -> &AuthProviderInfo {
        &self.providers[self.selected_provider]
    }

    fn current_field_info(&self) -> Option<&AuthFieldInfo> {
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
    /// Channel for receiving ECS polling results from background thread.
    pub(crate) ecs_poll_rx: Option<mpsc::Receiver<ecs::EcsTaskResult>>,
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
                existing_providers: Vec::new(),
                editing_provider_name: None,
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
    match &auth.phase {
        AuthPhase::ManagingProviders => Some(RawInputCapture::Question {
            id: auth.id.clone(),
            // existing providers + "+ Add new provider" option
            option_count: auth.existing_providers.len() + 1,
            allow_free_text: false,
            multiple: false,
        }),
        AuthPhase::ProviderAction { provider_idx } => {
            let is_active = auth
                .existing_providers
                .get(*provider_idx)
                .map(|ep| ep.is_active)
                .unwrap_or(false);
            // "Set as active" (only if not active) + "Edit" + "Cancel"
            let option_count = if is_active { 2 } else { 3 };
            Some(RawInputCapture::Question {
                id: auth.id.clone(),
                option_count,
                allow_free_text: false,
                multiple: false,
            })
        }
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
        AuthPhase::AliyunPolling { .. } => {
            // During polling, capture Esc to allow cancellation
            Some(RawInputCapture::Question {
                id: auth.id.clone(),
                option_count: 0,
                allow_free_text: false,
                multiple: false,
            })
        }
    }
}

pub(crate) fn has_pending_auth(state: &InlineState) -> bool {
    state.auth.state.is_some()
}

/// Check if a background ECS polling task has completed.
/// Should be called from the event loop on each iteration.
pub(crate) fn check_aliyun_poll_result<W: std::io::Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    // Only check if we're in AliyunPolling phase
    let is_polling = state
        .auth
        .state
        .as_ref()
        .map(|a| matches!(a.phase, AuthPhase::AliyunPolling { .. }))
        .unwrap_or(false);
    if !is_polling {
        return Ok(());
    }

    // Non-blocking check on the channel
    let result = state
        .auth
        .ecs_poll_rx
        .as_ref()
        .and_then(|rx| rx.try_recv().ok());

    let Some(result) = result else {
        return Ok(()); // Not ready yet
    };

    // Polling completed — drop the receiver
    state.auth.ecs_poll_rx = None;

    match result {
        ecs::EcsTaskResult::Authorized(creds) => {
            // Auto-submit with STS credentials
            if let Some(auth) = state.auth.state.as_mut() {
                auth.collected_values
                    .insert("access_key_id".to_string(), creds.access_key_id);
                auth.collected_values
                    .insert("access_key_secret".to_string(), creds.access_key_secret);
                auth.collected_values
                    .insert("security_token".to_string(), creds.security_token);
            }
            send_auth_response(state, output)?;
        }
        ecs::EcsTaskResult::NotOnEcs => {
            // Shouldn't happen (we detected ECS before starting poll), but handle gracefully
            if let Some(auth) = state.auth.state.as_mut() {
                auth.phase = AuthPhase::FillingField;
                auth.current_field = 0;
                auth.collected_values.clear();
                auth.field_input.clear();
            }
            render_current_auth_panel(state, output)?;
        }
        ecs::EcsTaskResult::AuthorizationFailed(msg) => {
            let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
            renderer.write_notice_panel(
                output,
                NoticePanelModel {
                    title: "Aliyun Auth Failed",
                    body: vec![msg, "Falling back to manual AK/SK input.".to_string()],
                    footer: None,
                },
            )?;
            // Fall back to AK/SK input
            if let Some(auth) = state.auth.state.as_mut() {
                auth.phase = AuthPhase::FillingField;
                auth.current_field = 0;
                auth.collected_values.clear();
                auth.field_input.clear();
            }
            render_current_auth_panel(state, output)?;
        }
    }
    Ok(())
}

/// Trigger auth panel from `/auth` slash command.
/// Now starts in ManagingProviders phase to show existing providers.
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

    // Load existing providers from config.toml
    let (existing_providers, _active) = load_existing_providers();

    // If there are existing providers, start in ManagingProviders phase
    let phase = if existing_providers.is_empty() {
        AuthPhase::SelectingProvider
    } else {
        AuthPhase::ManagingProviders
    };

    state.auth.state = Some(RuntimeAuthState {
        id: id.clone(),
        request_id,
        phase,
        providers,
        selected_provider: 0,
        current_field: 0,
        collected_values: HashMap::new(),
        field_input: String::new(),
        existing_providers,
        editing_provider_name: None,
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
    match auth.phase {
        AuthPhase::ManagingProviders => {
            let max = auth.existing_providers.len(); // last item = "+ Add new"
            auth.selected_provider = selected.min(max);
            clear_active_auth_panel(state, output)?;
            render_current_auth_panel(state, output)?;
        }
        AuthPhase::ProviderAction { .. } => {
            auth.selected_provider = selected;
            clear_active_auth_panel(state, output)?;
            render_current_auth_panel(state, output)?;
        }
        AuthPhase::SelectingProvider => {
            auth.selected_provider = selected.min(auth.providers.len().saturating_sub(1));
            clear_active_auth_panel(state, output)?;
            render_current_auth_panel(state, output)?;
        }
        _ => {}
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
        AuthPhase::ManagingProviders => {
            let idx = auth.selected_provider;
            if idx < auth.existing_providers.len() {
                // Selected an existing provider -> show action menu
                auth.phase = AuthPhase::ProviderAction { provider_idx: idx };
                auth.selected_provider = 0;
                clear_active_auth_panel(state, output)?;
                render_current_auth_panel(state, output)?;
            } else {
                // Selected "+ Add new provider" -> go to SelectingProvider
                auth.selected_provider = 0;
                auth.editing_provider_name = None;
                auth.phase = AuthPhase::SelectingProvider;
                auth.current_field = 0;
                auth.collected_values.clear();
                auth.field_input.clear();
                clear_active_auth_panel(state, output)?;
                render_current_auth_panel(state, output)?;
            }
            Ok(true)
        }
        AuthPhase::ProviderAction { provider_idx } => {
            let existing = auth.existing_providers[provider_idx].clone();
            let is_active = existing.is_active;

            // Determine which action was selected
            let action = if is_active {
                // Options: "Edit configuration" / "Cancel"
                match auth.selected_provider {
                    0 => "edit",
                    _ => "cancel",
                }
            } else {
                // Options: "Set as active" / "Edit configuration" / "Cancel"
                match auth.selected_provider {
                    0 => "activate",
                    1 => "edit",
                    _ => "cancel",
                }
            };

            match action {
                "activate" => {
                    // Just update active_provider in config, no editing
                    activate_provider(&existing.name);
                    // Clear and show confirmation
                    state.auth.state.take();
                    clear_active_auth_panel(state, output)?;
                    let renderer =
                        RatatuiInlineRenderer::for_terminal().with_language(state.language);
                    renderer.write_notice_panel(
                        output,
                        NoticePanelModel {
                            title: "Provider switched",
                            body: vec![format!(
                                "Active provider: {} (\"{}\")",
                                existing.label, existing.name
                            )],
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
                }
                "edit" => {
                    // Enter edit mode for this provider
                    let provider_type = existing.provider_type.as_str();
                    let template_idx = auth
                        .providers
                        .iter()
                        .position(|p| match provider_type {
                            "dashscope" => p.id == "dashscope",
                            "aliyun" => p.id == "aliyun",
                            _ => p.id == "openai_compat",
                        })
                        .unwrap_or(0);

                    auth.selected_provider = template_idx;
                    auth.editing_provider_name = Some(existing.name.clone());

                    // Pre-fill collected_values from existing config
                    let config_path = config_file_path();
                    let content =
                        std::fs::read_to_string(&config_path).unwrap_or_default();
                    let config: MiniConfig =
                        toml::from_str(&content).unwrap_or_default();
                    if let Some(pcfg) = config.ai.providers.get(&existing.name) {
                        if let Some(ref v) = pcfg.api_key {
                            auth.collected_values
                                .insert("api_key".to_string(), v.clone());
                        }
                        if let Some(ref v) = pcfg.base_url {
                            auth.collected_values
                                .insert("base_url".to_string(), v.clone());
                        }
                        if let Some(ref v) = pcfg.model {
                            auth.collected_values
                                .insert("model".to_string(), v.clone());
                        }
                        if let Some(ref v) = pcfg.access_key_id {
                            auth.collected_values
                                .insert("access_key_id".to_string(), v.clone());
                        }
                        if let Some(ref v) = pcfg.access_key_secret {
                            auth.collected_values
                                .insert("access_key_secret".to_string(), v.clone());
                        }
                        if let Some(ref v) = pcfg.security_token {
                            auth.collected_values
                                .insert("security_token".to_string(), v.clone());
                        }
                    }

                    auth.phase = AuthPhase::FillingField;
                    auth.current_field = 0;
                    let first_field_name =
                        auth.current_field_info().map(|f| f.name.clone());
                    if let Some(name) = first_field_name {
                        auth.field_input = auth
                            .collected_values
                            .get(&name)
                            .cloned()
                            .unwrap_or_default();
                    } else {
                        auth.field_input.clear();
                    }
                    clear_active_auth_panel(state, output)?;
                    render_current_auth_panel(state, output)?;
                }
                _ => {
                    // Cancel -> back to ManagingProviders
                    auth.phase = AuthPhase::ManagingProviders;
                    auth.selected_provider = provider_idx;
                    clear_active_auth_panel(state, output)?;
                    render_current_auth_panel(state, output)?;
                }
            }
            Ok(true)
        }
        AuthPhase::SelectingProvider => {
            let selected_id = auth.current_provider().id.clone();

            if selected_id == "aliyun" {
                // Aliyun flow: synchronously detect ECS (fast: 0.5s timeout)
                clear_active_auth_panel(state, output)?;

                let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
                renderer.write_notice_panel(
                    output,
                    NoticePanelModel {
                        title: "Aliyun Authentication",
                        body: vec!["Detecting ECS environment...".to_string()],
                        footer: None,
                    },
                )?;
                output.flush()?;

                if let Some(ecs_info) = ecs::detect_ecs_environment() {
                    // On ECS: show console URL + QR code, start polling in background
                    let console_url = ecs_info.console_url.clone();
                    let instance_id = ecs_info.instance_id.clone();

                    // Start background polling thread
                    let (tx, rx) = mpsc::channel();
                    std::thread::spawn(move || {
                        let result = ecs::poll_and_get_credentials();
                        let _ = tx.send(result);
                    });
                    state.auth.ecs_poll_rx = Some(rx);

                    let auth = state.auth.state.as_mut().unwrap();
                    auth.phase = AuthPhase::AliyunPolling {
                        instance_id,
                        console_url,
                    };
                    // Clear the "Detecting..." notice and render polling panel
                    clear_active_auth_panel(state, output)?;
                    render_current_auth_panel(state, output)?;
                } else {
                    // Not on ECS: enter AK/SK input (uses the standard FillingField flow)
                    let auth = state.auth.state.as_mut().unwrap();
                    auth.phase = AuthPhase::FillingField;
                    auth.current_field = 0;
                    auth.collected_values.clear();
                    auth.field_input.clear();
                    clear_active_auth_panel(state, output)?;
                    render_current_auth_panel(state, output)?;
                }
            } else {
                // DashScope / OpenAI: standard field-filling flow
                auth.phase = AuthPhase::FillingField;
                auth.current_field = 0;
                auth.collected_values.clear();
                auth.field_input.clear();
                clear_active_auth_panel(state, output)?;
                render_current_auth_panel(state, output)?;
            }
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
            // Load next field's pre-filled value (for edit mode)
            let next_field_name = auth.current_field_info().map(|f| f.name.clone());
            if let Some(name) = next_field_name {
                auth.field_input = auth
                    .collected_values
                    .get(&name)
                    .cloned()
                    .unwrap_or_default();
            } else {
                auth.field_input.clear();
            }

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
        AuthPhase::AliyunPolling { .. } => {
            // During polling, user input is ignored (Esc/cancel handled elsewhere)
            Ok(false)
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
    let editing_name = auth.editing_provider_name.clone();
    let response = AuthResponse {
        provider_id: provider.id.clone(),
        values: auth.collected_values,
        persist: true,
    };

    if let Some(active_run) = state.agent_run.active.as_ref() {
        if active_run.handle.respond_auth(response.clone()).is_err() {
            persist_auth_credentials(&response, editing_name.as_deref());
        }
    } else {
        persist_auth_credentials(&response, editing_name.as_deref());
    }

    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
    renderer.write_notice_panel(
        output,
        NoticePanelModel {
            title: "Auth configured",
            body: vec![format!(
                "Provider: {} \u{2014} credentials saved.",
                provider.label
            )],
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

/// Directly persist auth credentials to cosh-core's config file.
/// Uses INCREMENTAL update: reads existing config, inserts/updates the provider, preserves others.
fn persist_auth_credentials(response: &AuthResponse, editing_name: Option<&str>) {
    let config_path = config_file_path();
    let config_dir = config_path.parent().unwrap().to_path_buf();
    if std::fs::create_dir_all(&config_dir).is_err() {
        return;
    }

    // Read and parse existing config
    let existing_content = std::fs::read_to_string(&config_path).unwrap_or_default();
    let mut config: MiniConfig = toml::from_str(&existing_content).unwrap_or_default();

    // Determine the section name for this provider
    let section_name = editing_name
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            // For new providers, generate a section name based on type
            match response.provider_id.as_str() {
                "dashscope" => "dashscope".to_string(),
                "aliyun" => "aliyun".to_string(),
                "openai_compat" => "openai_compat".to_string(),
                other => other.to_string(),
            }
        });

    // Build the provider config
    let provider_type = match response.provider_id.as_str() {
        "dashscope" => "dashscope",
        "aliyun" => "aliyun",
        _ => "openai",
    };
    let default_model = match response.provider_id.as_str() {
        "dashscope" | "aliyun" => "qwen3.7-plus",
        _ => "gpt-4o",
    };
    let user_model = response.values.get("model").filter(|m| !m.is_empty());
    let final_model = user_model
        .map(|m| m.as_str())
        .or_else(|| default_model_for_provider(&response.provider_id))
        .unwrap_or(default_model);

    let mut pcfg = MiniProviderConfig {
        provider_type: Some(provider_type.to_string()),
        model: Some(final_model.to_string()),
        ..Default::default()
    };

    if response.provider_id == "aliyun" {
        pcfg.access_key_id = response.values.get("access_key_id").cloned();
        pcfg.access_key_secret = response.values.get("access_key_secret").cloned();
        pcfg.security_token = response.values.get("security_token").cloned();
    } else {
        let base_url = response.values.get("base_url").cloned()
            .or_else(|| builtin_base_url_for_provider(&response.provider_id).map(str::to_string));
        pcfg.base_url = base_url;
        pcfg.api_key = response.values.get("api_key").cloned();
    }

    // Insert/update this provider (preserves all others)
    config.ai.providers.insert(section_name.clone(), pcfg);

    // Update active_provider and active_model
    config.ai.active_provider = Some(section_name);
    config.ai.active_model = Some(final_model.to_string());

    // Serialize back: preserve non-[ai] sections, rewrite [ai] section
    write_config_incremental(&config_path, &config_dir, &existing_content, &config.ai);
}

/// Write config.toml preserving non-[ai] sections and rewriting [ai] + providers.
fn write_config_incremental(
    config_path: &std::path::Path,
    config_dir: &std::path::Path,
    existing_content: &str,
    ai: &MiniAiConfig,
) {
    let mut preserved = String::new();
    let mut in_ai_section = false;
    for line in existing_content.lines() {
        if line.trim().starts_with("[ai") {
            in_ai_section = true;
            continue;
        }
        if in_ai_section && line.trim().starts_with('[') && !line.trim().starts_with("[ai") {
            in_ai_section = false;
        }
        if !in_ai_section {
            preserved.push_str(line);
            preserved.push('\n');
        }
    }

    // Write [ai] section
    preserved.push_str("[ai]\n");
    if let Some(ref active) = ai.active_provider {
        preserved.push_str(&format!("active_provider = \"{}\"\n", escape_toml(active)));
    }
    if let Some(ref model) = ai.active_model {
        preserved.push_str(&format!("active_model = \"{}\"\n", escape_toml(model)));
    }
    if let Some(ref lang) = ai.output_language {
        preserved.push_str(&format!("output_language = \"{}\"\n", escape_toml(lang)));
    }
    if let Some(ref thinking) = ai.thinking {
        preserved.push_str(&format!("thinking = \"{}\"\n", escape_toml(thinking)));
    }
    preserved.push('\n');

    // Write all providers
    for (name, provider) in &ai.providers {
        preserved.push_str(&format!("[ai.providers.{}]\n", name));
        if let Some(ref t) = provider.provider_type {
            preserved.push_str(&format!("type = \"{}\"\n", escape_toml(t)));
        }
        if let Some(ref url) = provider.base_url {
            preserved.push_str(&format!("base_url = \"{}\"\n", escape_toml(url)));
        }
        if let Some(ref key) = provider.api_key {
            preserved.push_str(&format!("api_key = \"{}\"\n", escape_toml(key)));
        }
        if let Some(ref m) = provider.model {
            preserved.push_str(&format!("model = \"{}\"\n", escape_toml(m)));
        }
        if let Some(ref ak) = provider.access_key_id {
            preserved.push_str(&format!("access_key_id = \"{}\"\n", escape_toml(ak)));
        }
        if let Some(ref sk) = provider.access_key_secret {
            preserved.push_str(&format!("access_key_secret = \"{}\"\n", escape_toml(sk)));
        }
        if let Some(ref st) = provider.security_token {
            preserved.push_str(&format!("security_token = \"{}\"\n", escape_toml(st)));
        }
        preserved.push('\n');
    }

    // Atomic write
    let pid = std::process::id();
    let tmp_path = config_dir.join(format!("config.toml.tmp.{pid}"));
    if std::fs::write(&tmp_path, &preserved).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&tmp_path, perms);
    }
    if std::fs::rename(&tmp_path, config_path).is_err() {
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
        AuthPhase::ManagingProviders => {
            let mut options: Vec<String> = auth
                .existing_providers
                .iter()
                .map(|ep| {
                    let active_mark = if ep.is_active { "* [active] " } else { "  " };
                    let model_info = if ep.model.is_empty() {
                        String::new()
                    } else {
                        format!(" - {}", ep.model)
                    };
                    format!(
                        "{}{} - \"{}\"{}",
                        active_mark, ep.label, ep.name, model_info
                    )
                })
                .collect();
            options.push("  + Add new provider".to_string());

            let model = QuestionPanelModel {
                id: &auth.id,
                question: "\u{1f511} Provider Management \u{2014} Select your AI provider:",
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
        AuthPhase::ProviderAction { provider_idx } => {
            let ep = &auth.existing_providers[provider_idx];
            let title = format!(
                "\u{1f511} {} \u{2014} \"{}\":",
                ep.label, ep.name
            );
            let options: Vec<String> = if ep.is_active {
                vec![
                    "Edit configuration".to_string(),
                    "Cancel".to_string(),
                ]
            } else {
                vec![
                    "Set as active provider".to_string(),
                    "Edit configuration".to_string(),
                    "Cancel".to_string(),
                ]
            };
            let model = QuestionPanelModel {
                id: &auth.id,
                question: &title,
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
            let is_editing = auth.editing_provider_name.is_some();
            let action = if is_editing { "Edit" } else { "Enter" };
            let mut question = format!("\u{1f511} {} \u{2014} {} {}:", provider.label, action, label);
            if !hint_text.is_empty() {
                question.push_str(&format!("\n  hint: {}", hint_text));
            }
            if is_editing && !auth.field_input.is_empty() {
                question.push_str("\n  (Enter to keep current value)");
            }
            if !auth.field_input.is_empty() {
                let display = if is_secret {
                    "\u{2022}".repeat(auth.field_input.len())
                } else {
                    auth.field_input.clone()
                };
                question.push_str(&format!("\n  > {}", display));
            } else {
                question.push_str("\n  > ");
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
        AuthPhase::AliyunPolling {
            ref console_url,
            ref instance_id,
        } => {
            let mut body = vec![
                "Please open the URL below in your browser to authorize:".to_string(),
                String::new(),
                console_url.clone(),
                String::new(),
                format!("ECS Instance ID: {}", instance_id),
            ];

            // Generate QR code text (plain Unicode, no ANSI codes)
            if let Some(qr_string) = generate_qr_text(console_url) {
                body.push(String::new());
                body.push("Or scan the QR code:".to_string());
                for line in qr_string.lines() {
                    body.push(line.to_string());
                }
            }

            body.push(String::new());
            body.push("Waiting for authorization... (Press Esc to cancel)".to_string());

            let notice = NoticePanelModel {
                title: "\u{1f511} Aliyun Authentication",
                body,
                footer: None,
            };
            renderer.write_notice_panel(output, notice)?;
            // Set a minimal height for clear; notice panels don't return height
            state.questions.active_panel_height = 0;
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

fn escape_toml(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Generate a plain-text QR code using Unicode half-block characters.
///
/// Uses `█`, `▀`, `▄`, and space to render the QR code without ANSI escape
/// codes. This avoids issues with the notice panel renderer stripping ANSI.
///
/// On a dark terminal (light foreground on dark background):
/// - `█` (full block) → foreground/light → QR "light" module
/// - ` ` (space)      → background/dark  → QR "dark" module
/// - `▀` (upper half) → top light, bottom dark
/// - `▄` (lower half) → top dark, bottom light
fn generate_qr_text(data: &str) -> Option<String> {
    use qrcode::QrCode;

    let code = QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let margin = 2usize;
    let total_width = width + 2 * margin;

    let mut result = String::new();

    let light_row: String = "\u{2588}".repeat(total_width);

    // Quiet zone top
    for _ in 0..margin {
        result.push_str(&light_row);
        result.push('\n');
    }

    // QR data rows (two module rows per text line)
    let mut y = 0;
    while y < width {
        // Left margin
        for _ in 0..margin {
            result.push('\u{2588}');
        }

        for x in 0..width {
            let top_dark = colors[y * width + x] == qrcode::Color::Dark;
            let bottom_dark = if y + 1 < width {
                colors[(y + 1) * width + x] == qrcode::Color::Dark
            } else {
                false
            };

            result.push(match (top_dark, bottom_dark) {
                (true, true) => ' ',
                (true, false) => '\u{2584}',  // ▄
                (false, true) => '\u{2580}',  // ▀
                (false, false) => '\u{2588}', // █
            });
        }

        // Right margin
        for _ in 0..margin {
            result.push('\u{2588}');
        }
        result.push('\n');
        y += 2;
    }

    // Quiet zone bottom
    for _ in 0..margin {
        result.push_str(&light_row);
        result.push('\n');
    }

    Some(result)
}
