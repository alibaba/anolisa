//! API server module
//!
//! Provides a lightweight HTTP API server using actix-web for querying
//! AgentSight storage data, and optionally serves the embedded frontend.

mod handlers;
mod token_savings;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use actix_cors::Cors;
use actix_web::{App, HttpRequest, HttpResponse, HttpServer, Responder, get, web};
use include_dir::{Dir, include_dir};

use crate::health::{HealthChecker, HealthStore};
use crate::storage::sqlite::InterruptionStore;

/// Embedded frontend static files (built from dashboard/ via `npm run build:embed`)
/// The directory `frontend-dist/` must exist at compile time; if it is absent
/// (e.g. first build before running npm), Rust will use an empty dir.
static FRONTEND: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/frontend-dist");

/// agent-sec security observability integration configuration.
#[derive(Clone, Debug)]
pub struct SecurityObservabilityConfig {
    /// Per-request daemon timeout.
    pub timeout_ms: u64,
}

impl Default for SecurityObservabilityConfig {
    fn default() -> Self {
        Self { timeout_ms: 5_000 }
    }
}

/// Shared application state accessible from all handlers
pub struct AppState {
    /// Path to the SQLite database file
    pub storage_path: PathBuf,
    /// Server start time (for uptime calculation)
    pub start_time: Instant,
    /// Shared health store populated by the background HealthChecker
    pub health_store: Arc<RwLock<HealthStore>>,
    /// Interruption events store
    pub interruption_store: Option<Arc<InterruptionStore>>,
    /// agent-sec security observability integration configuration
    pub security_observability: SecurityObservabilityConfig,
}

// ─── Static file handler ─────────────────────────────────────────────────────

/// Serve embedded frontend files.
/// Any path that doesn't start with /api or /health is treated as a static
/// asset; unknown paths fall back to index.html (SPA client-side routing).
#[get("/")]
async fn serve_frontend_root() -> impl Responder {
    serve_frontend_path("")
}

#[get("/{tail:.*}")]
async fn serve_frontend(req: HttpRequest) -> impl Responder {
    let path = req.match_info().get("tail").unwrap_or("");
    serve_frontend_path(path)
}

fn serve_frontend_path(path: &str) -> HttpResponse {
    // Try exact match first
    let file = if path.is_empty() {
        FRONTEND.get_file("index.html")
    } else {
        FRONTEND.get_file(path)
    };

    match file {
        Some(f) => {
            let mime = if path.is_empty() {
                "text/html; charset=utf-8"
            } else {
                mime_for_path(path)
            };
            HttpResponse::Ok().content_type(mime).body(f.contents())
        }
        None => {
            // SPA fallback: return index.html for unmatched paths
            match FRONTEND.get_file("index.html") {
                Some(index) => HttpResponse::Ok()
                    .content_type("text/html; charset=utf-8")
                    .body(index.contents()),
                None => HttpResponse::NotFound()
                    .body("Frontend not embedded. Run `npm run build:embed` first."),
            }
        }
    }
}

fn mime_for_path(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg
        // API routes (registered before the catch-all static handler)
        .service(handlers::health)
        .service(handlers::metrics)
        .service(handlers::list_sessions)
        .service(handlers::list_traces_by_session)
        .service(handlers::get_trace_detail)
        .service(handlers::get_conversation_events)
        .service(handlers::list_agent_names)
        .service(handlers::get_timeseries)
        .service(handlers::export_atif_trace)
        .service(handlers::export_atif_session)
        .service(handlers::export_atif_conversation)
        .service(handlers::get_agent_health)
        .service(handlers::delete_agent_health)
        .service(handlers::restart_agent_health)
        // Interruption API routes
        .service(handlers::list_interruptions)
        .service(handlers::interruption_count)
        .service(handlers::interruption_stats)
        .service(handlers::interruption_session_counts)
        .service(handlers::interruption_conversation_counts)
        .service(handlers::list_session_interruptions)
        .service(handlers::list_conversation_interruptions)
        .service(handlers::resolve_interruption)
        .service(handlers::get_interruption)
        .service(token_savings::get_token_savings)
        .service(token_savings::get_session_savings)
        // agent-sec Security Observability API routes
        .service(handlers::security_status)
        .service(handlers::security_summary)
        .service(handlers::security_events_count_by)
        .service(handlers::security_events_list)
        .service(handlers::security_event_detail)
        .service(handlers::security_observability_sessions)
        .service(handlers::security_observability_runs)
        .service(handlers::security_observability_timeline)
        // Skill Metrics API routes
        .service(handlers::skill_metrics_all)
        .service(handlers::skill_metrics_downloads)
        .service(handlers::skill_metrics_loads)
        .service(handlers::skill_metrics_usage_ratio)
        .service(handlers::skill_metrics_distribution)
        .service(handlers::skill_metrics_hotness)
        // Frontend static files (catch-all, must be last)
        .service(serve_frontend_root)
        .service(serve_frontend);
}

// ─── Server entry point ───────────────────────────────────────────────────────

/// Start the API server
///
/// Binds to the given host:port and serves API endpoints + embedded frontend.
/// This function blocks until the server is shut down.
pub async fn run_server(host: &str, port: u16, storage_path: PathBuf) -> std::io::Result<()> {
    let security_observability = SecurityObservabilityConfig::default();

    // Initialize GenAI SQLite store (needed for HealthChecker to query pending calls)
    let genai_store: Option<Arc<crate::storage::sqlite::GenAISqliteStore>> =
        match crate::storage::sqlite::GenAISqliteStore::new() {
            Ok(store) => {
                log::info!("GenAI SQLite store initialized for HealthChecker");
                Some(Arc::new(store))
            }
            Err(e) => {
                log::warn!("Failed to initialize GenAI store for HealthChecker: {e}");
                None
            }
        };

    // Initialize interruption store
    let interruption_store: Option<Arc<InterruptionStore>> = {
        use crate::storage::sqlite::GenAISqliteStore;
        let db_path = GenAISqliteStore::default_path()
            .parent()
            .unwrap_or(std::path::Path::new("/var/log/sysak/.agentsight"))
            .join("interruption_events.db");
        match InterruptionStore::new_with_path(&db_path) {
            Ok(store) => {
                log::info!("Interruption store initialized at {db_path:?}");
                Some(Arc::new(store))
            }
            Err(e) => {
                log::warn!("Failed to open interruption store: {e}");
                None
            }
        }
    };

    // Spin up the background health checker
    let health_store = Arc::new(RwLock::new(HealthStore::new()));
    let mut checker = HealthChecker::new(Arc::clone(&health_store), Duration::from_secs(30));
    if let Some(ref istore) = interruption_store {
        checker = checker.with_interruption_store(Arc::clone(istore));
    }
    if let Some(ref gstore) = genai_store {
        checker = checker.with_genai_store(Arc::clone(gstore));
    }
    checker.start();

    let data = web::Data::new(AppState {
        storage_path,
        start_time: Instant::now(),
        health_store,
        interruption_store,
        security_observability,
    });

    let has_frontend = FRONTEND.get_file("index.html").is_some();
    log::info!("AgentSight API server listening on http://{host}:{port}");
    eprintln!("AgentSight API server listening on http://{host}:{port}");
    if has_frontend {
        eprintln!("Dashboard UI: http://{host}:{port}/");
    } else {
        eprintln!(
            "[WARN] Frontend not embedded. Run `npm run build:embed` in dashboard/ then recompile."
        );
    }

    HttpServer::new(move || {
        let cors = Cors::default()
            .allow_any_origin()
            .allowed_methods(vec!["GET", "DELETE", "POST", "OPTIONS"])
            .allowed_headers(vec!["Content-Type"])
            .max_age(3600);

        App::new()
            .wrap(cors)
            .app_data(data.clone())
            .configure(configure_routes)
    })
    .bind((host, port))?
    .run()
    .await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};
    use std::time::Instant;

    use actix_web::http::StatusCode;
    use actix_web::test as awtest;
    use actix_web::{App, web};

    use crate::health::HealthStore;

    use super::{
        AppState, SecurityObservabilityConfig, configure_routes, serve_frontend,
        serve_frontend_root,
    };

    #[test]
    fn security_observability_config_defaults_to_five_seconds() {
        let config = SecurityObservabilityConfig::default();

        assert_eq!(config.timeout_ms, 5_000);
    }

    #[actix_web::test]
    async fn configure_routes_registers_security_routes_before_static_fallback() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .configure(configure_routes),
        )
        .await;
        let request = awtest::TestRequest::get()
            .uri("/api/security/summary?limit=bad")
            .to_request();

        let response = awtest::call_service(&app, request).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn frontend_routes_handle_root_and_tail_paths() {
        let app = awtest::init_service(
            App::new()
                .service(serve_frontend_root)
                .service(serve_frontend),
        )
        .await;

        let root =
            awtest::call_service(&app, awtest::TestRequest::get().uri("/").to_request()).await;
        let tail = awtest::call_service(
            &app,
            awtest::TestRequest::get().uri("/missing").to_request(),
        )
        .await;

        assert!(root.status().is_success() || root.status() == StatusCode::NOT_FOUND);
        assert!(tail.status().is_success() || tail.status() == StatusCode::NOT_FOUND);
    }

    fn test_app_state(timeout_ms: u64) -> web::Data<AppState> {
        web::Data::new(AppState {
            storage_path: PathBuf::from(":memory:"),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            security_observability: SecurityObservabilityConfig { timeout_ms },
        })
    }
}
