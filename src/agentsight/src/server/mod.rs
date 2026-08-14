//! API server module
//!
//! Provides a lightweight HTTP API server using actix-web for querying
//! AgentSight storage data, and optionally serves the embedded frontend.

pub mod auth;
mod causal;
mod containment;
mod enforcement;
mod handlers;
pub mod optimize;
mod secret;
mod system_audit;
mod token_savings;

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use actix_cors::Cors;
use actix_web::{App, HttpRequest, HttpResponse, HttpServer, Responder, get, web};
use agentsight_audit::AuditService;
use include_dir::{Dir, include_dir};

use crate::config::ServerAuthConfig;
use crate::enforcement::{EnforcementClient, EnforcementCoordinator, EnforcementStore};
use crate::grader::EvaluationStore;
use crate::health::{HealthChecker, HealthStore};
use crate::security::{ContainmentCoordinator, SecurityCoordinator};
use crate::storage::sqlite::InterruptionStore;
use agentsight_trajectory_collector::TrajectoryStore;

use self::auth::{AuthMiddleware, DashboardAuth};

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
    /// Grader evaluation store
    pub evaluation_store: Arc<EvaluationStore>,
    /// Desired enforcement state and privileged-service client.
    pub enforcement: Option<Arc<EnforcementCoordinator>>,
    /// Case-level durable containment orchestration.
    pub containment: Option<Arc<ContainmentCoordinator>>,
    /// AgentSight-owned system-audit application service.
    pub audit_service: Arc<AuditService>,
    /// agent-sec security observability integration configuration
    pub security_observability: SecurityObservabilityConfig,
    /// Dashboard authentication state
    pub auth: Arc<DashboardAuth>,
    /// Optimization analysis state (LLM config + result store)
    pub optimize: Option<Arc<optimize::OptimizeState>>,
    /// Read-only store over collected trajectories (`trajectories.db`)
    ///
    /// Wrapped in `RwLock` so `trajectory_store()` can memoize lazy opens
    /// (write once when the DB first appears, read on every subsequent call).
    pub trajectory_store: Arc<RwLock<Option<Arc<TrajectoryStore>>>>,
}

impl AppState {
    /// Return a trajectory store if collection has produced `trajectories.db`.
    ///
    /// `serve` is commonly started before `trace`; in that case the DB does not
    /// exist at server startup. Re-checking on demand lets the UI show newly
    /// collected log sessions without requiring a server restart.
    pub fn trajectory_store(&self) -> Option<Arc<TrajectoryStore>> {
        // Fast path: already opened
        {
            let guard = self
                .trajectory_store
                .read()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(store) = guard.as_ref() {
                return Some(Arc::clone(store));
            }
        }

        // Check if DB exists
        let db_path = crate::storage::sqlite::sibling_db_path("trajectories.db");
        if !db_path.exists() {
            return None;
        }

        // Try to open; upgrade to write lock to memoize
        match TrajectoryStore::new_with_path(&db_path) {
            Ok(store) => {
                let mut guard = self
                    .trajectory_store
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                // Double-check: another thread may have opened it while we waited
                if let Some(existing) = guard.as_ref() {
                    Some(Arc::clone(existing))
                } else {
                    let arc = Arc::new(store);
                    *guard = Some(Arc::clone(&arc));
                    log::info!("Trajectory store opened lazily at {db_path:?}");
                    Some(arc)
                }
            }
            Err(e) => {
                log::warn!("Failed to lazily open trajectory store: {e}");
                None
            }
        }
    }
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
        // Top-level health & metrics (not under /api)
        .service(handlers::health)
        .service(handlers::metrics)
        // Auth endpoints (exempt from middleware)
        .service(
            web::scope("/api/auth")
                .service(handlers::auth_status)
                .service(handlers::auth_verify)
                .service(web::resource("/login").route(web::post().to(handlers::auth_login))),
        )
        // All API routes under /api scope
        .service(
            web::scope("/api")
                .service(handlers::list_sessions)
                .service(handlers::list_traces_by_session)
                .service(handlers::get_trace_detail)
                .service(handlers::get_conversation_events)
                .service(handlers::evaluate_grader)
                .service(handlers::latest_grader)
                .service(handlers::list_agent_names)
                .service(handlers::get_timeseries)
                .service(handlers::get_latency_metrics)
                .service(handlers::export_atif_trace)
                .service(handlers::export_atif_session)
                .service(handlers::export_atif_conversation)
                .service(handlers::get_agent_health)
                .service(
                    web::resource("/agent-health/{pid}")
                        .route(web::delete().to(handlers::delete_agent_health)),
                )
                .service(handlers::restart_agent_health)
                // Interruption API routes
                .service(handlers::list_interruptions)
                .service(handlers::interruption_count)
                .service(handlers::interruption_stats)
                .service(handlers::interruption_session_counts)
                .service(handlers::interruption_conversation_counts)
                .service(handlers::list_session_interruptions)
                .service(handlers::list_conversation_interruptions)
                .service(
                    web::resource("/interruptions/{interruption_id}/resolve")
                        .route(web::post().to(handlers::resolve_interruption)),
                )
                .service(handlers::get_interruption)
                .service(token_savings::get_token_savings)
                .service(token_savings::get_session_savings)
                // AgentSight local security and system-audit API routes
                .service(handlers::security_status)
                .service(handlers::security_summary)
                .service(handlers::security_events_count_by)
                .service(handlers::security_events_list)
                .service(handlers::security_event_detail)
                .service(handlers::security_observability_sessions)
                .service(handlers::security_observability_runs)
                .service(handlers::security_observability_timeline)
                .service(system_audit::summary)
                .service(system_audit::sessions)
                .service(system_audit::events)
                .service(system_audit::cases)
                .service(system_audit::case_detail)
                .service(system_audit::review_case)
                .service(containment::containment_plan)
                .service(containment::contain_case)
                // AgentSight-owned enforcement API routes
                .service(enforcement::health)
                .service(enforcement::apply_binding)
                .service(enforcement::apply_file_binding)
                .service(enforcement::apply_credential_binding)
                .service(enforcement::list_bindings)
                .service(enforcement::detach_binding)
                .service(enforcement::list_violations)
                // Skill Metrics API routes
                .service(handlers::skill_metrics_all)
                .service(handlers::skill_metrics_downloads)
                .service(handlers::skill_metrics_loads)
                .service(handlers::skill_metrics_usage_ratio)
                .service(handlers::skill_metrics_distribution)
                .service(handlers::skill_metrics_hotness)
                // Optimization analysis API routes
                .service(optimize::run_optimization)
                .service(optimize::get_optimization_results)
                .service(optimize::list_optimization_history)
                .service(optimize::get_optimize_config)
                .service(optimize::update_optimize_config)
                // Causal attribution API routes
                .service(causal::run_causal_attribution)
                // Trajectory collection API routes (filters before the dynamic segment)
                .service(handlers::list_trajectories)
                .service(handlers::trajectory_filters)
                .service(handlers::get_trajectory_detail)
                .default_service(web::route().to(api_not_found)),
        )
        // Health scope with not-found fallback
        .service(web::scope("/health").default_service(web::route().to(api_not_found)))
        // Frontend static files (catch-all, must be last)
        .service(serve_frontend_root)
        .service(serve_frontend);
}

async fn api_not_found() -> impl Responder {
    HttpResponse::NotFound()
        .json(serde_json::json!({"error": "not_found", "message": "No matching API endpoint"}))
}

/// Builds the JSON body extractor config registered on the server `App`.
///
/// Actix's default rejection is a `text/plain` raw serde message; API
/// consumers expect the structured `{"error":{...}}` envelope, so every
/// `web::Json` failure is rewritten here. Kept as a shared constructor so
/// tests exercise the exact handler used in production.
fn json_extractor_config() -> web::JsonConfig {
    // Generic prefix keeps Rust type names out of the leading text while
    // preserving the serde detail (field/variant names are public API states).
    web::JsonConfig::default()
        .error_handler(|error, _req| extractor_error(format!("invalid request body: {error}")))
}

/// Builds the typed path extractor config registered on the server `App`.
///
/// Also turns typed-path failures (e.g. a non-UUID `{binding_id}`) into 400
/// instead of actix's default 404, matching handlers that parse ids manually.
fn path_extractor_config() -> web::PathConfig {
    web::PathConfig::default()
        .error_handler(|error, _req| extractor_error(format!("invalid path parameter: {error}")))
}

/// Wraps an extractor failure into a 400 response with the shared envelope.
fn extractor_error(message: String) -> actix_web::Error {
    let response = system_audit::error_response(
        actix_web::http::StatusCode::BAD_REQUEST,
        "bad_request",
        &message,
        false,
    );
    actix_web::error::InternalError::from_response(message, response).into()
}

// ─── Server entry point ───────────────────────────────────────────────────────

fn private_state_dir(storage_path: &Path) -> PathBuf {
    storage_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("/var/log/sysak/.agentsight"))
        .join(".agentsight-private")
}

/// Start the API server
///
/// Binds to the given host:port and serves API endpoints + embedded frontend.
/// This function blocks until the server is shut down.
pub async fn run_server(
    host: &str,
    port: u16,
    storage_path: PathBuf,
    auth_config: ServerAuthConfig,
    audit_retention_days: u64,
) -> std::io::Result<()> {
    let security_observability = SecurityObservabilityConfig::default();

    let state_dir = private_state_dir(&storage_path);
    let security_store = Arc::new(
        crate::security::open_private_store(&state_dir)
            .map_err(|error| std::io::Error::other(error.to_string()))?,
    );
    let audit_service = Arc::new(AuditService::new(security_store.audit_store()));

    let evaluation_store = Arc::new(
        EvaluationStore::new_with_path(&storage_path)
            .map_err(|error| std::io::Error::other(error.to_string()))?,
    );

    let enforcement_socket = std::env::var_os("AGENTSIGHT_ENFORCER_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/agentsight/enforcer.sock"));
    let enforcement_client = EnforcementClient::new(enforcement_socket);
    let enforcement = Arc::new(EnforcementCoordinator::new(
        enforcement_client.clone(),
        EnforcementStore::open_private(&state_dir)
            .map_err(|error| std::io::Error::other(error.to_string()))?,
    ));
    let enforcement_ingestion = enforcement
        .start_ingestion()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let security_coordinator =
        SecurityCoordinator::with_service(enforcement_client, Arc::clone(&audit_service));
    let security_ingestion = match security_coordinator.start() {
        Ok(ingestion) => ingestion,
        Err(error) => {
            stop_enforcement_ingestion(&enforcement, enforcement_ingestion);
            return Err(std::io::Error::other(error.to_string()));
        }
    };
    let containment = Arc::new(ContainmentCoordinator::new(
        Arc::clone(&security_store),
        enforcement.clone(),
    ));
    let containment_reconciler = match containment::start_reconciler(&containment) {
        Ok(worker) => worker,
        Err(error) => {
            stop_security_ingestion(&security_coordinator, security_ingestion);
            stop_enforcement_ingestion(&enforcement, enforcement_ingestion);
            return Err(std::io::Error::other(error.to_string()));
        }
    };

    // Initialize dashboard authentication
    let storage_base = storage_path
        .parent()
        .unwrap_or(std::path::Path::new("/var/log/sysak/.agentsight"));
    let dashboard_auth = Arc::new(DashboardAuth::init(&auth_config, storage_base));
    if dashboard_auth.enabled {
        if let Some(token) = dashboard_auth.read_token_from_file() {
            let masked = if token.len() > 8 {
                format!("{}****", &token[..8])
            } else {
                "****".to_string()
            };
            eprintln!(
                "Dashboard auth enabled. Token: {masked}  (use `agentsight dashboard` to view)"
            );
        }
    }

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
        let db_path = crate::storage::sqlite::sibling_db_path("interruption_events.db");
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

    // Initialize read-only trajectory store (collector writes it in `trace` mode;
    // serve only consumes). Path is derived via the shared sibling_db_path helper
    // so reader and writer always resolve the same file. A missing DB simply
    // yields an empty table → empty API results (graceful degradation).
    // Only open when the file already exists to avoid creating an empty DB as a
    // persistent side-effect in serve mode when collection was never enabled.
    let trajectory_store: Option<Arc<TrajectoryStore>> = {
        let db_path = crate::storage::sqlite::sibling_db_path("trajectories.db");
        if !db_path.exists() {
            log::debug!("Trajectory store not found at {db_path:?}; endpoints degrade to empty");
            None
        } else {
            match TrajectoryStore::new_with_path(&db_path) {
                Ok(store) => {
                    log::info!("Trajectory store initialized at {db_path:?}");
                    Some(Arc::new(store))
                }
                Err(e) => {
                    log::warn!("Failed to open trajectory store: {e}");
                    None
                }
            }
        }
    };

    let optimize_state = optimize::OptimizeState::init(storage_base);

    let data = web::Data::new(AppState {
        storage_path,
        start_time: Instant::now(),
        health_store,
        interruption_store,
        evaluation_store,
        enforcement: Some(Arc::clone(&enforcement)),
        containment: Some(Arc::clone(&containment)),
        audit_service,
        security_observability,
        auth: dashboard_auth.clone(),
        optimize: Some(optimize_state),
        trajectory_store: Arc::new(RwLock::new(trajectory_store)),
    });
    let audit_retention =
        start_audit_retention(Arc::clone(&data.audit_service), audit_retention_days);

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

    let server = match HttpServer::new(move || {
        let cors = Cors::default()
            .allow_any_origin()
            .allowed_methods(vec!["GET", "DELETE", "POST", "OPTIONS"])
            .allowed_headers(vec!["Content-Type", "Authorization"])
            .max_age(3600);

        App::new()
            .wrap(cors)
            .wrap(AuthMiddleware::new(dashboard_auth.clone()))
            .app_data(data.clone())
            .app_data(json_extractor_config())
            .app_data(path_extractor_config())
            .configure(configure_routes)
    })
    .bind((host, port))
    {
        Ok(server) => server,
        Err(error) => {
            if let Some(worker) = audit_retention {
                worker.abort();
            }
            containment::stop_reconciler(&containment, containment_reconciler);
            stop_security_ingestion(&security_coordinator, security_ingestion);
            stop_enforcement_ingestion(&enforcement, enforcement_ingestion);
            return Err(error);
        }
    };

    // Guide users toward the `dashboard` subcommand when listening on all interfaces
    if host == "0.0.0.0" || host == "::" {
        eprintln!();
        eprintln!("提示：远程访问需要安全组放行 TCP {port}。");
        eprintln!("运行 'agentsight dashboard' 可自动检测并生成配置命令。");
        eprintln!();
    }

    let server_result = server.run().await;

    if let Some(worker) = audit_retention {
        worker.abort();
    }
    containment::stop_reconciler(&containment, containment_reconciler);
    stop_security_ingestion(&security_coordinator, security_ingestion);
    stop_enforcement_ingestion(&enforcement, enforcement_ingestion);
    server_result
}

const AUDIT_RETENTION_INTERVAL: Duration = Duration::from_secs(60 * 60);

fn start_audit_retention(
    audit_service: Arc<AuditService>,
    retention_days: u64,
) -> Option<actix_web::rt::task::JoinHandle<()>> {
    (retention_days > 0).then(|| {
        actix_web::rt::spawn(async move {
            loop {
                let now_ns = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let retention_ns = retention_days.saturating_mul(24 * 60 * 60 * 1_000_000_000);
                let cutoff_ns = now_ns.saturating_sub(retention_ns);
                match audit_service.purge_before(cutoff_ns) {
                    Ok(deleted) if deleted > 0 => {
                        log::info!("purged {deleted} expired system-audit records");
                    }
                    Ok(_) => {}
                    Err(error) => log::warn!("system-audit retention purge failed: {error}"),
                }
                actix_web::rt::time::sleep(AUDIT_RETENTION_INTERVAL).await;
            }
        })
    })
}

fn stop_security_ingestion(
    coordinator: &SecurityCoordinator,
    ingestion: std::thread::JoinHandle<()>,
) {
    coordinator.stop();
    if ingestion.join().is_err() {
        log::error!("AgentSight security ingestion worker panicked during shutdown");
    }
}

fn stop_enforcement_ingestion(
    coordinator: &EnforcementCoordinator,
    ingestion: std::thread::JoinHandle<()>,
) {
    coordinator.stop_ingestion();
    if ingestion.join().is_err() {
        log::error!("AgentSight enforcement ingestion worker panicked during shutdown");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};
    use std::time::Instant;

    use actix_web::http::StatusCode;
    use actix_web::test as awtest;
    use actix_web::{App, web};

    use crate::grader::EvaluationStore;
    use crate::health::HealthStore;

    use super::auth::DashboardAuth;
    use super::{
        AppState, SecurityObservabilityConfig, TrajectoryStore, configure_routes,
        json_extractor_config, path_extractor_config, private_state_dir, serve_frontend,
        serve_frontend_root,
    };
    use crate::config::ServerAuthConfig;

    #[test]
    fn security_observability_config_defaults_to_five_seconds() {
        let config = SecurityObservabilityConfig::default();

        assert_eq!(config.timeout_ms, 5_000);
    }

    #[test]
    fn private_state_uses_a_dedicated_sibling_directory() {
        assert_eq!(
            private_state_dir(std::path::Path::new("/tmp/agentsight.db")),
            std::path::Path::new("/tmp/.agentsight-private")
        );
    }

    #[test]
    fn trajectory_store_returns_some_when_already_set() {
        let store = TrajectoryStore::new_with_path(std::path::Path::new(":memory:")).unwrap();
        let state = test_app_state_with_trajectory_store(store);

        assert!(state.trajectory_store().is_some());
    }

    #[test]
    fn trajectory_store_returns_none_when_not_set_and_db_missing() {
        let state = test_app_state(0);
        // The default db path (/var/log/sysak/.agentsight/trajectories.db)
        // should not exist in CI, so lazy loading returns None.
        if crate::storage::sqlite::sibling_db_path("trajectories.db").exists() {
            return; // db exists, can't test the "missing" path
        }

        assert!(state.trajectory_store().is_none());
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
    async fn configure_routes_registers_enforcement_routes() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .configure(configure_routes),
        )
        .await;
        let request = awtest::TestRequest::get()
            .uri("/api/enforcement/health")
            .to_request();

        let response = awtest::call_service(&app, request).await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let request = awtest::TestRequest::post()
            .uri("/api/enforcement/file-bindings")
            .insert_header(("content-type", "application/json"))
            .set_payload("{")
            .to_request();

        let response = awtest::call_service(&app, request).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// Asserts the response is a 400 carrying the structured error envelope
    /// produced by the extractor error handlers (issues #2372/#2392).
    async fn assert_bad_request_envelope<B>(
        response: actix_web::dev::ServiceResponse<B>,
        message_fragment: &str,
    ) where
        B: actix_web::body::MessageBody,
    {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(actix_web::http::header::CONTENT_TYPE)
                .expect("content-type header"),
            "application/json"
        );
        let body: serde_json::Value = awtest::read_body_json(response).await;
        assert_eq!(body["error"]["code"], "bad_request");
        assert_eq!(body["error"]["retryable"], false);
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(message_fragment),
            "message {message:?} does not contain {message_fragment:?}"
        );
    }

    #[actix_web::test]
    async fn review_json_extractor_errors_return_error_envelope() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .app_data(json_extractor_config())
                .app_data(path_extractor_config())
                .configure(configure_routes),
        )
        .await;
        let uri = "/api/audit/cases/00000000-0000-0000-0000-000000000000/review";

        let missing_field = awtest::TestRequest::post()
            .uri(uri)
            .insert_header(("content-type", "application/json"))
            .set_payload("{}")
            .to_request();
        let response = awtest::call_service(&app, missing_field).await;
        assert_bad_request_envelope(response, "missing field").await;

        let invalid_variant = awtest::TestRequest::post()
            .uri(uri)
            .insert_header(("content-type", "application/json"))
            .set_payload(r#"{"status":"bogus"}"#)
            .to_request();
        let response = awtest::call_service(&app, invalid_variant).await;
        assert_bad_request_envelope(response, "unknown variant").await;
    }

    #[actix_web::test]
    async fn enforcement_json_extractor_error_returns_error_envelope() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .app_data(json_extractor_config())
                .app_data(path_extractor_config())
                .configure(configure_routes),
        )
        .await;
        let request = awtest::TestRequest::post()
            .uri("/api/enforcement/bindings")
            .insert_header(("content-type", "application/json"))
            .set_payload("{}")
            .to_request();

        let response = awtest::call_service(&app, request).await;

        assert_bad_request_envelope(response, "invalid request body").await;
    }

    #[actix_web::test]
    async fn enforcement_path_extractor_error_returns_error_envelope() {
        let app = awtest::init_service(
            App::new()
                .app_data(test_app_state(0))
                .app_data(json_extractor_config())
                .app_data(path_extractor_config())
                .configure(configure_routes),
        )
        .await;
        // Without the PathConfig handler actix answers 404 here; 400 matches
        // handlers that parse path ids manually (e.g. audit case_detail).
        let request = awtest::TestRequest::delete()
            .uri("/api/enforcement/bindings/not-a-uuid")
            .to_request();

        let response = awtest::call_service(&app, request).await;

        assert_bad_request_envelope(response, "invalid path parameter").await;
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
        let auth_config = ServerAuthConfig { enabled: false };
        let auth = Arc::new(DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        web::Data::new(AppState {
            storage_path: PathBuf::from(":memory:"),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            evaluation_store: Arc::new(
                EvaluationStore::new_with_path(std::path::Path::new(":memory:")).unwrap(),
            ),
            enforcement: None,
            containment: None,
            audit_service: Arc::new(agentsight_audit::AuditService::new(
                crate::security::SecurityStore::open_in_memory()
                    .unwrap()
                    .audit_store(),
            )),
            security_observability: SecurityObservabilityConfig { timeout_ms },
            auth,
            optimize: None,
            trajectory_store: Arc::new(RwLock::new(None)),
        })
    }

    fn test_app_state_with_trajectory_store(store: TrajectoryStore) -> web::Data<AppState> {
        let auth_config = ServerAuthConfig { enabled: false };
        let auth = Arc::new(DashboardAuth::init(
            &auth_config,
            std::path::Path::new("/tmp"),
        ));
        web::Data::new(AppState {
            storage_path: PathBuf::from(":memory:"),
            start_time: Instant::now(),
            health_store: Arc::new(RwLock::new(HealthStore::new())),
            interruption_store: None,
            evaluation_store: Arc::new(
                EvaluationStore::new_with_path(std::path::Path::new(":memory:")).unwrap(),
            ),
            enforcement: None,
            containment: None,
            audit_service: Arc::new(agentsight_audit::AuditService::new(
                crate::security::SecurityStore::open_in_memory()
                    .unwrap()
                    .audit_store(),
            )),
            security_observability: SecurityObservabilityConfig { timeout_ms: 0 },
            auth,
            optimize: None,
            trajectory_store: Arc::new(RwLock::new(Some(Arc::new(store)))),
        })
    }
}
