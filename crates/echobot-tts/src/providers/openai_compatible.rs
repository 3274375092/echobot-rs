//! `echobot-tts::providers::openai_compatible` — TTS via any
//! OpenAI-compatible `/audio/speech` HTTP endpoint.
//!
//! Mirrors the Python implementation in
//! `echobot/tts/providers/openai_compatible.py`. We use `reqwest` to POST
//! `{base_url}/audio/speech` with a JSON body and read the raw audio
//! bytes back.
//!
//! The provider is configured with:
//! * `api_key`        — Bearer token. `"EMPTY"` is allowed for local
//!                      servers (e.g. llama.cpp, vllm).
//! * `base_url`       — e.g. `"https://api.openai.com/v1"`.
//! * `model`          — required.
//! * `response_format` — one of `mp3 | opus | aac | flac | wav | pcm`.
//!                       Defaults to `"wav"` (matches the Python port).
//! * `default_voice`  — voice short name. Defaults to `"alloy"`.
//! * `instructions`   — optional system-style instruction for the model.
//! * `extra_body`     — extra JSON keys merged into the request body.
//! * `timeout_seconds` — request timeout, defaults to 60s.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

use crate::base::{
    TtsAudio, TtsError, TtsProvider, TtsProviderStatus, TtsSynthesisOptions, VoiceOption,
};

/// Default voice for the openai-compatible provider.
pub const DEFAULT_OPENAI_COMPATIBLE_TTS_VOICE: &str = "alloy";

/// Default response format. Matches the Python port.
pub const DEFAULT_OPENAI_COMPATIBLE_TTS_RESPONSE_FORMAT: &str = "wav";

/// Official OpenAI voice short names, used as a fallback when the
/// provider doesn't ship a `/audio/voices` endpoint.
pub const OFFICIAL_OPENAI_TTS_VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "coral", "echo", "fable", "onyx", "nova", "sage", "shimmer",
    "verse", "marin", "cedar",
];

const SUPPORTED_RESPONSE_FORMATS: &[&str] = &["mp3", "opus", "aac", "flac", "wav", "pcm"];

fn normalize_response_format(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if SUPPORTED_RESPONSE_FORMATS.contains(&normalized.as_str()) {
        normalized
    } else {
        DEFAULT_OPENAI_COMPATIBLE_TTS_RESPONSE_FORMAT.to_string()
    }
}

fn is_official_openai_endpoint(base_url: &str) -> bool {
    // Cheap, allocation-free: we only need the host portion.
    let lower = base_url.to_ascii_lowercase();
    lower.contains("api.openai.com")
}

/// Configuration for [`OpenAICompatibleTtsProvider`].
#[derive(Debug, Clone)]
pub struct OpenAICompatibleTtsConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub timeout_seconds: f32,
    pub default_voice: String,
    pub response_format: String,
    pub voices: Vec<String>,
    pub instructions: String,
    pub extra_body: serde_json::Map<String, Value>,
}

impl OpenAICompatibleTtsConfig {
    /// Build a config with sensible defaults and a single `api_key`.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            ..Self::default()
        }
    }

    /// Override the base URL (defaults to OpenAI's official endpoint).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Override the request timeout (seconds).
    pub fn with_timeout(mut self, seconds: f32) -> Self {
        self.timeout_seconds = seconds.max(1.0);
        self
    }

    /// Override the default voice.
    pub fn with_default_voice(mut self, voice: impl Into<String>) -> Self {
        self.default_voice = voice.into();
        self
    }

    /// Override the response format (`mp3 | opus | aac | flac | wav | pcm`).
    pub fn with_response_format(mut self, format: impl Into<String>) -> Self {
        self.response_format = format.into();
        self
    }

    /// Provide an explicit list of voice names. When non-empty, these are
    /// returned by `list_voices` without hitting the network.
    pub fn with_voices<I, S>(mut self, voices: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.voices = voices.into_iter().map(Into::into).collect();
        self
    }

    /// Provide an `instructions` string for the model.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = instructions.into();
        self
    }

    /// Merge extra keys into the JSON request body.
    pub fn with_extra_body(mut self, body: serde_json::Map<String, Value>) -> Self {
        self.extra_body = body;
        self
    }
}

impl Default for OpenAICompatibleTtsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: String::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            timeout_seconds: 60.0,
            default_voice: DEFAULT_OPENAI_COMPATIBLE_TTS_VOICE.to_string(),
            response_format: DEFAULT_OPENAI_COMPATIBLE_TTS_RESPONSE_FORMAT.to_string(),
            voices: Vec::new(),
            instructions: String::new(),
            extra_body: serde_json::Map::new(),
        }
    }
}

/// Shared HTTP client (one per provider). Held behind an `OnceCell` so
/// the provider can be cloned cheaply and the client is created lazily
/// on first use — this keeps construction infallible.
struct SharedClient {
    client: OnceCell<Arc<reqwest::Client>>,
    base_url: String,
    api_key: String,
    timeout: Duration,
}

impl SharedClient {
    fn new(base_url: String, api_key: String, timeout: Duration) -> Self {
        Self {
            client: OnceCell::new(),
            base_url,
            api_key,
            timeout,
        }
    }

    async fn get(&self) -> Result<Arc<reqwest::Client>, TtsError> {
        self.client
            .get_or_try_init(|| async {
                reqwest::Client::builder()
                    .timeout(self.timeout)
                    .build()
                    .map(Arc::new)
                    .map_err(|e| TtsError::network(format!("failed to build HTTP client: {e}")))
            })
            .await
            .cloned()
    }

    fn auth_header(&self) -> String {
        if self.api_key.is_empty() {
            "Bearer EMPTY".to_string()
        } else {
            format!("Bearer {}", self.api_key)
        }
    }
}

/// OpenAI-compatible TTS provider.
pub struct OpenAICompatibleTtsProvider {
    config: OpenAICompatibleTtsConfig,
    client: SharedClient,
}

impl OpenAICompatibleTtsProvider {
    /// Default voice constant (used by the factory and by callers that
    /// want a compile-time reference to the default).
    pub fn default_voice_const() -> &'static str {
        DEFAULT_OPENAI_COMPATIBLE_TTS_VOICE
    }

    /// Default response format constant.
    pub fn default_response_format_const() -> &'static str {
        DEFAULT_OPENAI_COMPATIBLE_TTS_RESPONSE_FORMAT
    }

    /// Construct a provider from a config struct.
    pub fn new(config: OpenAICompatibleTtsConfig) -> Self {
        let api_key = config.api_key.trim().to_string();
        let base_url = config.base_url.trim().to_string();
        let timeout = Duration::from_secs_f32(config.timeout_seconds.max(1.0));
        Self {
            config: OpenAICompatibleTtsConfig {
                api_key: api_key.clone(),
                base_url: base_url.clone(),
                default_voice: {
                    let v = config.default_voice.trim();
                    if v.is_empty() {
                        DEFAULT_OPENAI_COMPATIBLE_TTS_VOICE.to_string()
                    } else {
                        v.to_string()
                    }
                },
                response_format: normalize_response_format(&config.response_format),
                ..config
            },
            client: SharedClient::new(base_url, api_key, timeout),
        }
    }

    /// Build the JSON body for the `/audio/speech` request. Exposed for
    /// testing.
    pub fn build_request_body(
        &self,
        text: &str,
        voice: &str,
        options: Option<&TtsSynthesisOptions>,
    ) -> Value {
        build_request_body(&self.config, text, voice, options)
    }

    /// Build the full request URL.
    pub fn build_request_url(&self) -> String {
        format!(
            "{}/audio/speech",
            self.client.base_url.trim_end_matches('/')
        )
    }

    /// Provider status, with availability reasoning.
    pub fn status(&self) -> TtsProviderStatus {
        let (state, detail) = self.compute_status();
        TtsProviderStatus {
            name: self.name().to_string(),
            label: self.label().to_string(),
            available: state == "ready",
            state: state.to_string(),
            detail: if state == "ready" {
                String::new()
            } else {
                detail.to_string()
            },
        }
    }

    fn compute_status(&self) -> (&'static str, &'static str) {
        if self.client.base_url.is_empty() {
            return (
                "missing",
                "OpenAI-compatible TTS provider is missing ECHOBOT_TTS_OPENAI_BASE_URL.",
            );
        }
        if self.config.model.is_empty() {
            return (
                "missing",
                "OpenAI-compatible TTS provider is missing ECHOBOT_TTS_OPENAI_MODEL.",
            );
        }
        if is_official_openai_endpoint(&self.client.base_url)
            && self.client.api_key.is_empty()
        {
            return (
                "missing",
                "OpenAI official TTS endpoint requires ECHOBOT_TTS_OPENAI_API_KEY.",
            );
        }
        ("ready", "")
    }

    async fn fetch_voice_options(&self) -> Result<Vec<VoiceOption>, TtsError> {
        let url = format!(
            "{}/audio/voices",
            self.client.base_url.trim_end_matches('/')
        );
        let client = self.client.get().await?;
        let response = client
            .get(&url)
            .header("Authorization", self.client.auth_header())
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| TtsError::network(format!("voices request failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(TtsError::provider(format!(
                "voices endpoint returned status {status}"
            )));
        }
        let payload: Value = response
            .json()
            .await
            .map_err(|e| TtsError::invalid_response(format!("voices JSON parse failed: {e}")))?;
        Ok(voice_options_from_payload(&payload, &self.config.default_voice))
    }

    fn fallback_voice_options(&self) -> Vec<VoiceOption> {
        if is_official_openai_endpoint(&self.client.base_url) {
            voice_options_from_names(
                OFFICIAL_OPENAI_TTS_VOICES.iter().copied(),
                &self.config.default_voice,
            )
        } else {
            voice_options_from_names(
                std::iter::once(self.config.default_voice.as_str()),
                &self.config.default_voice,
            )
        }
    }
}

#[async_trait]
impl TtsProvider for OpenAICompatibleTtsProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn label(&self) -> &str {
        "OpenAI-Compatible TTS"
    }

    fn default_voice(&self) -> &str {
        &self.config.default_voice
    }

    fn status(&self) -> TtsProviderStatus {
        // Defer to inherent method so the same logic is reusable.
        OpenAICompatibleTtsProvider::status(self)
    }

    async fn list_voices(&self) -> Result<Vec<VoiceOption>, TtsError> {
        let (state, detail) = self.compute_status();
        if state != "ready" {
            return Err(TtsError::config(detail));
        }
        if !self.config.voices.is_empty() {
            return Ok(voice_options_from_names(
                self.config.voices.iter().map(|s| s.as_str()),
                &self.config.default_voice,
            ));
        }
        match self.fetch_voice_options().await {
            Ok(v) if !v.is_empty() => Ok(v),
            _ => Ok(self.fallback_voice_options()),
        }
    }

    async fn synthesize(
        &self,
        text: &str,
        options: Option<&TtsSynthesisOptions>,
    ) -> Result<TtsAudio, TtsError> {
        let (state, detail) = self.compute_status();
        if state != "ready" {
            return Err(TtsError::config(detail));
        }

        let voice = options
            .and_then(|o| o.voice.clone())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| self.config.default_voice.clone());

        let body = self.build_request_body(text, &voice, options);
        let url = self.build_request_url();
        let client = self.client.get().await?;
        let response = client
            .post(&url)
            .header("Authorization", self.client.auth_header())
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| TtsError::network(format!("TTS request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            // Best-effort error body.
            let body = response.text().await.unwrap_or_default();
            return Err(TtsError::provider(format!(
                "TTS request failed: status={}, body={}",
                status, body
            )));
        }

        // The provider's content-type tells us the format. Fall back to
        // our configured default.
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| TtsError::network(format!("TTS response read failed: {e}")))?;

        let (mime, ext) = mime_and_ext_for(&content_type, &self.config.response_format);
        Ok(TtsAudio {
            audio_bytes: bytes.to_vec(),
            content_type: mime.to_string(),
            file_extension: ext.to_string(),
            provider: self.name().to_string(),
            voice,
        })
    }
}

impl TtsError {
    /// Helper to construct an "invalid response" error from a value.
    pub fn invalid_response<S: Into<String>>(msg: S) -> Self {
        Self::InvalidResponse(msg.into())
    }
}

/// Build the JSON request body for `/audio/speech`. Exposed for unit
/// tests.
pub fn build_request_body(
    config: &OpenAICompatibleTtsConfig,
    text: &str,
    voice: &str,
    options: Option<&TtsSynthesisOptions>,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("input".to_string(), Value::String(text.to_string()));
    body.insert("model".to_string(), Value::String(config.model.clone()));
    body.insert("voice".to_string(), Value::String(voice.to_string()));

    if !config.instructions.trim().is_empty() {
        body.insert(
            "instructions".to_string(),
            Value::String(config.instructions.clone()),
        );
    }
    if !config.response_format.is_empty() {
        body.insert(
            "response_format".to_string(),
            Value::String(config.response_format.clone()),
        );
    }
    if let Some(speed) = options.and_then(|o| o.speed) {
        body.insert("speed".to_string(), json!(speed));
    }
    // Merge extra_body last so explicit caller keys win.
    for (k, v) in &config.extra_body {
        body.insert(k.clone(), v.clone());
    }

    Value::Object(body)
}

/// Map a content-type to `(mime, file_extension)`. Falls back to the
/// supplied default format when the content-type is unknown.
fn mime_and_ext_for(content_type: &str, default_format: &str) -> (&'static str, &'static str) {
    let normalized = content_type.to_ascii_lowercase();
    let direct = match normalized.as_str() {
        "audio/mpeg" | "audio/mp3" => Some(("audio/mpeg", "mp3")),
        "audio/opus" => Some(("audio/opus", "opus")),
        "audio/aac" => Some(("audio/aac", "aac")),
        "audio/flac" => Some(("audio/flac", "flac")),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some(("audio/wav", "wav")),
        "audio/pcm" => Some(("audio/pcm", "pcm")),
        _ => None,
    };
    if let Some(hit) = direct {
        return hit;
    }

    let (mime, ext) = match default_format {
        "mp3" => ("audio/mpeg", "mp3"),
        "opus" => ("audio/opus", "opus"),
        "aac" => ("audio/aac", "aac"),
        "flac" => ("audio/flac", "flac"),
        "pcm" => ("audio/pcm", "pcm"),
        // wav is the catch-all default that matches the Python port.
        _ => ("audio/wav", "wav"),
    };
    (mime, ext)
}

fn voice_options_from_names<'a, I>(names: I, default_voice: &str) -> Vec<VoiceOption>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<VoiceOption> = Vec::new();
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !seen.insert(trimmed.to_string()) {
            continue;
        }
        out.push(VoiceOption::from_name(trimmed));
    }
    if out.is_empty() {
        out.push(VoiceOption::from_name(default_voice));
    }
    // Sort: default first, then alphabetical.
    out.sort_by(|a, b| {
        let a_is_default = a.short_name == default_voice;
        let b_is_default = b.short_name == default_voice;
        a_is_default
            .cmp(&b_is_default)
            .reverse()
            .then_with(|| a.short_name.cmp(&b.short_name))
    });
    out
}

fn voice_options_from_payload(payload: &Value, default_voice: &str) -> Vec<VoiceOption> {
    let arr = match payload.get("voices").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => match payload.as_array() {
            Some(a) => a,
            None => return Vec::new(),
        },
    };
    let names: Vec<String> = arr
        .iter()
        .filter_map(|item| {
            item.as_str()
                .map(|s| s.to_string())
                .or_else(|| {
                    item.get("short_name")
                        .or_else(|| item.get("name"))
                        .or_else(|| item.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
        })
        .collect();
    let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    voice_options_from_names(name_refs, default_voice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> OpenAICompatibleTtsConfig {
        OpenAICompatibleTtsConfig::new("EMPTY", "tts-1")
    }

    #[test]
    fn build_request_body_minimum_fields() {
        let provider = OpenAICompatibleTtsProvider::new(base_config());
        let body = provider.build_request_body("hello", "alloy", None);
        assert_eq!(body["input"], "hello");
        assert_eq!(body["model"], "tts-1");
        assert_eq!(body["voice"], "alloy");
        // `response_format` ships with a default ("wav") per the Python
        // port; speed only included when present.
        assert_eq!(body["response_format"], "wav");
        assert!(body.get("speed").is_none());
    }

    #[test]
    fn build_request_body_with_options_and_instructions() {
        let cfg = base_config()
            .with_response_format("mp3")
            .with_instructions("speak warmly");
        let provider = OpenAICompatibleTtsProvider::new(cfg);
        let opts = TtsSynthesisOptions::with_voice("nova");
        let body = provider.build_request_body("hi", "nova", Some(&opts));
        assert_eq!(body["voice"], "nova");
        assert_eq!(body["response_format"], "mp3");
        assert_eq!(body["instructions"], "speak warmly");
    }

    #[test]
    fn build_request_body_includes_speed() {
        let provider = OpenAICompatibleTtsProvider::new(base_config());
        let opts = TtsSynthesisOptions {
            voice: None,
            speed: Some(1.5),
            volume: None,
            pitch: None,
        };
        let body = provider.build_request_body("hi", "alloy", Some(&opts));
        assert_eq!(body["speed"], 1.5);
    }

    #[test]
    fn build_request_body_merges_extra_body() {
        let mut extra = serde_json::Map::new();
        extra.insert("temperature".to_string(), json!(0.7));
        let cfg = base_config().with_extra_body(extra);
        let provider = OpenAICompatibleTtsProvider::new(cfg);
        let body = provider.build_request_body("hi", "alloy", None);
        assert_eq!(body["temperature"], 0.7);
    }

    #[test]
    fn request_url_strips_trailing_slash() {
        let cfg = base_config().with_base_url("https://api.example.com/v1/");
        let provider = OpenAICompatibleTtsProvider::new(cfg);
        assert_eq!(
            provider.build_request_url(),
            "https://api.example.com/v1/audio/speech"
        );
    }

    #[test]
    fn mime_and_ext_known_content_types() {
        assert_eq!(mime_and_ext_for("audio/mpeg", "wav"), ("audio/mpeg", "mp3"));
        assert_eq!(mime_and_ext_for("audio/wav", "wav"), ("audio/wav", "wav"));
        assert_eq!(mime_and_ext_for("audio/opus", "wav"), ("audio/opus", "opus"));
    }

    #[test]
    fn mime_and_ext_falls_back_to_default_format() {
        assert_eq!(mime_and_ext_for("application/octet-stream", "mp3"), ("audio/mpeg", "mp3"));
    }

    #[test]
    fn status_reports_missing_model() {
        let mut cfg = base_config();
        cfg.model = String::new();
        let provider = OpenAICompatibleTtsProvider::new(cfg);
        let s = provider.status();
        assert!(!s.available);
        assert_eq!(s.state, "missing");
        assert!(s.detail.contains("MODEL"));
    }

    #[test]
    fn fallback_voices_uses_default_for_unknown_endpoint() {
        let cfg = base_config().with_base_url("https://llama.local/v1");
        let provider = OpenAICompatibleTtsProvider::new(cfg);
        let voices = provider.fallback_voice_options();
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].short_name, "alloy");
    }

    #[test]
    fn fallback_voices_uses_official_list_for_openai() {
        let cfg = base_config().with_base_url("https://api.openai.com/v1");
        let provider = OpenAICompatibleTtsProvider::new(cfg);
        let voices = provider.fallback_voice_options();
        assert!(voices.iter().any(|v| v.short_name == "alloy"));
    }
}
