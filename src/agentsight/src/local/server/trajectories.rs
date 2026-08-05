//! Trajectory collection endpoints for macOS local server.
//!
//! Mirrors the upstream `/api/trajectories` routes but backed by
//! `agentsight_trajectory_collector::TrajectoryStore` instead of the full
//! Linux-only `AppState`. On macOS the collector scans Qoder/QoderWork session
//! directories and stores ATIF v1.7 documents in `trajectories.db`.

use std::sync::Arc;

use actix_web::{HttpResponse, Responder, get, web};
use agentsight_trajectory_collector::TrajectoryStore;
use serde::Deserialize;

use super::LocalState;

/// Default and hard-cap for the trajectory list `limit` parameter.
const TRAJECTORY_DEFAULT_LIMIT: i64 = 200;
const TRAJECTORY_MAX_LIMIT: i64 = 1000;

#[derive(Deserialize)]
pub struct TrajectoryQuery {
    pub project: Option<String>,
    pub source: Option<String>,
    pub agent_name: Option<String>,
    pub limit: Option<i64>,
}

/// GET /api/trajectories
#[get("/api/trajectories")]
pub async fn list_trajectories(
    state: web::Data<LocalState>,
    query: web::Query<TrajectoryQuery>,
) -> impl Responder {
    let Some(tstore) = state.trajectory_store() else {
        return HttpResponse::Ok().json(Vec::<serde_json::Value>::new());
    };
    let limit = match query.limit {
        Some(v) if v > 0 => v.min(TRAJECTORY_MAX_LIMIT),
        _ => TRAJECTORY_DEFAULT_LIMIT,
    };
    match tstore.list_summaries(
        query.project.as_deref(),
        query.source.as_deref(),
        query.agent_name.as_deref(),
        limit,
    ) {
        Ok(rows) => HttpResponse::Ok().json(rows),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/trajectories/filters
#[get("/api/trajectories/filters")]
pub async fn trajectory_filters(state: web::Data<LocalState>) -> impl Responder {
    let Some(tstore) = state.trajectory_store() else {
        return HttpResponse::Ok().json(serde_json::json!({
            "projects": [], "sources": [], "agent_names": []
        }));
    };
    match tstore.list_filters() {
        Ok(filters) => HttpResponse::Ok().json(filters),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

/// GET /api/trajectories/{session_id}
#[get("/api/trajectories/{session_id}")]
pub async fn get_trajectory_detail(
    state: web::Data<LocalState>,
    path: web::Path<String>,
) -> impl Responder {
    let Some(tstore) = state.trajectory_store() else {
        return HttpResponse::NotFound().json(
            serde_json::json!({"error": "not_found", "message": "Trajectory store not available"}),
        );
    };
    let session_id = path.into_inner();

    match tstore.get_atif_json(&session_id) {
        Ok(Some(json_str)) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&json_str).unwrap_or(serde_json::json!({"raw": json_str}));
            let mut doc = parsed;

            // Embed subagent trajectories if any
            if let Ok(subagents) = tstore.get_subagent_atif_jsons(&session_id)
                && !subagents.is_empty()
            {
                let mut sub_docs = Vec::new();
                for sa_json in subagents {
                    if let Ok(sa) = serde_json::from_str::<serde_json::Value>(&sa_json) {
                        sub_docs.push(sa);
                    }
                }
                if !sub_docs.is_empty() {
                    doc["subagent_trajectories"] = serde_json::Value::Array(sub_docs);
                }
            }

            HttpResponse::Ok().json(doc)
        }
        Ok(None) => HttpResponse::NotFound()
            .json(serde_json::json!({"error": "not_found", "message": "Trajectory not found"})),
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({"error": e.to_string()}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, test};
    use std::path::PathBuf;
    use std::sync::RwLock;

    fn make_state(store: Option<Arc<TrajectoryStore>>) -> web::Data<LocalState> {
        web::Data::new(LocalState {
            trajectory_store: Arc::new(RwLock::new(store)),
            db_path: PathBuf::from("/nonexistent/trajectories.db"),
        })
    }

    fn make_state_with_store(
        store: Arc<TrajectoryStore>,
        db_path: PathBuf,
    ) -> web::Data<LocalState> {
        web::Data::new(LocalState {
            trajectory_store: Arc::new(RwLock::new(Some(store))),
            db_path,
        })
    }

    #[actix_web::test]
    async fn test_list_trajectories_no_store() {
        let state = make_state(None);
        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(list_trajectories)
                .service(trajectory_filters)
                .service(get_trajectory_detail),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/api/trajectories")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let req = test::TestRequest::get()
            .uri("/api/trajectories/filters")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let req = test::TestRequest::get()
            .uri("/api/trajectories/nonexistent")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);
    }

    #[actix_web::test]
    async fn test_list_trajectories_with_store() {
        let tmp = std::env::temp_dir().join("agentsight_traj_handler_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let db_path = tmp.join("trajectories.db");
        let store = Arc::new(TrajectoryStore::new_with_path(&db_path).unwrap());
        let state = make_state_with_store(store, db_path);
        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(list_trajectories)
                .service(trajectory_filters),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/api/trajectories")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let req = test::TestRequest::get()
            .uri("/api/trajectories/filters")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[actix_web::test]
    async fn test_get_trajectory_detail_not_found() {
        let tmp = std::env::temp_dir().join("agentsight_traj_detail_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let db_path = tmp.join("trajectories.db");
        let store = Arc::new(TrajectoryStore::new_with_path(&db_path).unwrap());
        let state = make_state_with_store(store, db_path);
        let app =
            test::init_service(App::new().app_data(state).service(get_trajectory_detail)).await;

        let req = test::TestRequest::get()
            .uri("/api/trajectories/missing-session")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[actix_web::test]
    async fn test_get_trajectory_detail_found() {
        let tmp = std::env::temp_dir().join("agentsight_traj_found_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let db_path = tmp.join("trajectories.db");
        let store = Arc::new(TrajectoryStore::new_with_path(&db_path).unwrap());
        let atif = r#"{"schema_version":"ATIF-v1.7","session_id":"sess-found","steps":[]}"#;
        let record = agentsight_trajectory_collector::TrajectoryRecord {
            session_id: "sess-found".to_string(),
            schema_version: "ATIF-v1.7".to_string(),
            agent_name: "test".to_string(),
            model_name: None,
            num_steps: 0,
            total_prompt_tokens: None,
            total_completion_tokens: None,
            start_time: None,
            end_time: None,
            first_user_message: None,
            last_user_message: None,
            atif_json: atif.to_string(),
            project: "test".to_string(),
            source: "test".to_string(),
            is_subagent: false,
            file_path: String::new(),
            file_size: 0,
            file_mtime_ns: 0,
        };
        store.upsert_trajectory(&record).unwrap();

        let state = make_state_with_store(store, db_path);
        let app = test::init_service(
            App::new()
                .app_data(state)
                .service(get_trajectory_detail)
                .service(list_trajectories)
                .service(trajectory_filters),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/api/trajectories/sess-found")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let req = test::TestRequest::get()
            .uri("/api/trajectories?limit=10")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let req = test::TestRequest::get()
            .uri("/api/trajectories?limit=0")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
