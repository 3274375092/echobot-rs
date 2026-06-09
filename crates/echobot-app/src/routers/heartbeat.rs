//! `heartbeat` router — get / update the heartbeat file.

use std::path::Path;

use axum::extract::State;
use axum::routing::get;
use axum::Json;
use axum::Router;

use echobot_runtime::scheduling::heartbeat::{
    has_meaningful_heartbeat_content, read_or_create_heartbeat_file, write_heartbeat_file,
};

use crate::error::AppError;
use crate::schemas::{HeartbeatConfigResponse, UpdateHeartbeatRequest};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/heartbeat", get(get_heartbeat).put(update_heartbeat))
}

async fn read_heartbeat(path: &Path) -> Result<String, AppError> {
    read_or_create_heartbeat_file(path)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}

async fn get_heartbeat(
    State(state): State<AppState>,
) -> Result<Json<HeartbeatConfigResponse>, AppError> {
    let runtime = state.runtime();
    let path = runtime.heartbeat_file_path().to_path_buf();
    let content = read_heartbeat(&path).await?;
    let enabled = runtime
        .heartbeat_service()
        .map(|hb| hb.enabled)
        .unwrap_or(false);
    let interval = runtime
        .heartbeat_service()
        .map(|hb| hb.interval_seconds)
        .unwrap_or_else(|| runtime.heartbeat_interval_seconds());
    Ok(Json(HeartbeatConfigResponse {
        enabled,
        interval_seconds: interval.max(1),
        file_path: path.display().to_string(),
        has_meaningful_content: has_meaningful_heartbeat_content(&content),
        content,
    }))
}

async fn update_heartbeat(
    State(state): State<AppState>,
    Json(req): Json<UpdateHeartbeatRequest>,
) -> Result<Json<HeartbeatConfigResponse>, AppError> {
    let runtime = state.runtime();
    let path = runtime.heartbeat_file_path().to_path_buf();
    write_heartbeat_file(&path, &req.content)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let enabled = runtime
        .heartbeat_service()
        .map(|hb| hb.enabled)
        .unwrap_or(false);
    let interval = runtime
        .heartbeat_service()
        .map(|hb| hb.interval_seconds)
        .unwrap_or_else(|| runtime.heartbeat_interval_seconds());
    Ok(Json(HeartbeatConfigResponse {
        enabled,
        interval_seconds: interval.max(1),
        file_path: path.display().to_string(),
        has_meaningful_content: has_meaningful_heartbeat_content(&req.content),
        content: req.content,
    }))
}
