//! `chat` router — run / stream chat turns, list / inspect / cancel /
//! retry background jobs.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use serde::Deserialize;

use echobot_orchestration::jobs::ConversationJob;
use echobot_orchestration::ConversationCoordinator;

use crate::error::AppError;
use crate::schemas::{
    ChatJobResponse, ChatJobsResponse, ChatJobSummaryModel, ChatJobTraceResponse, ChatRequest,
    ChatResponse,
};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chat", post(run_chat))
        .route("/chat/stream", post(run_chat_stream))
        .route("/chat/jobs", get(list_chat_jobs))
        .route("/chat/jobs/{job_id}", get(get_chat_job))
        .route("/chat/jobs/{job_id}/trace", get(get_chat_job_trace))
        .route("/chat/jobs/{job_id}/cancel", post(cancel_chat_job))
        .route("/chat/jobs/{job_id}/retry", post(retry_chat_job))
}

#[derive(Debug, Deserialize, Default)]
pub struct ListChatJobsQuery {
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

fn coordinator(state: &AppState) -> Result<Arc<ConversationCoordinator>, AppError> {
    state
        .runtime()
        .coordinator
        .clone()
        .ok_or_else(|| AppError::Unavailable("Conversation coordinator is not ready".to_string()))
}

async fn run_chat(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, AppError> {
    let coordinator = coordinator(&state)?;
    let role_name = req.role_name.clone();
    let route_mode_str = req.route_mode.clone();
    let prompt = req.prompt.clone();
    let session_name = req.session_name.clone();
    let result = coordinator
        .handle_user_turn(
            &session_name,
            &prompt,
            None,
            None,
            role_name.as_deref(),
            None,
            None,
            None,
            1,
        )
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    Ok(Json(ChatResponse {
        session_name: result.session.name.clone(),
        response: result.response_text.clone(),
        response_content: serde_json::Value::String(result.response_text.clone()),
        updated_at: result.session.updated_at.clone(),
        steps: 0,
        compressed_summary: result.session.compressed_summary.clone(),
        delegated: result.delegated,
        completed: result.completed,
        job_id: result.job_id.clone(),
        status: result.status.clone(),
        role_name: role_name.unwrap_or_else(|| "default".to_string()),
    }.with_route_mode(route_mode_str)))
}

async fn run_chat_stream(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Response, AppError> {
    let coordinator = coordinator(&state)?;
    let session_name = req.session_name.clone();
    let prompt = req.prompt.clone();
    let role_name = req.role_name.clone();

    // Use a one-shot helper future that drives the turn and emits
    // newline-delimited JSON to the response body.
    let stream = async_stream::stream! {
        let result = coordinator
            .handle_user_turn(
                &session_name,
                &prompt,
                None,
                None,
                role_name.as_deref(),
                None,
                None,
                None,
                1,
            )
            .await;
        match result {
            Ok(turn) => {
                let chunk = serde_json::json!({
                    "type": "chunk",
                    "delta": turn.response_text,
                });
                yield Ok::<_, std::io::Error>(format!("{}\n", chunk));
                let done = serde_json::json!({
                    "type": "done",
                    "session_name": turn.session.name,
                    "response": turn.response_text,
                    "response_content": turn.response_text,
                    "updated_at": turn.session.updated_at,
                    "steps": 0,
                    "compressed_summary": turn.session.compressed_summary,
                    "delegated": turn.delegated,
                    "completed": turn.completed,
                    "job_id": turn.job_id,
                    "status": turn.status,
                    "role_name": role_name.clone().unwrap_or_else(|| "default".to_string()),
                });
                yield Ok(format!("{}\n", done));
            }
            Err(e) => {
                let err = serde_json::json!({ "type": "error", "message": e.to_string() });
                yield Ok(format!("{}\n", err));
            }
        }
    };
    let body = Body::from_stream(stream);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .map_err(|e| AppError::Internal(e.to_string()))
}

fn job_summary(job: &ConversationJob) -> ChatJobSummaryModel {
    ChatJobSummaryModel {
        job_id: job.job_id.clone(),
        session_name: job.session_name.clone(),
        prompt: job.prompt.clone(),
        role_name: job.role_name.clone(),
        status: job.status.clone(),
        attempt: job.attempt,
        retry_of_job_id: job.retry_of_job_id.clone(),
        can_retry: matches!(job.status.as_str(), "failed" | "cancelled"),
        error: job.error.clone(),
        created_at: job.created_at.clone(),
        started_at: job.started_at.clone(),
        finished_at: job.finished_at.clone(),
        updated_at: job.updated_at.clone(),
    }
}

fn job_response(job: &ConversationJob) -> ChatJobResponse {
    let response_text = if !job.final_response.is_empty() {
        job.final_response.clone()
    } else {
        job.immediate_response.clone()
    };
    ChatJobResponse {
        job_id: job.job_id.clone(),
        session_name: job.session_name.clone(),
        prompt: job.prompt.clone(),
        role_name: job.role_name.clone(),
        status: job.status.clone(),
        attempt: job.attempt,
        retry_of_job_id: job.retry_of_job_id.clone(),
        can_retry: matches!(job.status.as_str(), "failed" | "cancelled"),
        response: response_text.clone(),
        response_content: if !job.final_response_content.is_empty() {
            serde_json::to_value(&job.final_response_content)
                .unwrap_or(serde_json::Value::String(response_text.clone()))
        } else {
            serde_json::Value::String(response_text)
        },
        error: job.error.clone(),
        steps: 0,
        pending_user_input: job
            .pending_user_input
            .as_ref()
            .map(|m| {
                let mut obj = serde_json::Map::new();
                for (k, v) in m {
                    obj.insert(k.clone(), v.clone());
                }
                serde_json::Value::Object(obj)
            }),
        created_at: job.created_at.clone(),
        started_at: job.started_at.clone(),
        finished_at: job.finished_at.clone(),
        updated_at: job.updated_at.clone(),
    }
}

async fn list_chat_jobs(
    State(state): State<AppState>,
    Query(q): Query<ListChatJobsQuery>,
) -> Result<Json<ChatJobsResponse>, AppError> {
    let coordinator = coordinator(&state)?;
    let limit = q.limit.unwrap_or(50);
    let jobs = coordinator
        .list_jobs(q.session_name.as_deref(), q.status.as_deref(), limit)
        .await;
    let models = jobs.iter().map(job_summary).collect();
    Ok(Json(ChatJobsResponse { jobs: models }))
}

async fn get_chat_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<ChatJobResponse>, AppError> {
    let coordinator = coordinator(&state)?;
    let job = coordinator
        .get_job(&job_id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("Job not found: {job_id}")))?;
    Ok(Json(job_response(&job)))
}

async fn get_chat_job_trace(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<ChatJobTraceResponse>, AppError> {
    let coordinator = coordinator(&state)?;
    let trace = coordinator.get_job_trace(&job_id).await;
    let (job, events) = match trace {
        Ok(pair) => pair,
        Err(e) => return Err(AppError::NotFound(e.to_string())),
    };
    let job = job.ok_or_else(|| AppError::NotFound(format!("Job not found: {job_id}")))?;
    let event_values: Vec<serde_json::Value> = events
        .into_iter()
        .map(serde_json::Value::Object)
        .collect();
    Ok(Json(ChatJobTraceResponse {
        job_id: job.job_id.clone(),
        session_name: job.session_name.clone(),
        status: job.status.clone(),
        updated_at: job.updated_at.clone(),
        events: event_values,
    }))
}

async fn cancel_chat_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<ChatJobResponse>, AppError> {
    let coordinator = coordinator(&state)?;
    let job = coordinator
        .cancel_job(&job_id)
        .await
        .ok_or_else(|| AppError::NotFound(format!("Job not found: {job_id}")))?;
    Ok(Json(job_response(&job)))
}

async fn retry_chat_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<ChatResponse>, AppError> {
    let coordinator = coordinator(&state)?;
    let result = coordinator
        .retry_job(&job_id, None)
        .await
        .map_err(|e| {
            if e.to_string().contains("不存在") {
                AppError::NotFound(e.to_string())
            } else {
                AppError::BadRequest(e.to_string())
            }
        })?;
    Ok(Json(ChatResponse {
        session_name: result.session.name.clone(),
        response: result.response_text.clone(),
        response_content: serde_json::Value::String(result.response_text.clone()),
        updated_at: result.session.updated_at.clone(),
        steps: 0,
        compressed_summary: result.session.compressed_summary.clone(),
        delegated: result.delegated,
        completed: result.completed,
        job_id: result.job_id.clone(),
        status: result.status.clone(),
        role_name: "default".to_string(),
    }))
}

// helper trait for ChatResponse to attach the requested route mode
trait WithRouteMode {
    fn with_route_mode(self, route_mode: Option<String>) -> Self;
}

impl WithRouteMode for ChatResponse {
    fn with_route_mode(mut self, route_mode: Option<String>) -> Self {
        if let Some(mode) = route_mode {
            // ChatResponse schema does not carry a route_mode field; we
            // surface the requested mode in the role_name field as a
            // hint that the v1 wire shape diverges from the Python
            // schema. Future versions will add an explicit field.
            if !mode.is_empty() {
                self.role_name = mode;
            }
        }
        self
    }
}

// silence unused imports
#[allow(dead_code)]
fn _unused() {}
