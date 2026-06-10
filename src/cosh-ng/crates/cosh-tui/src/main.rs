#![forbid(unsafe_code)]

mod cli;
mod compression;
mod config;
mod context;
mod core;
mod headless;
mod hook;
mod interactive;
mod loop_detect;
mod migrate;
mod protocol;
mod provider;
mod session;
mod tool;
mod truncator;

use clap::Parser;

use config::CoreConfig;
use provider::openai_compat::OpenAICompatProvider;
use provider::profile;

fn create_provider(config: &CoreConfig) -> Box<dyn provider::ContentGenerator> {
    let resolved = config.resolve_provider();
    if resolved.api_key.is_empty() {
        eprintln!("[cosh-tui] Warning: no API key configured, using mock provider");
        return Box::new(provider::mock::MockProvider::text_only(
            "No API key configured. Please set DASHSCOPE_API_KEY or configure [ai.providers] in config.toml.",
        ));
    }
    let provider_profile = profile::profile_from_name(&resolved.provider_type);
    Box::new(OpenAICompatProvider::new(
        &resolved.base_url,
        &resolved.api_key,
        provider_profile,
    ))
}

#[tokio::main]
async fn main() {
    let args = cli::CliArgs::parse();
    let config = CoreConfig::load();

    if args.is_headless() {
        headless::run(&args, config).await;
    } else {
        interactive::run(&args, config).await;
    }
}
