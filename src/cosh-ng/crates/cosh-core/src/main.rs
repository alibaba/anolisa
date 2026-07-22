#![forbid(unsafe_code)]
#![allow(dead_code)]

mod auth;
mod cli;
mod compaction;
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
mod process;
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
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

use config::CoreConfig;
use provider::openai_compat::OpenAICompatProvider;
use provider::profile;

fn create_provider(config: &CoreConfig) -> Box<dyn provider::ContentGenerator> {
    let resolved = config.resolve_provider();
    if resolved.provider_type == "mock" {
        if resolved.model == "mock-partial-error" {
            return Box::new(provider::mock::MockProvider::partial_error());
        }
        if resolved.model == "mock-compact-summary" {
            // Deterministic bounded output for compaction lifecycle tests.
            return Box::new(provider::mock::MockProvider::repeat_text(
                "## Objective and constraints\n- deterministic mock summary",
            ));
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
    config.resolve_provider().auth_required()
}

#[cfg(unix)]
fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("failed to start async runtime: {error}");
            std::process::exit(1);
        });

    runtime.block_on(run_until_sigint());
    // Tokio reads stdin on a blocking thread, which cannot be cancelled while a pipe stays open.
    runtime.shutdown_timeout(Duration::from_millis(100));
}

#[cfg(not(unix))]
#[tokio::main]
async fn main() {
    run().await;
}

#[cfg(unix)]
async fn run_until_sigint() {
    let sigint_received = install_sigint_handler();

    tokio::select! {
        _ = wait_for_sigint(sigint_received) => {
            tracing::info!("received SIGINT, shutting down cosh-core");
        }
        _ = run() => {}
    }
}

async fn run() {
    let args = cli::CliArgs::parse();
    if args.is_session_control() {
        std::process::exit(session_control::run());
    }
    let workspace = args
        .workspace
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let config = if args.bare {
        CoreConfig::load_bare()
    } else {
        CoreConfig::load_for_workspace(&workspace)
    };

    let log_level = config.logging.effective_level(args.verbose);
    logging::init_logging(&log_level);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "cosh-core starting");

    if args.is_registry() {
        registry::run(&args, config).await;
    } else if args.is_compact() {
        std::process::exit(compaction::run_compact_cli(&args, config).await);
    } else if args.is_headless() {
        match headless::run(&args, config).await {
            Ok(0) => {}
            Ok(exit_code) => std::process::exit(exit_code),
            Err(error) => {
                eprintln!("[cosh-core] {error}");
                std::process::exit(2);
            }
        }
    } else {
        interactive::run(&args, config).await;
    }
}

#[cfg(unix)]
fn install_sigint_handler() -> Arc<AtomicBool> {
    let received = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&received)).unwrap_or_else(
        |error| {
            eprintln!("failed to install SIGINT handler: {error}");
            std::process::exit(1);
        },
    );
    received
}

#[cfg(unix)]
async fn wait_for_sigint(received: Arc<AtomicBool>) {
    while !received.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(10)).await;
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
