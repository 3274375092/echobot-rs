//! `cron` router — status, list, delete.

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get};
use axum::Json;
use axum::Router;
use serde::Deserialize;

use echobot_runtime::scheduling::cron::summarize_job;

use crate::error::AppError;
use crate::schemas::{CronDeleteResponse, CronJobModel, CronJobsResponse, CronStatusResponse};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cron/status", get(get_cron_status))
        .route("/cron/jobs", get(list_cron_jobs))
        .route("/cron/jobs/{job_id}", delete(delete_cron_job))
}

#[derive(Debug, Deserialize, Default)]
pub struct ListCronJobsQuery {
    #[serde(default)]
    pub include_disabled: Option<bool>,
}

async fn get_cron_status(
    State(state): State<AppState>,
) -> Result<Json<CronStatusResponse>, AppError> {
    let cron = state.runtime().cron_service().clone();
    let status = cron.status().await;
    let payload = status.to_value();
    Ok(Json(CronStatusResponse {
        enabled: payload.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        jobs: payload
            .get("jobs")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(0),
        next_run_at: payload
            .get("next_run_at")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    }))
}

async fn list_cron_jobs(
    State(state): State<AppState>,
    Query(q): Query<ListCronJobsQuery>,
) -> Result<Json<CronJobsResponse>, AppError> {
    let cron = state.runtime().cron_service().clone();
    let include_disabled = q.include_disabled.unwrap_or(false);
    let jobs: Vec<CronJobModel> = cron
        .list_jobs(include_disabled)
        .await
        .into_iter()
        .map(|job| {
            let summary = summarize_job(&job);
            let summary_map = match summary.as_object() {
                Some(m) => m,
                None => return CronJobModel {
                    id: job.id.clone(),
                    name: job.name.clone(),
                    enabled: job.enabled,
                    schedule: String::new(),
                    payload_kind: "agent".to_string(),
                    session_name: "default".to_string(),
                    next_run_at: job.state.next_run_at.clone(),
                    last_run_at: job.state.last_run_at.clone(),
                    last_status: job.state.last_status.clone(),
                    last_error: job.state.last_error.clone(),
                },
            };
            CronJobModel {
                id: summary_map.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                name: summary_map.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                enabled: summary_map.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                schedule: cron_schedule_string(summary_map),
                payload_kind: summary_map
                    .get("task_kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("agent")
                    .to_string(),
                session_name: summary_map
                    .get("session_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string(),
                next_run_at: job
                    .state
                    .next_run_at
                    .clone()
                    .or_else(|| {
                        summary_map
                            .get("next_run_at")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    }),
                last_run_at: job.state.last_run_at.clone(),
                last_status: job.state.last_status.clone(),
                last_error: job.state.last_error.clone(),
            }
        })
        .collect();
    Ok(Json(CronJobsResponse { jobs }))
}

fn cron_schedule_string(summary: &serde_json::Map<String, serde_json::Value>) -> String {
    let kind = summary.get("schedule_kind").and_then(|v| v.as_str()).unwrap_or("");
    let value = summary.get("schedule_value").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "at" => format!("at:{value}"),
        "every" => format!("every:{value}"),
        "cron" => value.to_string(),
        _ => value.to_string(),
    }
}

async fn delete_cron_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<CronDeleteResponse>, AppError> {
    let cron = state.runtime().cron_service().clone();
    let removed = cron
        .remove_job(&job_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if !removed {
        return Err(AppError::NotFound(format!("Cron job not found: {job_id}")));
    }
    Ok(Json(CronDeleteResponse {
        deleted: true,
        job_id,
    }))
}
