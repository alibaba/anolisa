//! Cosh-core registry transport and response decoding for the auth runtime.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use super::ExistingProvider;
use crate::adapter::{AdapterInstance, CoshCoreAdapter};
use crate::auth::reset::{self, CoreAuthConfigureError};
use crate::runtime::prelude::{AuthFieldInfo, AuthProviderInfo, AuthResponse};

#[derive(Debug, Deserialize)]
pub(super) struct CoreAuthState {
    pub(super) templates: Vec<AuthProviderInfo>,
    #[serde(default)]
    pub(super) saved_providers: Vec<CoreSavedProvider>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CoreSavedProvider {
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
    #[serde(default)]
    credentials_unavailable: bool,
}

fn default_provider_source() -> String {
    "user".to_string()
}

fn default_provider_editable() -> bool {
    true
}

pub(super) fn load_core_auth_state(cosh_core: &CoshCoreAdapter) -> Result<CoreAuthState, String> {
    let value = cosh_core.registry_query("auth", "state", Value::Null)?;
    serde_json::from_value(value).map_err(|error| format!("invalid auth state: {error}"))
}

pub(super) fn activate(adapter: &AdapterInstance, provider_id: &str) -> Result<(), String> {
    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        return Err("auth registry requires cosh-core backend".to_string());
    };
    cosh_core
        .registry_query("auth", "activate", json!({ "provider_id": provider_id }))
        .map(|_| ())
}

pub(super) fn configure(
    adapter: &AdapterInstance,
    response: &AuthResponse,
) -> Result<(), CoreAuthConfigureError> {
    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        return Err(CoreAuthConfigureError::Other(
            "auth registry requires cosh-core backend".to_string(),
        ));
    };
    cosh_core
        .registry_query("auth", "configure", reset::configure_params(response))
        .map(|_| ())
        .map_err(reset::classify_core_configure_error)
}

#[derive(Debug, Deserialize)]
struct CoreAuthVerify {
    authorized: bool,
}

pub(super) fn verify_aliyun_ecs(adapter: &AdapterInstance) -> Result<bool, String> {
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
    let verify: CoreAuthVerify = serde_json::from_value(value)
        .map_err(|error| format!("invalid auth verify response: {error}"))?;
    Ok(verify.authorized)
}

#[derive(Debug, Deserialize)]
pub(super) struct CoreAuthPrepare {
    pub(super) mode: String,
    pub(super) instance_id: Option<String>,
    pub(super) console_url: Option<String>,
    #[serde(default)]
    pub(super) values: HashMap<String, String>,
}

pub(super) fn prepare(
    adapter: &AdapterInstance,
    provider_type: &str,
) -> Result<CoreAuthPrepare, String> {
    let AdapterInstance::CoshCore(cosh_core) = adapter else {
        return Err("auth registry requires cosh-core backend".to_string());
    };
    let value =
        cosh_core.registry_query("auth", "prepare", json!({ "provider_type": provider_type }))?;
    serde_json::from_value(value).map_err(|error| format!("invalid auth prepare response: {error}"))
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
            credentials_unavailable: provider.credentials_unavailable,
        }
    }
}

pub(super) fn providers_with_provider_id_field(
    providers: Vec<AuthProviderInfo>,
) -> Vec<AuthProviderInfo> {
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
