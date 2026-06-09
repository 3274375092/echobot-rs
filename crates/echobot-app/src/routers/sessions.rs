//! `sessions` router — list / load current / switch / create /
//! rename / delete / set role / set route mode.

use axum::extract::{Path, State};
use axum::routing::{delete, get, patch, put};
use axum::Json;
use axum::Router;

use echobot_core::models::LLMMessage;
use echobot_orchestration::route_modes::RouteMode;
use echobot_orchestration::roles::{role_name_from_metadata, RoleCardRegistry};
use echobot_orchestration::ConversationCoordinator;

use crate::error::AppError;
use crate::schemas::{
    CreateSessionRequest, RenameSessionRequest, SessionDetailModel, SessionSummaryModel,
    SetCurrentSessionRequest, SetSessionRoleRequest, SetSessionRouteModeRequest,
};
use crate::services::session_service::SessionService;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/current", get(get_current_session).put(set_current_session))
        .route(
            "/sessions/{session_name}",
            get(get_session).patch(rename_session).delete(delete_session),
        )
        .route("/sessions/{session_name}/role", put(set_session_role))
        .route("/sessions/{session_name}/route-mode", put(set_session_route_mode))
}

async fn service(state: &AppState) -> Result<SessionService, AppError> {
    let runtime = state.runtime();
    let store = std::sync::Arc::new(runtime.session_store().clone());
    Ok(SessionService::new(store))
}

async fn list_sessions(
    State(state): State<AppState>,
) -> Result<Json<Vec<SessionSummaryModel>>, AppError> {
    let svc = service(&state).await?;
    let items = svc.list_sessions().await?;
    let models = items.into_iter().map(SessionSummaryModel::from).collect();
    Ok(Json(models))
}

async fn get_current_session(
    State(state): State<AppState>,
) -> Result<Json<SessionDetailModel>, AppError> {
    let svc = service(&state).await?;
    let session = svc.load_current_session().await?;
    Ok(Json(session_detail_from_chat_session(&session)))
}

async fn set_current_session(
    State(state): State<AppState>,
    Json(req): Json<SetCurrentSessionRequest>,
) -> Result<Json<SessionDetailModel>, AppError> {
    let svc = service(&state).await?;
    let session = svc.switch_session(&req.name).await?;
    // Persist the new current-session pointer.
    svc.store()
        .set_current_session(&session.name)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(session_detail_from_chat_session(&session)))
}

async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionDetailModel>, AppError> {
    let svc = service(&state).await?;
    let session = svc.create_session(req.name.as_deref()).await?;
    Ok(Json(session_detail_from_chat_session(&session)))
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_name): Path<String>,
) -> Result<Json<SessionDetailModel>, AppError> {
    let svc = service(&state).await?;
    let session = svc.load_session(&session_name).await?;
    Ok(Json(session_detail_from_chat_session(&session)))
}

async fn rename_session(
    State(state): State<AppState>,
    Path(session_name): Path<String>,
    Json(req): Json<RenameSessionRequest>,
) -> Result<Json<SessionDetailModel>, AppError> {
    let svc = service(&state).await?;
    let session = svc.rename_session(&session_name, &req.name).await?;
    Ok(Json(session_detail_from_chat_session(&session)))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(session_name): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = service(&state).await?;
    let deleted = svc.delete_session(&session_name).await?;
    if !deleted {
        return Err(AppError::NotFound(format!("Session not found: {session_name}")));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn set_session_role(
    State(state): State<AppState>,
    Path(session_name): Path<String>,
    Json(req): Json<SetSessionRoleRequest>,
) -> Result<Json<SessionDetailModel>, AppError> {
    let runtime = state.runtime();
    let coordinator = runtime
        .coordinator
        .clone()
        .ok_or_else(|| AppError::Unavailable("Conversation coordinator is not ready".to_string()))?;
    coordinator
        .set_session_role(&session_name, &req.role_name)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let svc = service(&state).await?;
    let session = svc.load_session(&session_name).await?;
    Ok(Json(session_detail_from_chat_session(&session)))
}

async fn set_session_route_mode(
    State(state): State<AppState>,
    Path(session_name): Path<String>,
    Json(req): Json<SetSessionRouteModeRequest>,
) -> Result<Json<SessionDetailModel>, AppError> {
    let runtime = state.runtime();
    let coordinator = runtime
        .coordinator
        .clone()
        .ok_or_else(|| AppError::Unavailable("Conversation coordinator is not ready".to_string()))?;
    let mode: RouteMode = RouteMode::parse(&req.route_mode);
    coordinator
        .set_session_route_mode(&session_name, mode)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let svc = service(&state).await?;
    let session = svc.load_session(&session_name).await?;
    Ok(Json(session_detail_from_chat_session(&session)))
}

// `store` accessor lives in `crate::services::session_service`.

// Conversion helpers -----------------------------------------------------

impl From<echobot_runtime::sessions::SessionInfo> for SessionSummaryModel {
    fn from(info: echobot_runtime::sessions::SessionInfo) -> Self {
        Self {
            name: info.name,
            message_count: info.message_count,
            updated_at: info.updated_at,
        }
    }
}

pub(crate) fn session_detail_from_chat_session(
    session: &echobot_runtime::sessions::ChatSession,
) -> SessionDetailModel {
    let route_mode = session
        .metadata
        .get("route_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .to_string();
    let role_name = role_name_from_metadata(Some(&session.metadata));
    let history: Vec<crate::schemas::MessageModel> = session
        .history
        .iter()
        .map(message_to_model)
        .collect();
    SessionDetailModel {
        name: session.name.clone(),
        updated_at: session.updated_at.clone(),
        compressed_summary: session.compressed_summary.clone(),
        role_name,
        route_mode,
        history,
    }
}

fn message_to_model(m: &LLMMessage) -> crate::schemas::MessageModel {
    use echobot_core::models::MessageContent;
    let content = match &m.content {
        MessageContent::Text(t) => serde_json::Value::String(t.clone()),
        MessageContent::Blocks(blocks) => {
            let values: Vec<serde_json::Value> = blocks
                .iter()
                .map(|b| b.to_value())
                .collect();
            serde_json::Value::Array(values)
        }
    };
    let tool_calls = m
        .tool_calls
        .iter()
        .map(|tc| crate::schemas::ToolCallModel {
            id: tc.id.clone(),
            name: tc.name.clone(),
            arguments: tc.arguments.clone(),
        })
        .collect();
    crate::schemas::MessageModel {
        role: m.role.as_str().to_string(),
        content,
        name: m.name.clone(),
        tool_call_id: m.tool_call_id.clone(),
        tool_calls,
    }
}

// Quiet unused imports for items that are wired into the wider handler
// surface but only referenced through the conversion helpers above.
#[allow(dead_code)]
fn _force_link(
    _registry: &RoleCardRegistry,
    _coordinator: &ConversationCoordinator,
    _set_role_name: fn(&mut std::collections::HashMap<String, serde_json::Value>, &str) -> bool,
) {
}
