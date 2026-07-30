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
use agentsight_trajectory_collector::{TrajectoryRecord, TrajectoryStore};

use uuid::Uuid;

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
///
/// When the session has no eBPF-captured events, falls back to the
/// log-collected trajectory store (trajectories.db), whose rows already hold
/// ready-made ATIF v1.7 JSON.
fn load_trajectory(
    db_path: &Path,
    trajectory_store: Option<Arc<TrajectoryStore>>,
    session_id: &str,
) -> Result<AtifTrajectory, HttpResponse> {
    let store = GenAISqliteStore::new_with_path(db_path).map_err(|e| {
        HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
    })?;
    let events = store.get_events_by_session(session_id).map_err(|e| {
        HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
    })?;
    if events.is_empty() {
        return load_collected_trajectory(trajectory_store, session_id);
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

/// Fallback for sessions absent from the eBPF capture: load the ATIF JSON
/// persisted by the trajectory collector (log-collected Qoder/QoderWork
/// sessions).
fn load_collected_trajectory(
    trajectory_store: Option<Arc<TrajectoryStore>>,
    session_id: &str,
) -> Result<AtifTrajectory, HttpResponse> {
    let Some(tstore) = trajectory_store else {
        return Err(HttpResponse::NotFound()
            .json(serde_json::json!({"error": "session not found or pruned"})));
    };
    match tstore.get_atif_json(session_id) {
        Ok(Some(atif_json)) => AtifTrajectory::from_json(&atif_json).map_err(|e| {
            HttpResponse::UnprocessableEntity().json(serde_json::json!({
                "error": "atif_parse_failed",
                "message": e.to_string(),
            }))
        }),
        Ok(None) => Err(HttpResponse::NotFound()
            .json(serde_json::json!({"error": "session not found or pruned"}))),
        Err(e) => {
            Err(HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": e.to_string()})))
        }
    }
}

/// Create or update the per-target optimization run root trajectory
/// (`opt:<target>`). Each dimension analysis appends one dispatch step with a
/// `ToolCall(Agent)` + `subagent_trajectory_ref` pointing at the dimension
/// record (`opt:<target>:subagent:<dim>-<id>`), so the existing subagent
/// injection and session-list folding group all analyses under one row.
fn upsert_opt_run_root(
    store: &TrajectoryStore,
    target_session_id: &str,
    dimension_raw: &str,
    dim_doc: &agentsight_atif::AtifTrajectory,
) -> anyhow::Result<()> {
    use agentsight_atif as schema;

    let root_id = format!("opt:{target_session_id}");
    let mut root: schema::AtifTrajectory = match store.get_atif_json(&root_id)? {
        Some(json) => serde_json::from_str(&json)?,
        None => schema::AtifTrajectory {
            schema_version: schema::ATIF_SCHEMA_VERSION.to_string(),
            agent: schema::Agent {
                name: "agentsight-opt".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                model_name: None,
                tool_definitions: None,
                extra: None,
            },
            steps: Vec::new(),
            session_id: Some(root_id.clone()),
            trajectory_id: None,
            notes: Some(format!("优化分析运行 · 目标会话 {target_session_id}")),
            final_metrics: None,
            continued_trajectory_ref: None,
            subagent_trajectories: None,
            extra: None,
        },
    };
    if root.agent.model_name.is_none() {
        root.agent.model_name = dim_doc.agent.model_name.clone();
    }

    // Append one dispatch step for this dimension run.
    let sub_traj_id = dim_doc.trajectory_id.clone();
    let call_id = format!(
        "dispatch-{}",
        sub_traj_id.as_deref().unwrap_or(dimension_raw)
    );
    let ts = dim_doc
        .steps
        .last()
        .and_then(|s| s.timestamp.clone())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    root.steps.push(schema::Step {
        step_id: root.steps.len() + 1,
        source: schema::StepSource::Agent,
        message: String::new(),
        timestamp: Some(ts),
        model_name: dim_doc.agent.model_name.clone(),
        reasoning_effort: None,
        reasoning_content: None,
        tool_calls: Some(vec![schema::ToolCall {
            tool_call_id: call_id.clone(),
            function_name: "Agent".to_string(),
            arguments: serde_json::json!({
                "subagent_type": dimension_raw,
                "description": format!("{dimension_raw} 维度优化分析"),
            }),
            extra: None,
        }]),
        observation: Some(schema::Observation {
            results: vec![schema::ObservationResult {
                source_call_id: Some(call_id),
                content: None,
                subagent_trajectory_ref: Some(vec![schema::SubagentTrajectoryRef {
                    trajectory_id: sub_traj_id,
                    trajectory_path: None,
                    session_id: dim_doc.session_id.clone(),
                    extra: None,
                }]),
                extra: None,
            }],
        }),
        metrics: None,
        extra: None,
        llm_call_count: None,
        is_copied_context: None,
    });

    // Accumulate run totals across dimension analyses.
    let (dim_prompt, dim_completion) = dim_doc
        .final_metrics
        .as_ref()
        .map(|m| {
            (
                m.total_prompt_tokens.unwrap_or(0),
                m.total_completion_tokens.unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    let prev = root.final_metrics.take();
    let prev_prompt = prev
        .as_ref()
        .and_then(|m| m.total_prompt_tokens)
        .unwrap_or(0);
    let prev_completion = prev
        .as_ref()
        .and_then(|m| m.total_completion_tokens)
        .unwrap_or(0);
    root.final_metrics = Some(schema::FinalMetrics {
        total_prompt_tokens: Some(prev_prompt + dim_prompt),
        total_completion_tokens: Some(prev_completion + dim_completion),
        total_cached_tokens: None,
        total_cost_usd: None,
        total_steps: Some(root.steps.len()),
        extra: None,
    });

    let atif_json = serde_json::to_string(&root)?;
    let record = TrajectoryRecord {
        session_id: root_id,
        schema_version: root.schema_version.clone(),
        agent_name: "agentsight-opt".to_string(),
        model_name: root.agent.model_name.clone(),
        num_steps: root.steps.len() as i64,
        total_prompt_tokens: root
            .final_metrics
            .as_ref()
            .and_then(|m| m.total_prompt_tokens)
            .map(|v| v as i64),
        total_completion_tokens: root
            .final_metrics
            .as_ref()
            .and_then(|m| m.total_completion_tokens)
            .map(|v| v as i64),
        start_time: root.steps.first().and_then(|s| s.timestamp.clone()),
        end_time: root.steps.last().and_then(|s| s.timestamp.clone()),
        first_user_message: Some(format!("优化分析运行 · 目标会话 {target_session_id}")),
        last_user_message: Some(format!("最近维度: {dimension_raw}")),
        atif_json,
        project: target_session_id.to_string(),
        source: "agentsight-opt".to_string(),
        is_subagent: false,
        file_path: String::new(),
        file_size: 0,
        file_mtime_ns: 0,
    };
    store.upsert_trajectory(&record)?;
    Ok(())
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
        "summary" => Some(Dimension::Summary),
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

    let trajectory = match load_trajectory(&data.storage_path, data.trajectory_store(), &session_id)
    {
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
        Dimension::PerfIssues | Dimension::CostWaste | Dimension::Accuracy | Dimension::Summary => {
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
                Dimension::Summary => pipeline
                    .run_summary(&trajectory)
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

                // Persist to trajectories.db for Dashboard query. The record
                // becomes a subagent of the per-target run root (`opt:<target>`)
                // so all dimension analyses of one session group under one row.
                if let Some(traj_store) = data.trajectory_store() {
                    let mut doc = recorder.to_atif();
                    let run_suffix = format!(
                        "{dimension_raw}-{}",
                        &Uuid::new_v4().simple().to_string()[..8]
                    );
                    let record_session_id = format!("opt:{session_id}:subagent:{run_suffix}");
                    doc.session_id = Some(record_session_id.clone());
                    doc.trajectory_id = Some(run_suffix.clone());
                    let atif_json = serde_json::to_string(&doc).unwrap_or_default();
                    let (first_user_message, last_user_message) =
                        agentsight_trajectory_collector::store::extract_user_message_previews(
                            &atif_json,
                        );
                    let record = TrajectoryRecord {
                        session_id: record_session_id,
                        schema_version: doc.schema_version.clone(),
                        agent_name: "agentsight-opt".to_string(),
                        model_name: doc.agent.model_name.clone(),
                        num_steps: doc.steps.len() as i64,
                        total_prompt_tokens: doc
                            .final_metrics
                            .as_ref()
                            .and_then(|m| m.total_prompt_tokens)
                            .map(|v| v as i64),
                        total_completion_tokens: doc
                            .final_metrics
                            .as_ref()
                            .and_then(|m| m.total_completion_tokens)
                            .map(|v| v as i64),
                        start_time: doc.steps.first().and_then(|s| s.timestamp.clone()),
                        end_time: doc.steps.last().and_then(|s| s.timestamp.clone()),
                        first_user_message,
                        last_user_message,
                        atif_json,
                        project: session_id.clone(),
                        source: "agentsight-opt".to_string(),
                        is_subagent: true,
                        file_path: String::new(),
                        file_size: 0,
                        file_mtime_ns: 0,
                    };
                    if let Err(e) = traj_store.upsert_trajectory(&record) {
                        log::warn!("Failed to persist opt trajectory to SQLite: {e}");
                    } else if let Err(e) =
                        upsert_opt_run_root(traj_store.as_ref(), &session_id, &dimension_raw, &doc)
                    {
                        log::warn!("Failed to update opt run root trajectory: {e}");
                    }
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
                "summary": parse(&record.summary),
                "created_at_ns": record.created_at_ns,
                "updated_at_ns": record.updated_at_ns,
            }))
        }
        Ok(None) => HttpResponse::Ok().json(serde_json::json!({
            "session_id": session_id,
            "perf": null, "perf_issues": null, "cost": null,
            "cost_waste": null, "accuracy": null, "summary": null,
        })),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// Query parameters for the analysis-history listing.
#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    pub limit: Option<usize>,
}

/// Current UNIX time in nanoseconds; 0 if the clock is before the epoch.
fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Max rows a single history listing may return.
const HISTORY_MAX_LIMIT: usize = 200;
/// Default listing window when the caller passes no range: 30 days.
const HISTORY_DEFAULT_WINDOW_NS: i64 = 30 * 86_400_000_000_000;

/// GET /api/optimize/results?start_ns=&end_ns=&limit=
///
/// Lists previously analyzed sessions, newest first. Only per-dimension
/// presence flags are returned — never the payloads, which are megabytes of
/// JSON and are fetched per session via `/optimize/sessions/{id}/results`.
#[get("/optimize/results")]
pub async fn list_optimization_history(
    data: web::Data<AppState>,
    query: web::Query<HistoryQuery>,
) -> impl Responder {
    let state = match optimize_state(&data) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    // Store unavailable → empty list rather than an error: the history table is
    // a secondary view and should degrade like the trajectory endpoints do.
    let Some(ref store) = state.store else {
        return HttpResponse::Ok().json(Vec::<serde_json::Value>::new());
    };

    let end_ns = query.end_ns.unwrap_or_else(now_ns);
    let start_ns = query
        .start_ns
        .unwrap_or_else(|| end_ns.saturating_sub(HISTORY_DEFAULT_WINDOW_NS));
    let limit = query.limit.unwrap_or(100).clamp(1, HISTORY_MAX_LIMIT);

    match store.list(start_ns, end_ns, limit) {
        Ok(records) => {
            let items: Vec<serde_json::Value> = records
                .iter()
                .map(|r| {
                    let mut dimensions = Vec::new();
                    for (name, value) in [
                        ("perf", &r.perf),
                        ("perf_issues", &r.perf_issues),
                        ("cost", &r.cost),
                        ("cost_waste", &r.cost_waste),
                        ("accuracy", &r.accuracy),
                        ("summary", &r.summary),
                    ] {
                        if value.is_some() {
                            dimensions.push(name);
                        }
                    }
                    serde_json::json!({
                        "session_id": r.session_id,
                        "dimensions": dimensions,
                        "created_at_ns": r.created_at_ns,
                        "updated_at_ns": r.updated_at_ns,
                    })
                })
                .collect();
            HttpResponse::Ok().json(items)
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The eBPF export feeds the optimizer through JSON, so the shared-schema
    /// document must survive the analyzer's parser with tokens and per-step
    /// timing intact — the same contract the collected v1.7 trajectories rely on.
    #[test]
    fn converted_export_parses_into_analyzer_trajectory() {
        let events = crate::atif::converter::tests::two_call_chain();
        let doc = crate::atif::convert_session_to_atif("session-1", events).unwrap();
        let json = serde_json::to_string(&doc).unwrap();

        let traj = AtifTrajectory::from_json(&json).expect("analyzer must accept the export");
        assert_eq!(traj.session_id, "session-1");
        assert_eq!(traj.model_name(), "claude-opus-5");

        let agent_steps: Vec<_> = traj.steps.iter().filter(|s| s.is_agent()).collect();
        assert_eq!(agent_steps.len(), 2);
        // start_ts comes from extra.start_timestamp, end_ts from timestamp;
        // losing either one silently zeroes out the perf dimension.
        assert!(agent_steps[0].start_ts().is_some());
        assert!(agent_steps[0].end_ts() > agent_steps[0].start_ts());
        assert_eq!(agent_steps[0].results().len(), 1);
        assert_eq!(
            traj.final_metrics
                .as_ref()
                .and_then(|m| m.total_prompt_tokens),
            Some(200)
        );
    }

    #[test]
    fn config_prefers_explicit_values_and_masks_key() {
        let config = OptLlmConfig {
            api_key: Some("sk-1234567890abcd".into()),
            base_url: Some("http://localhost/v1".into()),
            model: Some("test-model".into()),
        };

        assert_eq!(
            config.effective_api_key().as_deref(),
            Some("sk-1234567890abcd")
        );
        assert_eq!(config.effective_base_url(), "http://localhost/v1");
        assert_eq!(config.effective_model(), "test-model");
        assert_eq!(config.masked_api_key().as_deref(), Some("sk-123••••abcd"));
    }

    #[test]
    fn config_ignores_empty_values_and_masks_short_key() {
        let config = OptLlmConfig {
            api_key: Some("short".into()),
            base_url: Some(String::new()),
            model: Some(String::new()),
        };

        assert_eq!(config.effective_api_key().as_deref(), Some("short"));
        assert_eq!(config.masked_api_key().as_deref(), Some("••••••"));
    }

    #[test]
    fn parses_known_optimization_dimensions() {
        assert_eq!(parse_dimension("perf"), Some(Dimension::Perf));
        assert_eq!(parse_dimension("perf-issues"), Some(Dimension::PerfIssues));
        assert_eq!(parse_dimension("cost"), Some(Dimension::Cost));
        assert_eq!(parse_dimension("cost-waste"), Some(Dimension::CostWaste));
        assert_eq!(parse_dimension("accuracy"), Some(Dimension::Accuracy));
        assert_eq!(parse_dimension("unknown"), None);
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("opt-traj-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn collected_record(session_id: &str, atif_json: &str) -> TrajectoryRecord {
        TrajectoryRecord {
            session_id: session_id.to_string(),
            schema_version: "ATIF-v1.7".to_string(),
            agent_name: "qoder".to_string(),
            model_name: None,
            num_steps: 0,
            total_prompt_tokens: None,
            total_completion_tokens: None,
            start_time: None,
            end_time: None,
            first_user_message: None,
            last_user_message: None,
            atif_json: atif_json.to_string(),
            project: "proj".to_string(),
            source: "qoder".to_string(),
            is_subagent: false,
            file_path: String::new(),
            file_size: 0,
            file_mtime_ns: 0,
        }
    }

    #[test]
    fn load_trajectory_falls_back_to_collected_store() {
        let dir = tmp_dir("fallback");
        let db_path = dir.join("genai.db");
        let tstore = TrajectoryStore::new_with_path(&dir.join("trajectories.db")).unwrap();
        let atif = r#"{"schema_version":"ATIF-v1.7","session_id":"log-1","steps":[]}"#;
        tstore
            .upsert_trajectory(&collected_record("log-1", atif))
            .unwrap();

        let trajectory = load_trajectory(&db_path, Some(Arc::new(tstore)), "log-1").unwrap();
        assert_eq!(trajectory.session_id, "log-1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_trajectory_returns_not_found_when_both_sources_miss() {
        let dir = tmp_dir("miss");
        let db_path = dir.join("genai.db");
        let tstore = TrajectoryStore::new_with_path(&dir.join("trajectories.db")).unwrap();

        // Store present but session absent → 404.
        let resp = load_trajectory(&db_path, Some(Arc::new(tstore)), "nope").unwrap_err();
        assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);

        // No store at all → 404 as well.
        let resp = load_trajectory(&db_path, None, "nope").unwrap_err();
        assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_trajectory_rejects_corrupt_collected_atif() {
        let dir = tmp_dir("corrupt");
        let db_path = dir.join("genai.db");
        let tstore = TrajectoryStore::new_with_path(&dir.join("trajectories.db")).unwrap();
        tstore
            .upsert_trajectory(&collected_record("bad-1", "not json"))
            .unwrap();

        let resp = load_trajectory(&db_path, Some(Arc::new(tstore)), "bad-1").unwrap_err();
        assert_eq!(
            resp.status(),
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn dimension_doc(
        traj_id: &str,
        session_id: &str,
        prompt: u64,
    ) -> agentsight_atif::AtifTrajectory {
        agentsight_atif::AtifTrajectory {
            schema_version: agentsight_atif::ATIF_SCHEMA_VERSION.to_string(),
            agent: agentsight_atif::Agent {
                name: "agentsight-opt".into(),
                version: "test".into(),
                model_name: Some("gpt-4o".into()),
                tool_definitions: None,
                extra: None,
            },
            steps: vec![],
            session_id: Some(session_id.to_string()),
            trajectory_id: Some(traj_id.to_string()),
            notes: None,
            final_metrics: Some(agentsight_atif::FinalMetrics {
                total_prompt_tokens: Some(prompt),
                total_completion_tokens: Some(10),
                total_cached_tokens: None,
                total_cost_usd: None,
                total_steps: None,
                extra: None,
            }),
            continued_trajectory_ref: None,
            subagent_trajectories: None,
            extra: None,
        }
    }

    #[test]
    fn opt_run_root_accumulates_dimension_dispatches() {
        let dir = tmp_dir("runroot");
        let tstore = TrajectoryStore::new_with_path(&dir.join("trajectories.db")).unwrap();

        upsert_opt_run_root(
            &tstore,
            "target-1",
            "perf-issues",
            &dimension_doc(
                "perf-issues-abc",
                "opt:target-1:subagent:perf-issues-abc",
                100,
            ),
        )
        .unwrap();
        upsert_opt_run_root(
            &tstore,
            "target-1",
            "cost-waste",
            &dimension_doc(
                "cost-waste-def",
                "opt:target-1:subagent:cost-waste-def",
                200,
            ),
        )
        .unwrap();

        let json = tstore.get_atif_json("opt:target-1").unwrap().unwrap();
        let root: agentsight_atif::AtifTrajectory = serde_json::from_str(&json).unwrap();

        // One dispatch step per dimension run; token totals accumulate.
        assert_eq!(root.steps.len(), 2);
        assert_eq!(root.session_id.as_deref(), Some("opt:target-1"));
        let metrics = root.final_metrics.unwrap();
        assert_eq!(metrics.total_prompt_tokens, Some(300));
        assert_eq!(metrics.total_completion_tokens, Some(20));

        // Dispatch step carries ToolCall(Agent) + subagent ref.
        let tc = &root.steps[1].tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.function_name, "Agent");
        assert_eq!(tc.arguments["subagent_type"], "cost-waste");
        let refs = root.steps[1].observation.as_ref().unwrap().results[0]
            .subagent_trajectory_ref
            .as_ref()
            .unwrap();
        assert_eq!(refs[0].trajectory_id.as_deref(), Some("cost-waste-def"));
        assert_eq!(
            refs[0].session_id.as_deref(),
            Some("opt:target-1:subagent:cost-waste-def")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
