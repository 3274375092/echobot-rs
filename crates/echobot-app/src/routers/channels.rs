//! `channels` router — definitions / config / status.
//!
//! v1: the channel manager is a stub on `AppRuntime`. The config
//! endpoints return an empty config and the status endpoint returns
//! the empty map reported by `AppRuntime::channel_status()`.

use axum::extract::State;
use axum::routing::get;
use axum::Json;
use axum::Router;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/channels/definitions", get(get_definitions))
        .route("/channels/config", get(get_config).put(update_config))
        .route("/channels/status", get(get_status))
}

async fn get_definitions(
    State(_state): State<AppState>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    Ok(Json(Vec::new()))
}

async fn get_config(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({})))
}

async fn update_config(
    State(_state): State<AppState>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !raw.is_object() {
        return Err(AppError::BadRequest("Channel config must be a JSON object".to_string()));
    }
    Ok(Json(raw))
}

async fn get_status(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!(state.runtime().channel_status())))
}
