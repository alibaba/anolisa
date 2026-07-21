use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::adapter::{AdapterInstance, CoshCoreAdapter};
use crate::auth::completion::finish_auth_configuration;
use crate::auth::provider_display::auth_required_providers_for_display;
use crate::runtime::dispatcher::stable_event_key;
use crate::runtime::prelude::{
    AgentEvent, AuthFieldInfo, AuthProviderInfo, AuthResponse, GovernedEvent, NoticePanelModel,
    QuestionPanelModel, QuestionSelectionMode, RatatuiInlineRenderer, RawInputCapture, ShellEvent,
    ShellEventKind,
};
use crate::runtime::state::InlineState;

/// An existing provider loaded from config.toml for the ManagingProviders phase.
#[derive(Debug, Clone)]
pub(crate) struct ExistingProvider {
    pub(crate) name: String,          // section name (e.g. "default")
    pub(crate) provider_type: String, // type field value
    pub(crate) label: String,         // display name based on type
    pub(crate) model: String,         // current model
    pub(crate) is_active: bool,       // whether this is the active_provider
    pub(crate) editable: bool,
    pub(crate) source: String,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key_mask: Option<String>,
    pub(crate) access_key_id_mask: Option<String>,
    pub(crate) access_key_secret_mask: Option<String>,
    pub(crate) security_token_mask: Option<String>,
    pub(crate) auth_source: Option<String>,
}

fn secret_mask(len: usize) -> String {
    "•".repeat(len)
}

fn label_for_provider_type(provider_type: &str) -> &'static str {
    match provider_type {
        "dashscope" => "DashScope (\u{767e}\u{70bc})",
        "aliyun" => "Aliyun Authentication",
        _ => "OpenAI Compatible",
    }
}

fn provider_action_options(is_active: bool, editable: bool) -> Vec<String> {
    match (is_active, editable) {
        (true, true) => vec!["Edit configuration".to_string(), "Cancel".to_string()],
        (true, false) => vec!["Cancel".to_string()],
        (false, true) => vec![
            "Set as active provider".to_string(),
            "Edit configuration".to_string(),
            "Cancel".to_string(),
        ],
        (false, false) => vec!["Set as active provider".to_string(), "Cancel".to_string()],
    }
}

fn provider_action_choice(is_active: bool, editable: bool, selected: usize) -> &'static str {
    match (is_active, editable, selected) {
        (true, true, 0) => "edit",
        (true, _, _) => "cancel",
        (false, _, 0) => "activate",
        (false, true, 1) => "edit",
        _ => "cancel",
    }
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
    backend: AuthBackend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthBackend {
    ActiveRun,
    CoreRegistry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthPhase {
    /// Show existing providers list + "Add new" option
    ManagingProviders,
    /// Action menu after selecting an existing provider
    ProviderAction {
        provider_idx: usize,
    },
    SelectingProvider,
    FillingField,
    AliyunEcsChallenge {
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
            let providers = auth_required_providers_for_display(providers);
            state.auth.state = Some(RuntimeAuthState {
                id: id.clone(),
                request_id: request_id.clone(),
                phase: AuthPhase::SelectingProvider,
                providers,
                selected_provider: 0,
                current_field: 0,
                collected_values: HashMap::new(),
                field_input: String::new(),
                existing_providers: Vec::new(),
                editing_provider_name: None,
                backend: AuthBackend::ActiveRun,
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
            option_count: auth.existing_providers.len() + 1,
            allow_free_text: false,
            multiple: false,
            secret: false,
        }),
        AuthPhase::ProviderAction { provider_idx } => {
            let existing = auth.existing_providers.get(*provider_idx);
            let option_count = provider_action_options(
                existing.is_some_and(|provider| provider.is_active),
                existing.map(|provider| provider.editable).unwrap_or(true),
            )
            .len();
            Some(RawInputCapture::Question {
                id: auth.id.clone(),
                option_count,
                allow_free_text: false,
                multiple: false,
                secret: false,
            })
        }
        AuthPhase::SelectingProvider => Some(RawInputCapture::Question {
            id: auth.id.clone(),
            option_count: auth.providers.len(),
            allow_free_text: false,
            multiple: false,
            secret: false,
        }),
        AuthPhase::FillingField => {
            let secret = auth
                .providers
                .get(auth.selected_provider)
                .and_then(|provider| provider.fields.get(auth.current_field))
                .is_some_and(|field| field.secret);
            Some(RawInputCapture::Question {
                id: auth.id.clone(),
                option_count: 0,
                allow_free_text: true,
                multiple: false,
                secret,
            })
        }
        AuthPhase::AliyunEcsChallenge { .. } => Some(RawInputCapture::Question {
            id: auth.id.clone(),
            option_count: 1,
            allow_free_text: false,
            multiple: false,
            secret: false,
        }),
    }
}

pub(crate) fn has_pending_auth(state: &InlineState) -> bool {
    state.auth.state.is_some()
}

/// Trigger auth panel from `/auth` slash command.
/// Now starts in ManagingProviders phase to show existing providers.
pub(crate) fn trigger_auth_from_slash<W: std::io::Write>(
    adapter: &AdapterInstance,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if state.auth.state.is_some() {
        return Ok(());
    }
    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
        renderer.write_notice_panel(
            output,
            NoticePanelModel {
                title: "Auth unavailable",
                body: vec![
                    "Authentication is managed by cosh-core.".to_string(),
                    "Switch to the cosh-core backend before running /auth.".to_string(),
                ],
                footer: None,
            },
        )?;
        return Ok(());
    };

    let core_state = match load_core_auth_state(cosh_core) {
        Ok(state) => state,
        Err(message) => {
            let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
            renderer.write_notice_panel(
                output,
                NoticePanelModel {
                    title: "Auth unavailable",
                    body: vec![
                        "Unable to read auth state from cosh-core.".to_string(),
                        message,
                    ],
                    footer: None,
                },
            )?;
            return Ok(());
        }
    };

    let providers = providers_with_provider_id_field(core_state.templates);
    let request_id = format!(
        "slash-auth-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let id = format!("auth-{request_id}");

    let mut existing_providers: Vec<ExistingProvider> = core_state
        .saved_providers
        .into_iter()
        .map(ExistingProvider::from)
        .collect();
    existing_providers.sort_by(|a, b| b.is_active.cmp(&a.is_active).then(a.name.cmp(&b.name)));

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
        backend: AuthBackend::CoreRegistry,
    });

    render_current_auth_panel(state, output)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct CoreAuthState {
    templates: Vec<AuthProviderInfo>,
    #[serde(default)]
    saved_providers: Vec<CoreSavedProvider>,
}

#[derive(Debug, Deserialize)]
struct CoreSavedProvider {
    provider_id: String,
    provider_type: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    auth_source: Option<String>,
    active: bool,
    #[serde(default = "default_provider_source")]
    source: String,
    #[serde(default = "default_provider_editable")]
    editable: bool,
    #[serde(default)]
    api_key_len: usize,
    #[serde(default)]
    access_key_id_len: usize,
    #[serde(default)]
    access_key_secret_len: usize,
    #[serde(default)]
    security_token_len: usize,
}

fn default_provider_source() -> String {
    "user".to_string()
}

fn default_provider_editable() -> bool {
    true
}

fn load_core_auth_state(cosh_core: &CoshCoreAdapter) -> Result<CoreAuthState, String> {
    let value = cosh_core.registry_query("auth", "state", Value::Null)?;
    serde_json::from_value(value).map_err(|e| format!("invalid auth state: {e}"))
}

fn core_auth_activate(adapter: &AdapterInstance, provider_id: &str) -> Result<(), String> {
    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        return Err("auth registry requires cosh-core backend".to_string());
    };
    cosh_core
        .registry_query("auth", "activate", json!({ "provider_id": provider_id }))
        .map(|_| ())
}

fn core_auth_configure(adapter: &AdapterInstance, response: &AuthResponse) -> Result<(), String> {
    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        return Err("auth registry requires cosh-core backend".to_string());
    };
    cosh_core
        .registry_query(
            "auth",
            "configure",
            json!({
                "provider_id": response.provider_id,
                "provider_type": response.provider_type,
                "values": response.values,
            }),
        )
        .map(|_| ())
}

#[derive(Debug, Deserialize)]
struct CoreAuthVerify {
    authorized: bool,
}

fn core_auth_verify_aliyun_ecs(adapter: &AdapterInstance) -> Result<bool, String> {
    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        return Err("auth registry requires cosh-core backend".to_string());
    };
    let value = cosh_core.registry_query(
        "auth",
        "verify",
        json!({
            "provider_type": "aliyun",
            "auth_source": "ecs_ram_role"
        }),
    )?;
    let verify: CoreAuthVerify =
        serde_json::from_value(value).map_err(|e| format!("invalid auth verify response: {e}"))?;
    Ok(verify.authorized)
}

#[derive(Debug, Deserialize)]
struct CoreAuthPrepare {
    mode: String,
    instance_id: Option<String>,
    console_url: Option<String>,
    #[serde(default)]
    values: HashMap<String, String>,
}

fn core_auth_prepare(
    adapter: &AdapterInstance,
    provider_type: &str,
) -> Result<CoreAuthPrepare, String> {
    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        return Err("auth registry requires cosh-core backend".to_string());
    };
    let value =
        cosh_core.registry_query("auth", "prepare", json!({ "provider_type": provider_type }))?;
    serde_json::from_value(value).map_err(|e| format!("invalid auth prepare response: {e}"))
}

impl From<CoreSavedProvider> for ExistingProvider {
    fn from(provider: CoreSavedProvider) -> Self {
        let provider_type = provider
            .provider_type
            .unwrap_or_else(|| "openai_compat".to_string());
        let model = provider.model.unwrap_or_default();
        ExistingProvider {
            name: provider.provider_id,
            label: label_for_provider_type(&provider_type).to_string(),
            provider_type,
            model,
            is_active: provider.active,
            editable: provider.editable,
            source: provider.source,
            base_url: provider.base_url,
            api_key_mask: (provider.api_key_len > 0).then(|| secret_mask(provider.api_key_len)),
            access_key_id_mask: (provider.access_key_id_len > 0)
                .then(|| secret_mask(provider.access_key_id_len)),
            access_key_secret_mask: (provider.access_key_secret_len > 0)
                .then(|| secret_mask(provider.access_key_secret_len)),
            security_token_mask: (provider.security_token_len > 0)
                .then(|| secret_mask(provider.security_token_len)),
            auth_source: provider.auth_source,
        }
    }
}

fn providers_with_provider_id_field(providers: Vec<AuthProviderInfo>) -> Vec<AuthProviderInfo> {
    providers
        .into_iter()
        .map(|mut provider| {
            provider.fields.insert(
                0,
                AuthFieldInfo {
                    name: "provider_id".to_string(),
                    label: "Provider ID".to_string(),
                    hint: Some("Unique config id, e.g. qwen-prod".to_string()),
                    secret: false,
                    required: true,
                    placeholder: Some(provider.id.clone()),
                },
            );
            provider
        })
        .collect()
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
    adapter: &AdapterInstance,
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
            let editable = existing.editable;

            let action = provider_action_choice(is_active, editable, auth.selected_provider);

            match action {
                "activate" => {
                    core_auth_activate(adapter, &existing.name).map_err(std::io::Error::other)?;
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

                    auth.collected_values
                        .insert("provider_id".to_string(), existing.name.clone());
                    if let Some(ref v) = existing.api_key_mask {
                        auth.collected_values
                            .insert("api_key".to_string(), v.clone());
                    }
                    if let Some(ref v) = existing.base_url {
                        auth.collected_values
                            .insert("base_url".to_string(), v.clone());
                    }
                    if !existing.model.is_empty() {
                        auth.collected_values
                            .insert("model".to_string(), existing.model.clone());
                    }
                    if let Some(ref v) = existing.access_key_id_mask {
                        auth.collected_values
                            .insert("access_key_id".to_string(), v.clone());
                    }
                    if let Some(ref v) = existing.access_key_secret_mask {
                        auth.collected_values
                            .insert("access_key_secret".to_string(), v.clone());
                    }
                    if let Some(ref v) = existing.security_token_mask {
                        auth.collected_values
                            .insert("security_token".to_string(), v.clone());
                    }
                    if let Some(ref v) = existing.auth_source {
                        auth.collected_values
                            .insert("auth_source".to_string(), v.clone());
                    }

                    if should_apply_aliyun_prepare_for_edit(&existing) {
                        if apply_aliyun_prepare(adapter, auth).map_err(std::io::Error::other)? {
                            clear_active_auth_panel(state, output)?;
                            render_current_auth_panel(state, output)?;
                            return Ok(true);
                        }
                        clear_ecs_auth_source_for_manual_aliyun_edit(
                            &existing,
                            &mut auth.collected_values,
                        );
                    }

                    auth.phase = AuthPhase::FillingField;
                    auth.current_field = 1.min(auth.current_provider().fields.len());
                    load_current_field_input(auth);
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
            if auth.current_provider().id == "aliyun"
                && should_apply_aliyun_prepare_on_provider_selection(auth.backend)
                && apply_aliyun_prepare(adapter, auth).map_err(std::io::Error::other)?
            {
                clear_active_auth_panel(state, output)?;
                render_current_auth_panel(state, output)?;
                return Ok(true);
            }
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
            let field = auth.current_field_info().cloned();
            if let Some(field) = field.clone() {
                auth.collected_values.insert(field.name.clone(), value);
            }
            if should_apply_aliyun_prepare_after_field(
                auth.backend,
                auth.editing_provider_name.is_some(),
                auth.current_provider().id.as_str(),
                field.as_ref().map(|f| f.name.as_str()),
            ) && apply_aliyun_prepare(adapter, auth).map_err(std::io::Error::other)?
            {
                clear_active_auth_panel(state, output)?;
                render_current_auth_panel(state, output)?;
                return Ok(true);
            }
            auth.current_field += 1;
            // Load next field's pre-filled value (for edit mode)
            load_current_field_input(auth);

            if auth.all_fields_collected() {
                clear_active_auth_panel(state, output)?;
                send_auth_response(Some(adapter), state, output)?;
                Ok(true)
            } else {
                clear_active_auth_panel(state, output)?;
                render_current_auth_panel(state, output)?;
                Ok(true)
            }
        }
        AuthPhase::AliyunEcsChallenge { .. } => {
            if !core_auth_verify_aliyun_ecs(adapter).map_err(std::io::Error::other)? {
                clear_active_auth_panel(state, output)?;
                let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
                renderer.write_notice_panel(
                    output,
                    NoticePanelModel {
                        title: "Aliyun authorization pending",
                        body: vec![
                            "ECS RAM Role credentials are not available yet.".to_string(),
                            "Open the authorization link or scan the QR code, then confirm again."
                                .to_string(),
                        ],
                        footer: None,
                    },
                )?;
                render_current_auth_panel(state, output)?;
                return Ok(true);
            }
            clear_active_auth_panel(state, output)?;
            send_auth_response(Some(adapter), state, output)?;
            Ok(true)
        }
    }
}

fn load_current_field_input(auth: &mut RuntimeAuthState) {
    let field_name = auth.current_field_info().map(|f| f.name.clone());
    if let Some(name) = field_name {
        auth.field_input = auth
            .collected_values
            .get(&name)
            .cloned()
            .unwrap_or_default();
    } else {
        auth.field_input.clear();
    }
}

fn should_apply_aliyun_prepare_on_provider_selection(backend: AuthBackend) -> bool {
    backend == AuthBackend::ActiveRun
}

fn should_apply_aliyun_prepare_after_field(
    backend: AuthBackend,
    is_editing: bool,
    provider_type: &str,
    field_name: Option<&str>,
) -> bool {
    backend == AuthBackend::CoreRegistry
        && !is_editing
        && provider_type == "aliyun"
        && field_name == Some("provider_id")
}

fn should_apply_aliyun_prepare_for_edit(existing: &ExistingProvider) -> bool {
    existing.provider_type == "aliyun" && existing.auth_source.as_deref() == Some("ecs_ram_role")
}

fn clear_ecs_auth_source_for_manual_aliyun_edit(
    existing: &ExistingProvider,
    values: &mut HashMap<String, String>,
) {
    if should_apply_aliyun_prepare_for_edit(existing) {
        values.remove("auth_source");
    }
}

fn apply_aliyun_prepare(
    adapter: &AdapterInstance,
    auth: &mut RuntimeAuthState,
) -> Result<bool, String> {
    let prepare = core_auth_prepare(adapter, "aliyun")?;
    if prepare.mode != "ecs_ram_role" {
        return Ok(false);
    }
    for (key, value) in prepare.values {
        auth.collected_values.insert(key, value);
    }
    auth.collected_values.remove("access_key_id");
    auth.collected_values.remove("access_key_secret");
    auth.collected_values.remove("security_token");
    auth.phase = AuthPhase::AliyunEcsChallenge {
        instance_id: prepare.instance_id.unwrap_or_default(),
        console_url: prepare.console_url.unwrap_or_default(),
    };
    Ok(true)
}

fn send_auth_response<W: std::io::Write>(
    adapter: Option<&AdapterInstance>,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let auth = state.auth.state.take().expect("auth state present");
    state.auth.completed_ids.insert(auth.id.clone());
    let provider = &auth.providers[auth.selected_provider];
    let provider_id = auth
        .editing_provider_name
        .clone()
        .or_else(|| auth.collected_values.get("provider_id").cloned())
        .unwrap_or_else(|| provider.id.clone());
    let response = AuthResponse {
        request_id: auth.request_id.clone(),
        provider_id: provider_id.clone(),
        provider_type: Some(provider.id.clone()),
        values: auth.collected_values,
        persist: true,
    };

    if let Some(active_run) = state.agent_run.active.as_ref() {
        if active_run.handle.respond_auth(response.clone()).is_err() {
            let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
            renderer.write_notice_panel(
                output,
                NoticePanelModel {
                    title: "Auth failed",
                    body: vec![
                        "Unable to send credentials to cosh-core.".to_string(),
                        "Run /auth again after the current run finishes.".to_string(),
                    ],
                    footer: None,
                },
            )?;
            output.flush()?;
            return Ok(());
        }
    } else {
        match auth.backend {
            AuthBackend::CoreRegistry => {
                let adapter = adapter.ok_or_else(|| {
                    std::io::Error::other("missing adapter for cosh-core auth registry")
                })?;
                core_auth_configure(adapter, &response).map_err(std::io::Error::other)?;
            }
            AuthBackend::ActiveRun => {}
        }
    }

    finish_auth_configuration(state, output, &provider.label)
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
                    let source_info = if ep.source == "system" {
                        " [system]"
                    } else {
                        ""
                    };
                    format!(
                        "{}{} - \"{}\"{}{}",
                        active_mark, ep.label, ep.name, model_info, source_info
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
            let title = format!("\u{1f511} {} \u{2014} \"{}\":", ep.label, ep.name);
            let options = provider_action_options(ep.is_active, ep.editable);
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
            let mut question = format!(
                "\u{1f511} {} \u{2014} {} {}:",
                provider.label, action, label
            );
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
        AuthPhase::AliyunEcsChallenge {
            ref instance_id,
            ref console_url,
        } => {
            let mut question = format!(
                "\u{1f511} Aliyun Authentication \u{2014} Authorize ECS RAM Role\n  ECS Instance ID: {instance_id}\n  URL: {console_url}"
            );
            if let Some(qr) = generate_qr_text(console_url) {
                question.push_str("\n\n");
                question.push_str(&qr);
            }
            let options = vec!["I have authorized this ECS instance".to_string()];
            let model = QuestionPanelModel {
                id: &auth.id,
                question: &question,
                options: &options,
                selected_option: 0,
                selected_options: &[],
                custom_answer: "",
                allow_free_text: false,
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
    adapter: &AdapterInstance,
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
        if !is_auth_card_component(event.component.as_deref()) {
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
                        handle_auth_answer(adapter, state, &id, answer, output)?;
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

fn is_auth_card_component(component: Option<&str>) -> bool {
    matches!(component, Some("card") | Some("card_secret"))
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

fn generate_qr_text(data: &str) -> Option<String> {
    use qrcode::QrCode;

    let code = QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let margin = 2usize;
    let total_width = width + 2 * margin;
    let light_row: String = "\u{2588}".repeat(total_width);
    let mut result = String::new();

    for _ in 0..margin {
        result.push_str(&light_row);
        result.push('\n');
    }

    let mut y = 0;
    while y < width {
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
                (true, false) => '\u{2584}',
                (false, true) => '\u{2580}',
                (false, false) => '\u{2588}',
            });
        }
        for _ in 0..margin {
            result.push('\u{2588}');
        }
        result.push('\n');
        y += 2;
    }

    for _ in 0..margin {
        result.push_str(&light_row);
        result.push('\n');
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::{
        clear_ecs_auth_source_for_manual_aliyun_edit, provider_action_choice,
        provider_action_options, should_apply_aliyun_prepare_after_field,
        should_apply_aliyun_prepare_for_edit, should_apply_aliyun_prepare_on_provider_selection,
        AuthBackend, ExistingProvider,
    };
    use std::collections::HashMap;

    #[test]
    fn provider_action_options_hide_edit_for_non_editable_providers() {
        assert_eq!(
            provider_action_options(true, true),
            vec!["Edit configuration", "Cancel"]
        );
        assert_eq!(provider_action_options(true, false), vec!["Cancel"]);
        assert_eq!(
            provider_action_options(false, true),
            vec!["Set as active provider", "Edit configuration", "Cancel"]
        );
        assert_eq!(
            provider_action_options(false, false),
            vec!["Set as active provider", "Cancel"]
        );
    }

    #[test]
    fn provider_action_choice_never_edits_non_editable_providers() {
        assert_eq!(provider_action_choice(true, true, 0), "edit");
        assert_eq!(provider_action_choice(true, false, 0), "cancel");
        assert_eq!(provider_action_choice(false, true, 0), "activate");
        assert_eq!(provider_action_choice(false, true, 1), "edit");
        assert_eq!(provider_action_choice(false, false, 0), "activate");
        assert_eq!(provider_action_choice(false, false, 1), "cancel");
    }

    #[test]
    fn core_registry_aliyun_add_waits_for_provider_id_before_prepare() {
        assert!(!should_apply_aliyun_prepare_on_provider_selection(
            AuthBackend::CoreRegistry
        ));
        assert!(should_apply_aliyun_prepare_after_field(
            AuthBackend::CoreRegistry,
            false,
            "aliyun",
            Some("provider_id"),
        ));
    }

    #[test]
    fn active_run_aliyun_selection_can_prepare_without_provider_id_field() {
        assert!(should_apply_aliyun_prepare_on_provider_selection(
            AuthBackend::ActiveRun
        ));
        assert!(!should_apply_aliyun_prepare_after_field(
            AuthBackend::ActiveRun,
            false,
            "aliyun",
            Some("provider_id"),
        ));
    }

    #[test]
    fn manual_aliyun_edit_does_not_apply_ecs_prepare() {
        let manual = ExistingProvider {
            name: "aliyun-manual".to_string(),
            provider_type: "aliyun".to_string(),
            label: "Aliyun Authentication".to_string(),
            model: "qwen3.7-plus".to_string(),
            is_active: true,
            editable: true,
            source: "user".to_string(),
            base_url: None,
            api_key_mask: None,
            access_key_id_mask: Some("••••".to_string()),
            access_key_secret_mask: Some("••••••".to_string()),
            security_token_mask: None,
            auth_source: None,
        };
        let ecs = ExistingProvider {
            auth_source: Some("ecs_ram_role".to_string()),
            access_key_id_mask: None,
            access_key_secret_mask: None,
            ..manual.clone()
        };

        assert!(!should_apply_aliyun_prepare_for_edit(&manual));
        assert!(should_apply_aliyun_prepare_for_edit(&ecs));
    }

    #[test]
    fn ecs_aliyun_manual_fallback_clears_auth_source() {
        let ecs = ExistingProvider {
            name: "aliyun-ecs".to_string(),
            provider_type: "aliyun".to_string(),
            label: "Aliyun Authentication".to_string(),
            model: "qwen3.7-plus".to_string(),
            is_active: true,
            editable: true,
            source: "user".to_string(),
            base_url: None,
            api_key_mask: None,
            access_key_id_mask: None,
            access_key_secret_mask: None,
            security_token_mask: None,
            auth_source: Some("ecs_ram_role".to_string()),
        };
        let manual = ExistingProvider {
            auth_source: None,
            ..ecs.clone()
        };
        let mut ecs_values = HashMap::from([
            ("auth_source".to_string(), "ecs_ram_role".to_string()),
            ("access_key_id".to_string(), "manual-ak".to_string()),
            ("access_key_secret".to_string(), "manual-sk".to_string()),
            ("security_token".to_string(), "manual-token".to_string()),
        ]);
        let mut manual_values = ecs_values.clone();

        clear_ecs_auth_source_for_manual_aliyun_edit(&ecs, &mut ecs_values);
        clear_ecs_auth_source_for_manual_aliyun_edit(&manual, &mut manual_values);

        assert!(!ecs_values.contains_key("auth_source"));
        assert_eq!(
            ecs_values.get("access_key_id").map(String::as_str),
            Some("manual-ak")
        );
        assert_eq!(
            ecs_values.get("access_key_secret").map(String::as_str),
            Some("manual-sk")
        );
        assert_eq!(
            ecs_values.get("security_token").map(String::as_str),
            Some("manual-token")
        );
        assert_eq!(
            manual_values.get("auth_source").map(String::as_str),
            Some("ecs_ram_role")
        );
    }
}
