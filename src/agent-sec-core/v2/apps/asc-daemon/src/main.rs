use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use asc_daemon::{AppState, TokenVerifier, bind_socket, prepare_auth, serve};
use asc_daemon_core::PolicyService;
use asc_persistence_sqlite::SqlitePolicyStore;
use asc_policy_runtime::UnavailablePolicyAdapter;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

fn main() {
    if let Err(problem) = run() {
        eprintln!("asc-daemon: {problem}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = arguments.first().map(String::as_str) else {
        return Err("expected 'prepare-auth' or 'serve'".to_owned());
    };
    match command {
        "prepare-auth" => {
            let token_file = required_path(&arguments, "--token-file")?;
            prepare_auth(&token_file).map_err(|error| error.to_string())?;
            Ok(())
        }
        "serve" => {
            let socket = required_path(&arguments, "--socket")?;
            let database = required_path(&arguments, "--database")?;
            let token_file = required_path(&arguments, "--token-file")?;
            let shutdown = shutdown_flag()?;
            let tracer_provider = initialize_observability();
            let auth =
                Arc::new(TokenVerifier::load(&token_file).map_err(|error| error.to_string())?);
            let store =
                Arc::new(SqlitePolicyStore::open(&database).map_err(|error| error.to_string())?);
            let adapter = Arc::new(UnavailablePolicyAdapter);
            let policy = Arc::new(PolicyService::new(store, adapter));
            let state = AppState::new(policy, auth);
            let listener = bind_socket(&socket).map_err(|error| error.to_string())?;
            let result = serve(&listener, &state, &shutdown).map_err(|error| error.to_string());
            drop(listener);
            tracer_provider
                .shutdown()
                .map_err(|error| format!("OpenTelemetry shutdown failed: {error}"))?;
            result
        }
        _ => Err("expected 'prepare-auth' or 'serve'".to_owned()),
    }
}

fn shutdown_flag() -> Result<Arc<AtomicBool>, String> {
    let shutdown = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGTERM, Arc::clone(&shutdown))
        .map_err(|error| format!("failed to register SIGTERM handler: {error}"))?;
    signal_hook::flag::register(SIGINT, Arc::clone(&shutdown))
        .map_err(|error| format!("failed to register SIGINT handler: {error}"))?;
    Ok(shutdown)
}

fn initialize_observability() -> SdkTracerProvider {
    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("asc-daemon");
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("asc_daemon=info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().json())
        .with(telemetry)
        .init();
    provider
}

fn required_path(arguments: &[String], name: &str) -> Result<PathBuf, String> {
    let Some(index) = arguments.iter().position(|value| value == name) else {
        return Err(format!("missing {name}"));
    };
    arguments
        .get(index + 1)
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing value for {name}"))
}
