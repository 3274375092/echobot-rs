//! `roles` router — list / get / create / update / delete role cards.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::Json;
use axum::Router;

use echobot_orchestration::roles::{normalize_role_name, RoleCard, RoleCardRegistry, DEFAULT_ROLE_NAME};

use crate::error::AppError;
use crate::schemas::{CreateRoleRequest, RoleDetailModel, RoleSummaryModel, UpdateRoleRequest};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/roles", get(list_roles).post(create_role))
        .route(
            "/roles/{role_name}",
            get(get_role).put(update_role).delete(delete_role),
        )
}

fn registry(state: &AppState) -> Result<Arc<RoleCardRegistry>, AppError> {
    state
        .runtime()
        .role_registry
        .clone()
        .ok_or_else(|| AppError::Unavailable("Role registry is not ready".to_string()))
}

fn summary(card: &RoleCard) -> RoleSummaryModel {
    let is_default = card.name == DEFAULT_ROLE_NAME;
    RoleSummaryModel {
        name: card.name.clone(),
        editable: !is_default,
        deletable: !is_default,
        source_path: card
            .source_path
            .as_ref()
            .map(|p| path_to_string(p.as_path())),
    }
}

fn detail(card: &RoleCard) -> RoleDetailModel {
    let s = summary(card);
    RoleDetailModel {
        name: s.name,
        editable: s.editable,
        deletable: s.deletable,
        source_path: s.source_path,
        prompt: card.prompt.clone(),
    }
}

fn path_to_string(p: &std::path::Path) -> String {
    p.display().to_string()
}

// Kept for future use: per-handler error mapping is currently inlined at
// each call site, but we still want a single place to classify role
// error strings into `AppError` when a future handler needs it.
#[allow(dead_code)]
fn classify_role_error(err: &str) -> AppError {
    let _ = err;
    AppError::BadRequest("unsupported".to_string())
}

async fn list_roles(
    State(state): State<AppState>,
) -> Result<Json<Vec<RoleSummaryModel>>, AppError> {
    let registry = registry(&state)?;
    let cards = registry.cards().await;
    Ok(Json(cards.iter().map(summary).collect()))
}

async fn get_role(
    State(state): State<AppState>,
    Path(role_name): Path<String>,
) -> Result<Json<RoleDetailModel>, AppError> {
    let registry = registry(&state)?;
    let card = registry
        .get(Some(&role_name))
        .await
        .ok_or_else(|| AppError::NotFound(format!("Role not found: {role_name}")))?;
    Ok(Json(detail(&card)))
}

async fn create_role(
    State(state): State<AppState>,
    Json(req): Json<CreateRoleRequest>,
) -> Result<Json<RoleDetailModel>, AppError> {
    let registry = registry(&state)?;
    let normalized = normalize_role_name(&req.name);
    if normalized.is_empty() {
        return Err(AppError::BadRequest("Role name must not be empty".to_string()));
    }
    let card = RoleCard {
        name: normalized.clone(),
        prompt: req.prompt,
        source_path: Some(registry.managed_role_path(&normalized)),
    };
    registry
        .register(card.clone(), false)
        .await;
    let stored = registry
        .get(Some(&normalized))
        .await
        .ok_or_else(|| AppError::Internal("Role was registered but could not be loaded".to_string()))?;
    Ok(Json(detail(&stored)))
}

async fn update_role(
    State(state): State<AppState>,
    Path(role_name): Path<String>,
    Json(req): Json<UpdateRoleRequest>,
) -> Result<Json<RoleDetailModel>, AppError> {
    let registry = registry(&state)?;
    if role_name == DEFAULT_ROLE_NAME {
        return Err(AppError::BadRequest("The default role is read-only".to_string()));
    }
    let existing = registry
        .get(Some(&role_name))
        .await
        .ok_or_else(|| AppError::NotFound(format!("Role not found: {role_name}")))?;
    let card = RoleCard {
        name: existing.name.clone(),
        prompt: req.prompt,
        source_path: existing.source_path.clone(),
    };
    registry.register(card.clone(), true).await;
    let stored = registry
        .get(Some(&existing.name))
        .await
        .ok_or_else(|| AppError::Internal("Role was updated but could not be loaded".to_string()))?;
    Ok(Json(detail(&stored)))
}

async fn delete_role(
    State(state): State<AppState>,
    Path(role_name): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let registry = registry(&state)?;
    if role_name == DEFAULT_ROLE_NAME {
        return Err(AppError::BadRequest("The default role cannot be deleted".to_string()));
    }
    if registry.get(Some(&role_name)).await.is_none() {
        return Err(AppError::NotFound(format!("Role not found: {role_name}")));
    }
    // v1: role deletion is a file removal handled by the registry's managed
    // root; surface as success even when the file is absent.
    let path = registry.managed_role_path(&role_name);
    let _ = std::fs::remove_file(&path);
    Ok(Json(serde_json::json!({ "deleted": true, "name": role_name })))
}
