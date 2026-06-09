//! `web` router — web console configuration, TTS / ASR endpoints,
//! Live2D / stage assets, the ASR WebSocket.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::Json;
use axum::Router;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;

use echobot_asr::service::AsrService;
use echobot_runtime::settings::RuntimeSettingsManager;

use crate::error::AppError;
use crate::schemas::{
    ASRTranscriptionResponse, TTSRequest, TTSVoiceModel, TTSVoicesResponse,
    UpdateWebASRProviderRequest, UpdateWebLive2DAnnotationRequest, UpdateWebLive2DHotkeyRequest,
    UpdateWebRuntimeConfigRequest, WebASRConfigModel, WebConfigResponse,
    WebLive2DAnnotationResponse, WebLive2DConfigModel, WebLive2DHotkeyResponse,
    WebRuntimeConfigModel, WebStageConfigModel,
};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    // The asset path catch-all and the SPA-style fallthrough are
    // handled by `create_app::fallback(serve_static)`. The brace-prefix
    // catch-all syntax `{*name}` is rejected by matchit 0.7 (the
    // version bundled with axum 0.7.9) once any sibling route is
    // registered, so we keep the router to its explicit API endpoints
    // and let the top-level fallback dispatch asset requests.
    Router::new()
        .route("/config", get(get_web_config))
        .route("/runtime", patch(update_web_runtime))
        .route("/runtime/reset", post(reset_web_runtime))
        .route("/live2d", post(upload_live2d).get(get_live2d_index))
        .route("/live2d/annotations", patch(update_live2d_annotation))
        .route("/live2d/hotkeys", patch(update_live2d_hotkey))
        .route("/stage/backgrounds", post(upload_stage_background))
        .route("/tts/voices", get(get_tts_voices))
        .route("/tts", post(synthesize_tts))
        .route("/asr/status", get(get_asr_status))
        .route("/asr/provider", patch(update_asr_provider))
        .route("/asr", post(transcribe_audio))
        .route("/asr/ws", get(asr_websocket))
}

fn web_console(state: &AppState) -> Option<Arc<crate::services::web_console::WebConsoleService>> {
    state.runtime().web_console_service.clone()
}

fn runtime_settings_manager(
    state: &AppState,
) -> Result<Arc<RuntimeSettingsManager<echobot_runtime::bootstrap::RuntimeControlsCoordinator>>, AppError>
{
    let ctx = &state.runtime().context;
    ctx.settings_manager
        .clone()
        .ok_or_else(|| AppError::Unavailable("Settings manager is not ready".to_string()))
}

async fn get_web_config(
    State(state): State<AppState>,
) -> Result<Json<WebConfigResponse>, AppError> {
    let runtime = state.runtime();
    let coordinator = runtime
        .coordinator
        .clone()
        .ok_or_else(|| AppError::Unavailable("Coordinator is not ready".to_string()))?;
    let session = runtime
        .session_store()
        .load_current_session()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let role_name = coordinator
        .current_role_name(&session.name)
        .await
        .unwrap_or_else(|_| "default".to_string());
    let route_mode = match coordinator
        .current_route_mode(&session.name)
        .await
    {
        Ok(mode) => mode.as_str().to_string(),
        Err(_) => "auto".to_string(),
    };
    let sm = runtime_settings_manager(&state)?;
    let runtime_snapshot = sm
        .snapshot()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let payload = console
        .build_frontend_config(
            session.name.clone(),
            role_name,
            route_mode,
            runtime_snapshot,
        )
        .await;
    let value: WebConfigResponse =
        serde_json::from_value(payload).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(value))
}

async fn update_web_runtime(
    State(state): State<AppState>,
    Json(req): Json<UpdateWebRuntimeConfigRequest>,
) -> Result<Json<WebRuntimeConfigModel>, AppError> {
    let sm = runtime_settings_manager(&state)?;
    let mut updates = std::collections::HashMap::new();
    if let Some(v) = req.delegated_ack_enabled {
        updates.insert("delegated_ack_enabled".to_string(), Value::Bool(v));
    }
    if let Some(v) = req.shell_safety_mode {
        updates.insert("shell_safety_mode".to_string(), Value::String(v));
    }
    if let Some(v) = req.file_write_enabled {
        updates.insert("file_write_enabled".to_string(), Value::Bool(v));
    }
    if let Some(v) = req.cron_mutation_enabled {
        updates.insert("cron_mutation_enabled".to_string(), Value::Bool(v));
    }
    if let Some(v) = req.web_private_network_enabled {
        updates.insert("web_private_network_enabled".to_string(), Value::Bool(v));
    }
    let snapshot = sm
        .apply_updates(updates)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let value: WebRuntimeConfigModel =
        serde_json::from_value(snapshot).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(value))
}

async fn reset_web_runtime(
    State(state): State<AppState>,
) -> Result<Json<WebRuntimeConfigModel>, AppError> {
    let sm = runtime_settings_manager(&state)?;
    let default = state.runtime().context.default_runtime_config.clone();
    let defaults_map: std::collections::HashMap<String, Value> = std::collections::HashMap::from([
        ("delegated_ack_enabled".to_string(), Value::Bool(default.delegated_ack_enabled)),
        ("shell_safety_mode".to_string(), Value::String(default.shell_safety_mode)),
        ("file_write_enabled".to_string(), Value::Bool(default.file_write_enabled)),
        ("cron_mutation_enabled".to_string(), Value::Bool(default.cron_mutation_enabled)),
        ("web_private_network_enabled".to_string(), Value::Bool(default.web_private_network_enabled)),
    ]);
    let snapshot = sm
        .reset_overrides(defaults_map)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let value: WebRuntimeConfigModel =
        serde_json::from_value(snapshot).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(value))
}

async fn upload_stage_background(
    State(state): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<WebStageConfigModel>, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let mut bytes = Vec::new();
    let mut filename = String::new();
    let mut content_type: Option<String> = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        if field.name() != Some("image") {
            continue;
        }
        if let Some(name) = field.file_name() {
            filename = name.to_string();
        }
        if let Some(ctype) = field.content_type() {
            content_type = Some(ctype.to_string());
        }
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| AppError::BadRequest(e.to_string()))?
        {
            bytes.extend_from_slice(&chunk);
        }
        break;
    }
    let payload = console
        .save_stage_background(filename, content_type, bytes)
        .await
        .map_err(AppError::BadRequest)?;
    let value: WebStageConfigModel =
        serde_json::from_value(payload).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(value))
}

async fn upload_live2d(
    State(state): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<WebLive2DConfigModel>, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let mut uploaded: Vec<crate::services::web_console::Live2DUploadFile> = Vec::new();
    let mut relative_paths: Vec<String> = Vec::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "files" {
            let mut bytes = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?
            {
                bytes.extend_from_slice(&chunk);
            }
            let placeholder = field.file_name().unwrap_or("").to_string();
            uploaded.push(crate::services::web_console::Live2DUploadFile {
                relative_path: placeholder,
                file_bytes: bytes,
            });
        } else if name == "relative_paths" {
            let mut buf = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?
            {
                buf.extend_from_slice(&chunk);
            }
            relative_paths.push(String::from_utf8_lossy(&buf).to_string());
        }
    }
    if uploaded.len() != relative_paths.len() {
        return Err(AppError::BadRequest(
            "Uploaded Live2D files and paths do not match".to_string(),
        ));
    }
    for (i, path) in relative_paths.iter().enumerate() {
        uploaded[i].relative_path = path.clone();
    }
    let payload = console
        .save_live2d_directory(uploaded)
        .await
        .map_err(AppError::BadRequest)?;
    let value: WebLive2DConfigModel =
        serde_json::from_value(payload).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(value))
}

async fn update_live2d_annotation(
    State(state): State<AppState>,
    Json(req): Json<UpdateWebLive2DAnnotationRequest>,
) -> Result<Json<WebLive2DAnnotationResponse>, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let payload = console
        .save_live2d_annotation(req.selection_key, req.kind, req.file, req.note)
        .await
        .map_err(AppError::BadRequest)?;
    let value: WebLive2DAnnotationResponse =
        serde_json::from_value(payload).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(value))
}

async fn update_live2d_hotkey(
    State(state): State<AppState>,
    Json(req): Json<UpdateWebLive2DHotkeyRequest>,
) -> Result<Json<WebLive2DHotkeyResponse>, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let payload = console
        .save_live2d_hotkey(
            req.selection_key,
            req.hotkey_key,
            req.shortcut_tokens,
            req.restore_default,
        )
        .await
        .map_err(AppError::BadRequest)?;
    let value: WebLive2DHotkeyResponse =
        serde_json::from_value(payload).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(value))
}

async fn get_live2d_index() -> Result<Response, AppError> {
    Err(AppError::NotFound(
        "Live2D model list is not enabled in v1".to_string(),
    ))
}

// Kept for future use: the live2d asset fetchers are not currently
// wired into the router (the v1 web console has live2d metadata but
// does not yet serve the binary assets directly). They are part of the
// established public surface and will be enabled in a follow-up.
#[allow(dead_code)]
async fn get_live2d_asset(
    State(state): State<AppState>,
    Path(asset_path): Path<String>,
) -> Result<Response, AppError> {
    get_live2d_asset_inner(state, &asset_path).await
}

#[allow(dead_code)]
async fn get_live2d_asset_inner(
    state: AppState,
    asset_path: &str,
) -> Result<Response, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let asset = console
        .resolve_live2d_asset(asset_path)
        .map_err(AppError::BadRequest)?;
    let bytes = tokio::fs::read(&asset)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let mime = mime_guess::from_path(&asset).first_or_octet_stream();
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, mime.to_string())], Body::from(bytes))
        .into_response())
}

#[allow(dead_code)]
async fn get_stage_background(
    State(state): State<AppState>,
    Path(asset_path): Path<String>,
) -> Result<Response, AppError> {
    get_stage_background_inner(state, &asset_path).await
}

#[allow(dead_code)]
async fn get_stage_background_inner(
    state: AppState,
    asset_path: &str,
) -> Result<Response, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let asset = console
        .resolve_stage_background_asset(asset_path)
        .map_err(AppError::BadRequest)?;
    let bytes = tokio::fs::read(&asset)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let mime = mime_guess::from_path(&asset).first_or_octet_stream();
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, mime.to_string())], Body::from(bytes))
        .into_response())
}

#[derive(Debug, Deserialize, Default)]
pub struct TtsVoicesQuery {
    #[serde(default)]
    pub provider: Option<String>,
}

async fn get_tts_voices(
    State(state): State<AppState>,
    Query(q): Query<TtsVoicesQuery>,
) -> Result<Json<TTSVoicesResponse>, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let tts = console
        .tts_service
        .clone()
        .ok_or_else(|| AppError::Unavailable("TTS service is not ready".to_string()))?;
    let voices = tts
        .list_voices(q.provider.as_deref())
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let provider_name = q.provider.unwrap_or_else(|| tts.default_provider_name().to_string());
    let models = voices
        .into_iter()
        .map(|v| TTSVoiceModel {
            name: v.name,
            short_name: v.short_name,
            locale: v.locale,
            gender: v.gender,
            display_name: v.display_name,
        })
        .collect();
    Ok(Json(TTSVoicesResponse {
        provider: provider_name,
        voices: models,
    }))
}

async fn synthesize_tts(
    State(state): State<AppState>,
    Json(req): Json<TTSRequest>,
) -> Result<Response, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let tts = console
        .tts_service
        .clone()
        .ok_or_else(|| AppError::Unavailable("TTS service is not ready".to_string()))?;
    let text = req.text.trim();
    if text.is_empty() {
        return Err(AppError::BadRequest("TTS text must not be empty".to_string()));
    }
    let speech = tts
        .synthesize(
            text,
            req.provider.as_deref(),
            req.voice.as_deref(),
            req.rate.as_deref(),
            req.volume.as_deref(),
            req.pitch.as_deref(),
        )
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, speech.content_type)
        .header("X-TTS-Provider", speech.provider)
        .header("X-TTS-Voice", speech.voice)
        .body(Body::from(speech.audio_bytes))
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(response)
}

async fn get_asr_status(
    State(state): State<AppState>,
) -> Result<Json<WebASRConfigModel>, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let asr: &Arc<AsrService> = console
        .asr_service
        .as_ref()
        .ok_or_else(|| AppError::Unavailable("ASR service is not ready".to_string()))?;
    let snap = asr
        .status_snapshot()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let value = serde_json::to_value(snap).map_err(|e| AppError::Internal(e.to_string()))?;
    let model: WebASRConfigModel =
        serde_json::from_value(value).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(model))
}

async fn update_asr_provider(
    State(state): State<AppState>,
    Json(req): Json<UpdateWebASRProviderRequest>,
) -> Result<Json<WebASRConfigModel>, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let payload = console
        .set_selected_asr_provider(req.provider)
        .await
        .map_err(AppError::BadRequest)?;
    let model: WebASRConfigModel =
        serde_json::from_value(payload).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(model))
}

async fn transcribe_audio(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<ASRTranscriptionResponse>, AppError> {
    if body.is_empty() {
        return Err(AppError::BadRequest(
            "ASR audio body must not be empty".to_string(),
        ));
    }
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let asr = console
        .asr_service
        .as_ref()
        .ok_or_else(|| AppError::Unavailable("ASR service is not ready".to_string()))?;
    let result = asr
        .transcribe_wav_bytes(&body)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(Json(ASRTranscriptionResponse {
        text: result.text,
        language: result.language,
    }))
}

async fn asr_websocket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    let console = web_console(&state)
        .ok_or_else(|| AppError::Unavailable("Web console service is not ready".to_string()))?;
    let asr = console
        .asr_service
        .clone()
        .ok_or_else(|| AppError::Unavailable("ASR service is not ready".to_string()))?;
    Ok(ws.on_upgrade(move |socket| asr_ws_loop(socket, asr)))
}

async fn asr_ws_loop(socket: WebSocket, asr: Arc<AsrService>) {
    // v1: the WS handler is wired but the realtime session uses a
    // `Box<dyn VadSession>` (not `Sync`), so we cannot hold a session
    // across the closure without an `Arc<Mutex<…>>`. We create the
    // session on demand for each binary frame; production deployments
    // will swap in a channel-friendly design in a follow-up.
    let (mut sender, mut receiver) = socket.split();
    if let Ok(snap) = asr.status_snapshot().await {
        let _ = sender
            .send(Message::Text(
                serde_json::json!({
                    "type": "ready",
                    "sample_rate": snap.sample_rate,
                    "state": snap.state,
                    "detail": snap.detail,
                })
                .to_string(),
            ))
            .await;
    } else {
        let _ = sender
            .send(Message::Text(
                serde_json::json!({ "type": "error", "message": "ASR is not ready" }).to_string(),
            ))
            .await;
        let _ = sender.close().await;
        return;
    }
    while let Some(message) = receiver.next().await {
        let msg = match message {
            Ok(Message::Close(_)) => break,
            Ok(m) => m,
            Err(_) => break,
        };
        match msg {
            Message::Binary(_bytes) => {
                // Production: forward to a held realtime session. v1
                // does not maintain a long-lived session.
                let _ = sender
                    .send(Message::Text(
                        serde_json::json!({ "type": "ignored", "reason": "v1 stub" })
                            .to_string(),
                    ))
                    .await;
            }
            Message::Text(text) => {
                if text == "flush" {
                    let _ = sender
                        .send(Message::Text(
                            serde_json::json!({ "type": "flush_complete" }).to_string(),
                        ))
                        .await;
                } else if text == "reset" {
                    let _ = sender
                        .send(Message::Text(
                            serde_json::json!({ "type": "reset" }).to_string(),
                        ))
                        .await;
                }
            }
            _ => {}
        }
    }
    let _ = sender.close().await;
}

