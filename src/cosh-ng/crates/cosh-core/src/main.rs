#![forbid(unsafe_code)]
#![allow(dead_code)]

mod auth;
mod cli;
mod compression;
mod config;
mod context;
mod core;
mod extension;
mod headless;
mod hook;
mod interactive;
mod logging;
mod loop_detect;
mod metrics;
mod migrate;
mod protocol;
mod provider;
mod redaction;
mod registry;
mod session;
mod session_control;
mod skill;
mod sls;
mod state;
mod tool;
mod truncator;

use clap::Parser;
use std::path::PathBuf;

use config::CoreConfig;
use provider::openai_compat::OpenAICompatProvider;
use provider::profile;

fn create_provider(config: &CoreConfig) -> Box<dyn provider::ContentGenerator> {
    let resolved = config.resolve_provider();
    if resolved.provider_type == "mock" {
        if resolved.model == "mock-partial-error" {
            return Box::new(provider::mock::MockProvider::partial_error());
        }
        return Box::new(provider::mock::MockProvider::history_echo());
    }
    // Aliyun provider uses AK/SK, not API key
    if resolved.provider_type == "aliyun" {
        if resolved.auth_source.as_deref() == Some("ecs_ram_role") {
            return Box::new(provider::sysom::SysomProvider::from_ecs_ram_role());
        }
        if resolved.access_key_id.is_empty() || resolved.access_key_secret.is_empty() {
            tracing::warn!("no AK/SK configured for aliyun, using mock provider");
            return Box::new(provider::mock::MockProvider::text_only(
                "No AK/SK configured. Please set ALIBABA_CLOUD_ACCESS_KEY_ID/SECRET or use /auth.",
            ));
        }
        return Box::new(provider::sysom::SysomProvider::new(
            &resolved.access_key_id,
            &resolved.access_key_secret,
            resolved.security_token.as_deref(),
        ));
    }
    if resolved.api_key.is_empty() {
        tracing::warn!("no API key configured, using mock provider");
        return Box::new(provider::mock::MockProvider::text_only(
            "No API key configured. Please set DASHSCOPE_API_KEY or configure [ai.providers] in config.toml.",
        ));
    }
    create_provider_from_resolved(&resolved)
}

fn create_provider_from_resolved(
    resolved: &config::ResolvedProvider,
) -> Box<dyn provider::ContentGenerator> {
    let provider_profile = profile::profile_from_name(&resolved.provider_type);
    Box::new(OpenAICompatProvider::new(
        &resolved.base_url,
        &resolved.api_key,
        provider_profile,
    ))
}

/// Check if auth is needed (no API key or AK/SK configured).
fn needs_auth(config: &CoreConfig) -> bool {
    let resolved = config.resolve_provider();
    if resolved.provider_type == "mock" {
        return false;
    }
    if resolved.provider_type == "aliyun" {
        if resolved.auth_source.as_deref() == Some("ecs_ram_role") {
            return false;
        }
        return resolved.access_key_id.is_empty() || resolved.access_key_secret.is_empty();
    }
    resolved.api_key.is_empty()
}

#[tokio::main]
async fn main() {
    let args = cli::CliArgs::parse();
    if args.is_session_control() {
        std::process::exit(session_control::run());
    }
    let workspace = args
        .workspace
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let config = CoreConfig::load_for_workspace(&workspace);

    let log_level = config.logging.effective_level(args.verbose);
    logging::init_logging(&log_level);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "cosh-core starting");

    if args.is_registry() {
        registry::run(&args, config).await;
    } else if args.is_headless() {
        headless::run(&args, config).await;
    } else {
        interactive::run(&args, config).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiConfig, CoreConfig, ProviderConfig};
    use std::collections::HashMap;

    #[test]
    fn ecs_ram_role_aliyun_provider_does_not_need_static_auth() {
        let old_ak = std::env::var("ALIBABA_CLOUD_ACCESS_KEY_ID").ok();
        let old_sk = std::env::var("ALIBABA_CLOUD_ACCESS_KEY_SECRET").ok();
        let old_token = std::env::var("ALIBABA_CLOUD_SECURITY_TOKEN").ok();
        std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_ID");
        std::env::remove_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET");
        std::env::remove_var("ALIBABA_CLOUD_SECURITY_TOKEN");

        let mut providers = HashMap::new();
        providers.insert(
            "aliyun-ecs".to_string(),
            ProviderConfig {
                provider_type: Some("aliyun".to_string()),
                auth_source: Some("ecs_ram_role".to_string()),
                model: Some("qwen3.7-plus".to_string()),
                ..Default::default()
            },
        );
        let config = CoreConfig {
            ai: AiConfig {
                active_provider: Some("aliyun-ecs".to_string()),
                providers,
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!needs_auth(&config));

        if let Some(value) = old_ak {
            std::env::set_var("ALIBABA_CLOUD_ACCESS_KEY_ID", value);
        }
        if let Some(value) = old_sk {
            std::env::set_var("ALIBABA_CLOUD_ACCESS_KEY_SECRET", value);
        }
        if let Some(value) = old_token {
            std::env::set_var("ALIBABA_CLOUD_SECURITY_TOKEN", value);
        }
    }
}
