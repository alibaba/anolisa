//! Optimization analysis API — bridges captured GenAI sessions to the
//! `agentsight-opt` accuracy/perf/cost analyzers.
//!
//! LLM credentials are configured at runtime from the Dashboard settings page
//! and persisted to `optimization_config.json` next to the databases. Analysis
//! results are persisted per session via `agentsight-opt-store`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use actix_web::{HttpResponse, Responder, get, post, web};
use serde::{Deserialize, Serialize};

use agentsight_opt::{AnalyzePipeline, AtifTrajectory, LlmClient, TrajectoryRecorder};
use agentsight_opt_store::{Dimension, OptimizationStore};

use super::AppState;
use crate::storage::sqlite::GenAISqliteStore;

const CONFIG_FILE_NAME: &str = "optimization_config.json";
const DB_FILE_NAME: &str = "optimization.db";
const TRAJECTORIES_DIR_NAME: &str = "opt-trajectories";

// ─── LLM configuration ───────────────────────────────────────────────────────

/// Runtime LLM configuration for optimization analysis (Dashboard-managed).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OptLlmConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl OptLlmConfig {
    fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let json =
            serde_json::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(path, json)?;
        // Config contains an API key — restrict to owner.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    fn effective_base_url(&self) -> String {
        self.base_url
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
            .unwrap_or_else(|| "https://api.openai.com/v1".into())
    }

    fn effective_api_key(&self) -> Option<String> {
        self.api_key
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
    }

    fn effective_model(&self) -> String {
        self.model
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("OPENAI_MODEL").ok())
            .unwrap_or_else(|| "gpt-4o".into())
    }

    /// Mask the API key for display: first 6 and last 4 chars.
    fn masked_api_key(&self) -> Option<String> {
        self.effective_api_key().map(|k| {
            if k.chars().count() <= 12 {
                "••••••".to_string()
            } else {
                let head: String = k.chars().take(6).collect();
                let mut tail_chars: Vec<char> = k.chars().rev().take(4).collect();
                tail_chars.reverse();
                let tail: String = tail_chars.into_iter().collect();
                format!("{head}••••{tail}")
            }
        })
    }
}

// ─── Shared state ────────────────────────────────────────────────────────────

/// Optimization feature state shared across handlers.
pub struct OptimizeState {
    config_path: PathBuf,
    config: RwLock<OptLlmConfig>,
    store: Option<OptimizationStore>,
}

impl OptimizeState {
    /// Initialize from the storage base directory (where the .db files live).
    pub fn init(base_dir: &Path) -> Arc<Self> {
        let config_path = base_dir.join(CONFIG_FILE_NAME);
        let config = OptLlmConfig::load(&config_path);
        let store = match OptimizationStore::new_with_path(&base_dir.join(DB_FILE_NAME)) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("Failed to open optimization store: {e}");
                None
            }
        };
        Arc::new(Self {
            config_path,
            config: RwLock::new(config),
            store,
        })
    }

    fn snapshot(&self) -> OptLlmConfig {
        self.config.read().map(|c| c.clone()).unwrap_or_default()
    }

    fn build_client(&self) -> Result<LlmClient, HttpResponse> {
        let config = self.snapshot();
        let Some(api_key) = config.effective_api_key() else {
            return Err(HttpResponse::BadRequest().json(serde_json::json!({
                "error": "llm_not_configured",
                "message": "LLM API key not configured. Set it in the optimization settings.",
            })));
        };
        let mut client = LlmClient::with_config(
            config.effective_base_url(),
            api_key,
            config.effective_model(),
        );
        client.set_temperature(0.0);
        Ok(client)
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn optimize_state(data: &AppState) -> Result<&Arc<OptimizeState>, HttpResponse> {
    data.optimize.as_ref().ok_or_else(|| {
        HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "optimization feature unavailable"}))
    })
}

/// Load a session's captured events and build the ATIF trajectory that the
/// analyzers consume. The boundary format is standard ATIF JSON: the export
/// document is serialized and re-parsed into the opt crate's ATIF model.
fn load_trajectory(db_path: &Path, session_id: &str) -> Result<AtifTrajectory, HttpResponse> {
    let store = GenAISqliteStore::new_with_path(db_path).map_err(|e| {
        HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
    })?;
    let events = store.get_events_by_session(session_id).map_err(|e| {
        HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
    })?;
    if events.is_empty() {
        return Err(HttpResponse::NotFound()
            .json(serde_json::json!({"error": "session not found or pruned"})));
    }
    let doc = crate::atif::convert_session_to_atif(session_id, events).map_err(|e| {
        HttpResponse::UnprocessableEntity().json(serde_json::json!({
            "error": "atif_conversion_failed",
            "message": e.to_string(),
        }))
    })?;
    let json = serde_json::to_string(&doc).map_err(|e| {
        HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
    })?;
    AtifTrajectory::from_json(&json).map_err(|e| {
        HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "atif_parse_failed",
            "message": e.to_string(),
        }))
    })
}

/// Serialize an analysis result, persist it, and build the HTTP response.
fn persist_and_respond<T: Serialize>(
    state: &OptimizeState,
    session_id: &str,
    dimension: Dimension,
    result: &T,
) -> HttpResponse {
    let json = match serde_json::to_string(result) {
        Ok(j) => j,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()}));
        }
    };
    if let Some(ref store) = state.store {
        if let Err(e) = store.save_dimension(session_id, dimension, &json) {
            log::warn!("Failed to persist optimization result for {session_id}: {e}");
        }
    }
    HttpResponse::Ok()
        .content_type("application/json")
        .body(json)
}

fn parse_dimension(raw: &str) -> Option<Dimension> {
    match raw {
        "perf" => Some(Dimension::Perf),
        "perf-issues" => Some(Dimension::PerfIssues),
        "cost" => Some(Dimension::Cost),
        "cost-waste" => Some(Dimension::CostWaste),
        "accuracy" => Some(Dimension::Accuracy),
        _ => None,
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// POST /api/optimize/sessions/{session_id}/{dimension}
///
/// Runs one analysis dimension for a captured session. `perf` and `cost` are
/// pure computation; `perf-issues`, `cost-waste` and `accuracy` call the
/// configured LLM and can take 10–60 s.
#[post("/optimize/sessions/{session_id}/{dimension}")]
pub async fn run_optimization(
    data: web::Data<AppState>,
    path: web::Path<(String, String)>,
) -> impl Responder {
    let (session_id, dimension_raw) = path.into_inner();
    let state = match optimize_state(&data) {
        Ok(s) => Arc::clone(s),
        Err(resp) => return resp,
    };
    let Some(dimension) = parse_dimension(&dimension_raw) else {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "unknown dimension",
            "message": "expected one of: perf, perf-issues, cost, cost-waste, accuracy",
        }));
    };

    let trajectory = match load_trajectory(&data.storage_path, &session_id) {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    match dimension {
        Dimension::Perf => match AnalyzePipeline::run_perf(&trajectory) {
            Ok(stats) => persist_and_respond(&state, &session_id, dimension, &stats),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Dimension::Cost => match AnalyzePipeline::run_cost(&trajectory) {
            Ok(stats) => persist_and_respond(&state, &session_id, dimension, &stats),
            Err(e) => HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})),
        },
        Dimension::PerfIssues | Dimension::CostWaste | Dimension::Accuracy => {
            let mut client = match state.build_client() {
                Ok(c) => c,
                Err(resp) => return resp,
            };

            // Attach trajectory recorder to capture LLM calls.
            let recorder = std::sync::Arc::new(TrajectoryRecorder::new(
                client.model().to_string(),
                session_id.clone(),
            ));
            client.set_recorder(std::sync::Arc::clone(&recorder));

            let pipeline = AnalyzePipeline::new(&client);
            let result: Result<String, anyhow::Error> = match dimension {
                Dimension::PerfIssues => pipeline
                    .run_perf_issues(&trajectory)
                    .await
                    .and_then(|r| serde_json::to_string(&r).map_err(|e| anyhow::anyhow!(e))),
                Dimension::CostWaste => pipeline
                    .run_cost_waste(&trajectory)
                    .await
                    .and_then(|r| serde_json::to_string(&r).map_err(|e| anyhow::anyhow!(e))),
                Dimension::Accuracy => pipeline
                    .run_accuracy(&trajectory, None)
                    .await
                    .and_then(|r| serde_json::to_string(&r).map_err(|e| anyhow::anyhow!(e))),
                // Pure-compute dimensions handled in the outer match.
                Dimension::Perf | Dimension::Cost => unreachable!(),
            };

            // Save LLM trajectory as ATIF file (best-effort, non-blocking).
            if !recorder.is_empty() {
                let traj_dir = state
                    .config_path
                    .parent()
                    .unwrap_or(std::path::Path::new("/var/log/sysak/.agentsight"))
                    .join(TRAJECTORIES_DIR_NAME)
                    .join(&dimension_raw);
                if let Err(e) = recorder.save_to_dir(&traj_dir) {
                    log::warn!("Failed to save opt LLM trajectory: {e}");
                }
            }

            match result {
                Ok(json) => {
                    if let Some(ref store) = state.store {
                        if let Err(e) = store.save_dimension(&session_id, dimension, &json) {
                            log::warn!(
                                "Failed to persist optimization result for {session_id}: {e}"
                            );
                        }
                    }
                    HttpResponse::Ok()
                        .content_type("application/json")
                        .body(json)
                }
                Err(e) => HttpResponse::InternalServerError()
                    .json(serde_json::json!({"error": e.to_string()})),
            }
        }
    }
}

/// GET /api/optimize/sessions/{session_id}/results
///
/// Returns previously persisted analysis results for a session (dimension
/// payloads parsed back into JSON, null when never analyzed).
#[get("/optimize/sessions/{session_id}/results")]
pub async fn get_optimization_results(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> impl Responder {
    let session_id = path.into_inner();
    let state = match optimize_state(&data) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let Some(ref store) = state.store else {
        return HttpResponse::ServiceUnavailable()
            .json(serde_json::json!({"error": "optimization store unavailable"}));
    };

    match store.get(&session_id) {
        Ok(Some(record)) => {
            let parse = |s: &Option<String>| -> serde_json::Value {
                s.as_deref()
                    .and_then(|v| serde_json::from_str(v).ok())
                    .unwrap_or(serde_json::Value::Null)
            };
            HttpResponse::Ok().json(serde_json::json!({
                "session_id": record.session_id,
                "perf": parse(&record.perf),
                "perf_issues": parse(&record.perf_issues),
                "cost": parse(&record.cost),
                "cost_waste": parse(&record.cost_waste),
                "accuracy": parse(&record.accuracy),
                "created_at_ns": record.created_at_ns,
                "updated_at_ns": record.updated_at_ns,
            }))
        }
        Ok(None) => HttpResponse::Ok().json(serde_json::json!({
            "session_id": session_id,
            "perf": null, "perf_issues": null, "cost": null,
            "cost_waste": null, "accuracy": null,
        })),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/optimize/config — current LLM config with masked API key.
#[get("/optimize/config")]
pub async fn get_optimize_config(data: web::Data<AppState>) -> impl Responder {
    let state = match optimize_state(&data) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let config = state.snapshot();
    HttpResponse::Ok().json(serde_json::json!({
        "api_key": config.masked_api_key(),
        "base_url": config.effective_base_url(),
        "model": config.effective_model(),
        "configured": config.effective_api_key().is_some(),
    }))
}

/// Body for POST /api/optimize/config. Omitted fields keep their prior value.
#[derive(Debug, Deserialize)]
pub struct UpdateOptConfig {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
}

/// POST /api/optimize/config — update LLM config (persisted to disk).
#[post("/optimize/config")]
pub async fn update_optimize_config(
    data: web::Data<AppState>,
    body: web::Json<UpdateOptConfig>,
) -> impl Responder {
    let state = match optimize_state(&data) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let updated = {
        let mut config = match state.config.write() {
            Ok(c) => c,
            Err(_) => {
                return HttpResponse::InternalServerError()
                    .json(serde_json::json!({"error": "config lock poisoned"}));
            }
        };
        if let Some(ref key) = body.api_key {
            if !key.is_empty() && !key.contains('•') {
                config.api_key = Some(key.clone());
            }
        }
        if let Some(ref url) = body.base_url {
            if !url.is_empty() {
                config.base_url = Some(url.clone());
            }
        }
        if let Some(ref model) = body.model {
            if !model.is_empty() {
                config.model = Some(model.clone());
            }
        }
        config.clone()
    };

    if let Err(e) = updated.save(&state.config_path) {
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "error": format!("failed to persist config: {e}"),
        }));
    }

    HttpResponse::Ok().json(serde_json::json!({
        "api_key": updated.masked_api_key(),
        "base_url": updated.effective_base_url(),
        "model": updated.effective_model(),
        "configured": updated.effective_api_key().is_some(),
    }))
}
