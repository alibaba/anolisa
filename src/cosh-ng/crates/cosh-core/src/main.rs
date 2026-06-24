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
mod loop_detect;
mod migrate;
mod protocol;
mod provider;
mod registry;
mod session;
mod skill;
mod state;
mod tool;
mod truncator;

use clap::Parser;

use config::CoreConfig;
use provider::openai_compat::OpenAICompatProvider;
use provider::profile;

fn create_provider(config: &CoreConfig) -> Box<dyn provider::ContentGenerator> {
    let resolved = config.resolve_provider();
    if resolved.api_key.is_empty() {
        eprintln!("[cosh-core] Warning: no API key configured, using mock provider");
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

/// Check if auth is needed (no API key configured).
fn needs_auth(config: &CoreConfig) -> bool {
    config.resolve_provider().api_key.is_empty()
}

#[tokio::main]
async fn main() {
    let args = cli::CliArgs::parse();
    let config = CoreConfig::load();

    if args.is_registry() {
        registry::run(&args, config).await;
    } else if args.is_headless() {
        headless::run(&args, config).await;
    } else {
        interactive::run(&args, config).await;
    }
}
