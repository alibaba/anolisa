#![forbid(unsafe_code)]

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
mod registry;
mod session;
mod skill;
mod sls;
mod state;
mod tool;
mod truncator;

use clap::Parser;

use config::CoreConfig;
use provider::openai_compat::OpenAICompatProvider;
use provider::profile;

fn create_provider(config: &CoreConfig) -> Box<dyn provider::ContentGenerator> {
    let resolved = config.resolve_provider();
    // Aliyun provider uses AK/SK, not API key
    if resolved.provider_type == "aliyun" {
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
    if resolved.provider_type == "aliyun" {
        return resolved.access_key_id.is_empty() || resolved.access_key_secret.is_empty();
    }
    resolved.api_key.is_empty()
}

#[tokio::main]
async fn main() {
    let args = cli::CliArgs::parse();
    let config = CoreConfig::load();

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
