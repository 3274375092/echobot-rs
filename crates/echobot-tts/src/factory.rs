//! `echobot-tts::factory` — build a [`TtsService`] from environment
//! variables.
//!
//! Mirrors `echobot/tts/factory.py`. Environment variables honoured:
//!
//! * `ECHOBOT_TTS_PROVIDER`              — default provider name
//!   (default: `"edge"`).
//! * `ECHOBOT_TTS_OPENAI_API_KEY`        — OpenAI bearer token (use
//!   `EMPTY` for local servers).
//! * `ECHOBOT_TTS_OPENAI_MODEL`          — model name (required for the
//!   openai-compatible provider).
//! * `ECHOBOT_TTS_OPENAI_BASE_URL`       — base URL (default:
//!   `https://api.openai.com/v1`).
//! * `ECHOBOT_TTS_OPENAI_TIMEOUT`        — request timeout seconds
//!   (default: `60`).
//! * `ECHOBOT_TTS_OPENAI_DEFAULT_VOICE`  — default voice (default:
//!   `alloy`).
//! * `ECHOBOT_TTS_OPENAI_RESPONSE_FORMAT` — `mp3` / `opus` / `wav` / ...
//!   (default: `wav`).
//! * `ECHOBOT_TTS_OPENAI_VOICES`         — comma-separated voice list.
//! * `ECHOBOT_TTS_OPENAI_INSTRUCTIONS`    — instructions for the model.
//! * `ECHOBOT_TTS_OPENAI_EXTRA_BODY`     — JSON object merged into the
//!   request body.
//! * `ECHOBOT_TTS_EDGE_VOICE`             — Edge default voice (default:
//!   `zh-CN-XiaoxiaoNeural`).
//!
//! The Kokoro provider is *not* wired in `build_default_tts_service`.
//! It is only added when both the `kokoro` cargo feature is enabled
//! **and** a `KokoroTtsConfig` is passed in via the env-overridable
//! helper [`build_default_kokoro_provider`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::base::TtsProvider;
use crate::providers::edge::{EdgeTtsConfig, EdgeTtsProvider};
use crate::providers::openai_compatible::{
    OpenAICompatibleTtsConfig, OpenAICompatibleTtsProvider,
};
use crate::service::TtsService;

/// Default provider name when no env var is set.
pub const DEFAULT_TTS_PROVIDER: &str = "edge";

/// Build a service that registers only the Edge provider. Useful in
/// tests and minimal deployments.
pub fn build_minimal_tts_service() -> TtsService {
    let mut providers: BTreeMap<String, Arc<dyn TtsProvider>> = BTreeMap::new();
    providers.insert(
        "edge".to_string(),
        Arc::new(EdgeTtsProvider::new()),
    );
    TtsService::new(providers, DEFAULT_TTS_PROVIDER).expect("default service builds")
}

/// Build the default service. Reads env vars and registers Edge +
/// OpenAI-compatible (and Kokoro, when the cargo feature is enabled).
pub fn build_default_tts_service(workspace: Option<&Path>) -> TtsService {
    let _ = workspace; // currently only used when the `kokoro` feature is enabled.
    let mut providers: BTreeMap<String, Arc<dyn TtsProvider>> = BTreeMap::new();

    // --- Edge --------------------------------------------------------
    let edge_voice = env_text("ECHOBOT_TTS_EDGE_VOICE", "zh-CN-XiaoxiaoNeural");
    providers.insert(
        "edge".to_string(),
        Arc::new(EdgeTtsProvider::with_config(
            EdgeTtsConfig::new().with_default_voice(edge_voice),
        )),
    );

    // --- OpenAI-compatible -------------------------------------------
    providers.insert(
        "openai-compatible".to_string(),
        Arc::new(OpenAICompatibleTtsProvider::new(
            build_default_openai_compatible_config(),
        )),
    );

    // --- Kokoro (feature-gated) --------------------------------------
    #[cfg(feature = "kokoro")]
    {
        if let Some(ws) = workspace {
            let provider = build_default_kokoro_provider(ws);
            providers.insert("kokoro".to_string(), Arc::new(provider));
        }
    }

    let default = env_text("ECHOBOT_TTS_PROVIDER", DEFAULT_TTS_PROVIDER);
    // If the env var points at a provider that wasn't registered (e.g.
    // `kokoro` when the feature is off), fall back to the default
    // string. The service is happy with that as long as it exists.
    let default_provider = if providers.contains_key(&default) {
        default
    } else {
        DEFAULT_TTS_PROVIDER.to_string()
    };
    TtsService::new(providers, default_provider).expect("default service builds")
}

/// Build the openai-compatible config from env vars.
pub fn build_default_openai_compatible_config() -> OpenAICompatibleTtsConfig {
    OpenAICompatibleTtsConfig {
        api_key: env_text("ECHOBOT_TTS_OPENAI_API_KEY", "EMPTY"),
        model: env_text("ECHOBOT_TTS_OPENAI_MODEL", ""),
        base_url: env_text("ECHOBOT_TTS_OPENAI_BASE_URL", "https://api.openai.com/v1"),
        timeout_seconds: env_float("ECHOBOT_TTS_OPENAI_TIMEOUT", 60.0).max(1.0),
        default_voice: env_text(
            "ECHOBOT_TTS_OPENAI_DEFAULT_VOICE",
            OpenAICompatibleTtsProvider::default_voice_const(),
        ),
        response_format: env_text(
            "ECHOBOT_TTS_OPENAI_RESPONSE_FORMAT",
            OpenAICompatibleTtsProvider::default_response_format_const(),
        ),
        voices: env_csv("ECHOBOT_TTS_OPENAI_VOICES"),
        instructions: env_text("ECHOBOT_TTS_OPENAI_INSTRUCTIONS", ""),
        extra_body: env_json_object("ECHOBOT_TTS_OPENAI_EXTRA_BODY"),
    }
}

#[cfg(feature = "kokoro")]
fn build_default_kokoro_provider(workspace: &Path) -> crate::providers::kokoro::KokoroTtsProvider {
    use crate::providers::kokoro::{KokoroTtsConfig, KokoroTtsProvider};

    let voice = env_text("ECHOBOT_TTS_KOKORO_DEFAULT_VOICE", "zf_001");
    let config = KokoroTtsConfig::new().with_default_voice(voice);
    // `workspace` is preserved for phase 3 when we wire up model
    // download paths; we ignore the value today.
    let _ = workspace;
    KokoroTtsProvider::with_config(config)
}

// --- env helpers ------------------------------------------------------

fn env_text(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(v) => {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                default.to_string()
            } else {
                trimmed.to_string()
            }
        }
        Err(_) => default.to_string(),
    }
}

fn env_float(name: &str, default: f32) -> f32 {
    match std::env::var(name) {
        Ok(v) => v.trim().parse::<f32>().unwrap_or(default),
        Err(_) => default,
    }
}

fn env_csv(name: &str) -> Vec<String> {
    match std::env::var(name) {
        Ok(v) => v
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn env_json_object(name: &str) -> serde_json::Map<String, serde_json::Value> {
    match std::env::var(name) {
        Ok(v) => match serde_json::from_str::<serde_json::Value>(v.trim()) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        },
        Err(_) => serde_json::Map::new(),
    }
}

/// Resolve `raw_path` relative to `workspace` if it's not absolute.
pub fn resolve_optional_path(workspace: &Path, raw_path: &str) -> Option<PathBuf> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(trimmed);
    if candidate.is_absolute() {
        Some(candidate)
    } else {
        Some(workspace.join(candidate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_service_registers_edge() {
        let svc = build_minimal_tts_service();
        assert_eq!(svc.provider_names(), vec!["edge".to_string()]);
        assert_eq!(svc.default_provider_name(), "edge");
    }

    #[test]
    fn default_service_registers_at_least_edge_and_openai() {
        // Tests run in a clean env; we do *not* override env vars here.
        let svc = build_default_tts_service(None);
        let names = svc.provider_names();
        assert!(names.contains(&"edge".to_string()));
        assert!(names.contains(&"openai-compatible".to_string()));
    }

    #[test]
    fn resolve_optional_path_handles_relative_and_absolute() {
        let workspace = Path::new("/tmp/echobot");
        assert_eq!(
            resolve_optional_path(workspace, "models/tts"),
            Some(PathBuf::from("/tmp/echobot/models/tts"))
        );
        assert_eq!(
            resolve_optional_path(workspace, "/opt/models"),
            Some(PathBuf::from("/opt/models"))
        );
        assert_eq!(resolve_optional_path(workspace, ""), None);
        assert_eq!(resolve_optional_path(workspace, "  "), None);
    }
}
