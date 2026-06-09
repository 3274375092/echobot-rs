//! Web console service stub.
//!
//! Mirrors `echobot.app.services.web_console`. The v1 port returns
//! empty configuration for Live2D, stage backgrounds, and the
//! frontend-config aggregation. Future versions will populate them
//! from the workspace `.echobot/live2d/` directory.

pub mod live2d;

pub use live2d::{metadata::Live2DMetadataService, Live2DService, Live2DUploadFile};

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use echobot_asr::service::AsrService;
use echobot_tts::service::TtsService;

/// Aggregated web console service.
pub struct WebConsoleService {
    /// Workspace root (`.echobot/`'s parent).
    pub workspace: PathBuf,
    /// TTS service (may be `None` if construction failed).
    pub tts_service: Option<Arc<TtsService>>,
    /// ASR service (may be `None` if construction failed).
    pub asr_service: Option<Arc<AsrService>>,
}

impl std::fmt::Debug for WebConsoleService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebConsoleService")
            .field("workspace", &self.workspace)
            .finish_non_exhaustive()
    }
}

impl WebConsoleService {
    pub fn new(
        workspace: PathBuf,
        tts_service: Option<Arc<TtsService>>,
        asr_service: Option<Arc<AsrService>>,
    ) -> Self {
        Self {
            workspace,
            tts_service,
            asr_service,
        }
    }

    /// Initialise runtime settings (asr providers, etc.). Returns
    /// `true` if asr was initialised from persisted settings, `false`
    /// otherwise.
    pub async fn initialize_runtime_settings(&self) -> bool {
        if let Some(asr) = &self.asr_service {
            // on_startup is best-effort; v1 doesn't depend on its result.
            let _ = asr.on_startup().await;
            true
        } else {
            false
        }
    }

    /// Build the `WebConfigResponse` payload for `GET /api/web/config`.
    pub async fn build_frontend_config(
        &self,
        session_name: String,
        role_name: String,
        route_mode: String,
        runtime_config: Value,
    ) -> Value {
        let live2d = self.live2d_config_payload();
        let stage = self.stage_config_payload();
        let asr = self.asr_config_payload().await;
        let tts = self.tts_config_payload().await;
        json!({
            "session_name": session_name,
            "role_name": role_name,
            "route_mode": route_mode,
            "runtime": runtime_config,
            "live2d": live2d,
            "stage": stage,
            "asr": asr,
            "tts": tts,
        })
    }

    /// Resolve a Live2D asset path under `.echobot/live2d/`.
    pub fn resolve_live2d_asset(&self, _asset_path: &str) -> Result<PathBuf, String> {
        Err("Live2D assets are not enabled in v1".to_string())
    }

    /// Resolve a stage background asset path.
    pub fn resolve_stage_background_asset(
        &self,
        _asset_path: &str,
    ) -> Result<PathBuf, String> {
        Err("Stage backgrounds are not enabled in v1".to_string())
    }

    /// Render a Live2D `.model3.json` file with the workspace's
    /// expressions / motions / hotkeys. v1 returns the model JSON
    /// unchanged.
    pub async fn render_live2d_model_json(&self, _asset_path: &str) -> Vec<u8> {
        Vec::new()
    }

    /// Save a stage background upload. v1 returns an empty config.
    pub async fn save_stage_background(
        &self,
        _filename: String,
        _content_type: Option<String>,
        _file_bytes: Vec<u8>,
    ) -> Result<Value, String> {
        Ok(self.stage_config_payload())
    }

    /// Save a Live2D directory upload. v1 returns an empty config.
    pub async fn save_live2d_directory(
        &self,
        _uploaded_files: Vec<Live2DUploadFile>,
    ) -> Result<Value, String> {
        Ok(self.live2d_config_payload())
    }

    /// Save a Live2D annotation note.
    pub async fn save_live2d_annotation(
        &self,
        selection_key: String,
        kind: String,
        file: String,
        note: String,
    ) -> Result<Value, String> {
        Ok(json!({
            "selection_key": selection_key,
            "kind": kind,
            "file": file,
            "note": note,
        }))
    }

    /// Save a Live2D hotkey override.
    pub async fn save_live2d_hotkey(
        &self,
        selection_key: String,
        hotkey_key: String,
        shortcut_tokens: Vec<String>,
        _restore_default: bool,
    ) -> Result<Value, String> {
        Ok(json!({
            "selection_key": selection_key,
            "hotkey_key": hotkey_key,
            "hotkey_id": hotkey_key,
            "name": hotkey_key,
            "action": "",
            "file": "",
            "shortcut_tokens": shortcut_tokens,
            "shortcut_label": shortcut_tokens.join(" + "),
            "target_kind": "",
            "supported": false,
        }))
    }

    /// Switch the active ASR provider. v1 returns the current
    /// snapshot unchanged.
    pub async fn set_selected_asr_provider(&self, _provider: String) -> Result<Value, String> {
        Ok(self.asr_config_payload().await)
    }

    fn live2d_config_payload(&self) -> Value {
        json!({
            "available": false,
            "models": [],
        })
    }

    fn stage_config_payload(&self) -> Value {
        json!({
            "default_background_key": "default",
            "backgrounds": [
                {
                    "key": "default",
                    "label": "不使用背景",
                    "url": "",
                    "kind": "none",
                }
            ],
        })
    }

    async fn asr_config_payload(&self) -> Value {
        match &self.asr_service {
            Some(asr) => match asr.status_snapshot().await {
                Ok(snap) => serde_json::to_value(snap).unwrap_or_else(|_| json!({})),
                Err(_) => json!({}),
            },
            None => json!({}),
        }
    }

    async fn tts_config_payload(&self) -> Value {
        match &self.tts_service {
            Some(tts) => {
                let providers = tts.providers_status();
                let provider_values: Vec<Value> = providers
                    .into_iter()
                    .map(|p| {
                        json!({
                            "name": p.name,
                            "label": p.label,
                            "available": p.available,
                            "state": p.state,
                            "detail": p.detail,
                        })
                    })
                    .collect();
                let default_provider = tts.default_provider_name().to_string();
                let default_voice = tts
                    .default_voice_for(Some(&default_provider))
                    .unwrap_or_default();
                json!({
                    "default_provider": default_provider,
                    "default_voice": default_voice,
                    "default_voices": {},
                    "providers": provider_values,
                })
            }
            None => json!({
                "default_provider": "edge",
                "default_voice": "",
                "default_voices": {},
                "providers": [],
            }),
        }
    }
}
