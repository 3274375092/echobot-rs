//! `GET /api/health` — runtime health snapshot.

use axum::extract::State;
use axum::Json;

use crate::state::AppState;

pub async fn get_health(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, crate::error::AppError> {
    let snapshot = state.runtime().health_snapshot().await;
    Ok(Json(snapshot))
}
