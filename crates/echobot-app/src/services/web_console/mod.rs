//! Web console service — mirrors `echobot.app.services.web_console`.

pub mod live2d;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use echobot_asr::service::AsrService;
use echobot_tts::service::TtsService;

use self::live2d::{Live2DService, UploadFile};

/// Aggregated web console service.
pub struct WebConsoleService {
    pub workspace: PathBuf,
    pub tts_service: Option<Arc<TtsService>>,
    pub asr_service: Option<Arc<AsrService>>,
    live2d_service: Arc<Live2DService>,
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
        let live2d = Arc::new(Live2DService::new(
            workspace.join(".echobot").join("live2d"),
            workspace.join("builtin_live2d"),
        ));
        Self {
            workspace,
            tts_service,
            asr_service,
            live2d_service: live2d,
        }
    }

    pub async fn initialize_runtime_settings(&self) -> bool {
        if let Some(asr) = &self.asr_service {
            let _ = asr.on_startup().await;
            true
        } else {
            false
        }
    }

    pub async fn build_frontend_config(
        &self,
        session_name: String,
        role_name: String,
        route_mode: String,
        runtime_config: Value,
    ) -> Value {
        let live2d = self
            .live2d_service
            .build_config()
            .unwrap_or_else(|| self.live2d_service.empty_config());
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

    /// Return the live2d config payload (model list, selection, etc.).
    pub fn live2d_config(&self) -> Value {
        self.live2d_service
            .build_config()
            .unwrap_or_else(|| self.live2d_service.empty_config())
    }

    pub fn resolve_live2d_asset(&self, asset_path: &str) -> Result<PathBuf, String> {
        self.live2d_service
            .resolve_asset(asset_path)
            .map_err(|e| e.to_string())
    }

    pub fn resolve_stage_background_asset(
        &self,
        _asset_path: &str,
    ) -> Result<PathBuf, String> {
        Err("Stage backgrounds are not enabled in v1".to_string())
    }

    pub async fn render_live2d_model_json(&self, asset_path: &str) -> Vec<u8> {
        self.live2d_service
            .render_model_json(asset_path)
            .map(|s| s.into_bytes())
            .unwrap_or_default()
    }

    pub async fn save_stage_background(
        &self,
        _filename: String,
        _content_type: Option<String>,
        _file_bytes: Vec<u8>,
    ) -> Result<Value, String> {
        Ok(self.stage_config_payload())
    }

    pub async fn save_live2d_directory(
        &self,
        uploaded_files: Vec<UploadFile>,
    ) -> Result<Value, String> {
        self.live2d_service
            .save_directory(&uploaded_files)
            .map_err(|e| e.to_string())
    }

    pub async fn save_live2d_annotation(
        &self,
        selection_key: String,
        kind: String,
        file: String,
        note: String,
    ) -> Result<Value, String> {
        self.live2d_service
            .save_annotation(&selection_key, &kind, &file, &note)
            .map_err(|e| e.to_string())
    }

    pub async fn save_live2d_hotkey(
        &self,
        selection_key: String,
        hotkey_key: String,
        shortcut_tokens: Vec<String>,
        restore_default: bool,
    ) -> Result<Value, String> {
        self.live2d_service
            .save_hotkey(&selection_key, &hotkey_key, &shortcut_tokens, restore_default)
            .map_err(|e| e.to_string())
    }

    pub async fn set_selected_asr_provider(&self, _provider: String) -> Result<Value, String> {
        Ok(self.asr_config_payload().await)
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
