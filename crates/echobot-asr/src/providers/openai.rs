//! OpenAI-compatible Transcriptions API provider.
//!
//! POSTs an audio file (re-encoded as 16-bit PCM WAV at the service's
//! sample rate) to `{base_url}/audio/transcriptions` using multipart form
//! encoding. The response JSON is parsed and surfaced as a
//! [`TranscriptionResult`].
//!
//! Mirrors the Python `OpenAITranscriptionsASRProvider`.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::Deserialize;

use crate::audio::write_wav_bytes;
use crate::base::{AsrError, AsrProvider, Result};
use crate::models::{AsrConfig, AsrResult, ProviderStatusSnapshot, TranscriptionResult};

/// Default `base_url` for the OpenAI service. Override with
/// `ECHOBOT_ASR_OPENAI_BASE_URL`.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Default request timeout, in seconds.
pub const DEFAULT_TIMEOUT_SECONDS: f64 = 60.0;
/// Multipart field name for the audio file (matches OpenAI's API).
pub const MULTIPART_FILE_FIELD: &str = "file";
/// Multipart field name for the model identifier.
pub const MULTIPART_MODEL_FIELD: &str = "model";

/// User-facing configuration for [`OpenAITranscriptionsProvider`].
#[derive(Debug, Clone)]
pub struct OpenAITranscriptionsConfig {
    /// Target sample rate for the audio we re-encode.
    pub sample_rate: u32,
    /// API key. `EMPTY` is accepted for local OpenAI-compatible servers.
    pub api_key: String,
    /// Model name (e.g. `"whisper-1"`, `"Systran/faster-distil-whisper-large-v3"`).
    pub model: String,
    /// Base URL (no trailing slash, no `/audio/transcriptions` suffix).
    pub base_url: String,
    /// Per-request HTTP timeout.
    pub timeout: Duration,
    /// Optional BCP-47 language hint.
    pub language: String,
    /// Optional initial prompt.
    pub prompt: String,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
}

impl Default for OpenAITranscriptionsConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            api_key: String::new(),
            model: String::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            timeout: Duration::from_secs_f64(DEFAULT_TIMEOUT_SECONDS),
            language: String::new(),
            prompt: String::new(),
            temperature: None,
        }
    }
}

/// OpenAI-compatible Transcriptions API provider.
pub struct OpenAITranscriptionsProvider {
    config: OpenAITranscriptionsConfig,
    client: Client,
}

impl OpenAITranscriptionsProvider {
    /// Build a provider with the given config.
    pub fn new(config: OpenAITranscriptionsConfig) -> Result<Self> {
        if config.sample_rate == 0 {
            return Err(AsrError::Config(
                "ASR sample_rate must be positive".to_string(),
            ));
        }
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| AsrError::Config(format!("reqwest client init failed: {e}")))?;
        Ok(Self { config, client })
    }

    /// Build the multipart form for a transcribe request.
    ///
    /// Exposed as `pub` for unit testing — the form layout is part of the
    /// public contract.
    pub fn build_form(
        config: &OpenAITranscriptionsConfig,
        wav_bytes: Vec<u8>,
    ) -> Result<Form> {
        let mut form = Form::new();
        let part = Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| AsrError::Multipart(format!("file part failed: {e}")))?;
        form = form.text(MULTIPART_MODEL_FIELD, config.model.clone());
        form = form.part(MULTIPART_FILE_FIELD, part);
        if !config.language.trim().is_empty() {
            form = form.text("language", config.language.clone());
        }
        if !config.prompt.trim().is_empty() {
            form = form.text("prompt", config.prompt.clone());
        }
        if let Some(temperature) = config.temperature {
            form = form.text("temperature", temperature.to_string());
        }
        Ok(form)
    }

    /// Parse the JSON response body into a [`TranscriptionResult`].
    pub fn parse_response(body: &str) -> TranscriptionResult {
        // Try structured JSON first.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            let text = value
                .get("text")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            let language = value
                .get("language")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or_default()
                .to_string();

            let segments_text = if text.is_empty() {
                extract_segments_text(&value)
            } else {
                String::new()
            };

            return TranscriptionResult {
                text: if text.is_empty() { segments_text } else { text },
                language,
            };
        }
        // Fallback: treat the body as plain text.
        TranscriptionResult {
            text: body.trim().to_string(),
            language: String::new(),
        }
    }

    /// Returns `(state, detail)` for the provider status snapshot.
    pub fn status_state(&self) -> (&'static str, String) {
        if self.config.base_url.trim().is_empty() {
            return (
                "missing",
                "OpenAI transcriptions provider is missing base_url".to_string(),
            );
        }
        if self.config.model.trim().is_empty() {
            return (
                "missing",
                "OpenAI transcriptions provider is missing model".to_string(),
            );
        }
        if self.uses_official_openai_endpoint()
            && self.config.api_key.trim().to_ascii_uppercase().is_empty()
        {
            return (
                "missing",
                "OpenAI official transcription endpoint requires an API key".to_string(),
            );
        }
        (
            "ready",
            format!(
                "OpenAI-compatible ASR ready: model={}, base_url={}",
                self.config.model, self.config.base_url
            ),
        )
    }

    /// True if the configured base URL points at the official OpenAI service.
    pub fn uses_official_openai_endpoint(&self) -> bool {
        extract_host(&self.config.base_url)
            .map(|host| host.eq_ignore_ascii_case("api.openai.com"))
            .unwrap_or(false)
    }

    /// Build the full transcriptions endpoint URL.
    pub fn transcriptions_url(&self) -> String {
        let trimmed = self.config.base_url.trim_end_matches('/');
        format!("{trimmed}/audio/transcriptions")
    }
}

fn extract_segments_text(value: &serde_json::Value) -> String {
    let Some(segments) = value.get("segments").and_then(|v| v.as_array()) else {
        return String::new();
    };
    let mut out = String::new();
    for segment in segments {
        if let Some(text) = segment.get("text").and_then(|v| v.as_str()) {
            out.push_str(text);
        }
    }
    out.trim().to_string()
}

/// Extract the host portion of an HTTP(S) URL. Returns `None` for any URL
/// we can't parse — we never fail the status check just because the URL
/// is unusual; we just can't recognize the host.
fn extract_host(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))?;
    let host_end = rest
        .find(['/', '?', '#', ':'])
        .unwrap_or(rest.len());
    Some(rest[..host_end].to_ascii_lowercase())
}

#[async_trait]
impl AsrProvider for OpenAITranscriptionsProvider {
    fn name(&self) -> &str {
        "openai-transcriptions"
    }

    fn label(&self) -> &str {
        "OpenAI Transcriptions"
    }

    async fn status_snapshot(&self) -> Result<ProviderStatusSnapshot> {
        let (state, detail) = self.status_state();
        Ok(ProviderStatusSnapshot {
            kind: "asr".to_string(),
            name: self.name().to_string(),
            label: self.label().to_string(),
            selected: false,
            available: state == "ready",
            state: state.to_string(),
            detail,
            resource_directory: self.config.base_url.clone(),
        })
    }

    async fn transcribe_samples(&self, samples: &[f32]) -> Result<TranscriptionResult> {
        let (state, detail) = self.status_state();
        if state != "ready" {
            return Err(AsrError::Config(detail));
        }
        if samples.is_empty() {
            return Ok(TranscriptionResult::default());
        }
        let wav = write_wav_bytes(samples, self.config.sample_rate)?;
        let form = Self::build_form(&self.config, wav)?;
        let request = self
            .client
            .post(self.transcriptions_url())
            .header(
                "Authorization",
                format!("Bearer {}", self.config.api_key.trim()),
            )
            .multipart(form);
        let response = request.send().await?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(AsrError::HttpStatus {
                status: status.as_u16(),
                detail,
            });
        }

        // Helper struct for the canonical OpenAI response.
        #[derive(Deserialize)]
        struct ResponseShape {
            text: Option<String>,
            language: Option<String>,
        }
        let body = response.text().await.map_err(AsrError::from)?;
        if let Ok(parsed) = serde_json::from_str::<ResponseShape>(&body) {
            return Ok(TranscriptionResult {
                text: parsed.text.unwrap_or_default().trim().to_string(),
                language: parsed.language.unwrap_or_default().trim().to_string(),
            });
        }
        Ok(Self::parse_response(&body))
    }

    async fn transcribe_with_config(
        &self,
        samples: &[f32],
        config: &AsrConfig,
    ) -> Result<AsrResult> {
        // We currently ignore `config` because the OpenAI provider already
        // stores its settings in `self.config`. We still consult the per-
        // call config so callers can override language / temperature on a
        // per-request basis.
        let effective = OpenAITranscriptionsConfig {
            language: if !config.language.is_empty() {
                config.language.clone()
            } else {
                self.config.language.clone()
            },
            prompt: if !config.prompt.is_empty() {
                config.prompt.clone()
            } else {
                self.config.prompt.clone()
            },
            temperature: config.temperature.or(self.config.temperature),
            ..self.config.clone()
        };
        let provider = OpenAITranscriptionsProvider {
            config: effective,
            client: self.client.clone(),
        };
        let result = provider.transcribe_samples(samples).await?;
        Ok(AsrResult::from_transcription(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> OpenAITranscriptionsConfig {
        OpenAITranscriptionsConfig {
            sample_rate: 16_000,
            api_key: "test-key".to_string(),
            model: "whisper-1".to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            timeout: Duration::from_secs(5),
            language: "en".to_string(),
            prompt: "Greetings".to_string(),
            temperature: Some(0.2),
        }
    }

    #[test]
    fn multipart_request_builder_is_well_formed() {
        let config = make_config();
        // Build a tiny in-memory WAV blob.
        let samples: Vec<f32> = (0..160).map(|i| (i as f32 / 160.0) - 0.5).collect();
        let wav = write_wav_bytes(&samples, 16_000).expect("encode wav");
        let form = OpenAITranscriptionsProvider::build_form(&config, wav.clone())
            .expect("form should build");

        // Inspect the form's metadata by re-encoding it as multipart bytes.
        let body = form_into_bytes(form);
        let as_string = String::from_utf8_lossy(&body).into_owned();

        // Field names should be present.
        assert!(as_string.contains("name=\"model\""), "missing model field");
        assert!(as_string.contains("name=\"file\""), "missing file field");
        assert!(
            as_string.contains("name=\"language\""),
            "missing language field"
        );
        assert!(
            as_string.contains("name=\"prompt\""),
            "missing prompt field"
        );
        assert!(
            as_string.contains("name=\"temperature\""),
            "missing temperature field"
        );

        // Values should match what we configured.
        assert!(as_string.contains("whisper-1"));
        assert!(as_string.contains("en"));
        assert!(as_string.contains("Greetings"));
        assert!(as_string.contains("0.2"));

        // The audio bytes should be embedded.
        assert!(as_string.contains("audio/wav"));
        // RIFF header preserved.
        assert!(as_string.contains("RIFF"));
    }

    #[test]
    fn multipart_form_omits_optional_fields_when_blank() {
        let mut config = make_config();
        config.language.clear();
        config.prompt.clear();
        config.temperature = None;
        let samples: Vec<f32> = vec![0.0; 16];
        let wav = write_wav_bytes(&samples, 16_000).expect("encode");
        let form = OpenAITranscriptionsProvider::build_form(&config, wav).expect("form");
        let body = form_into_bytes(form);
        let as_string = String::from_utf8_lossy(&body).into_owned();
        assert!(as_string.contains("name=\"model\""));
        assert!(as_string.contains("name=\"file\""));
        assert!(!as_string.contains("name=\"language\""));
        assert!(!as_string.contains("name=\"prompt\""));
        assert!(!as_string.contains("name=\"temperature\""));
    }

    #[test]
    fn status_state_marks_missing_config() {
        let mut config = make_config();
        config.model.clear();
        let provider = OpenAITranscriptionsProvider::new(config).expect("build");
        let (state, detail) = provider.status_state();
        assert_eq!(state, "missing");
        assert!(detail.contains("model"));
    }

    #[test]
    fn status_state_requires_api_key_for_official_endpoint() {
        let mut config = make_config();
        config.api_key.clear();
        let provider = OpenAITranscriptionsProvider::new(config).expect("build");
        let (state, detail) = provider.status_state();
        assert_eq!(state, "missing");
        assert!(detail.contains("API key") || detail.contains("api key"));
    }

    #[test]
    fn status_state_ready_for_local_endpoint() {
        let mut config = make_config();
        config.api_key.clear();
        config.base_url = "http://localhost:8000/v1".to_string();
        let provider = OpenAITranscriptionsProvider::new(config).expect("build");
        let (state, _detail) = provider.status_state();
        assert_eq!(state, "ready");
    }

    #[test]
    fn transcriptions_url_appends_suffix() {
        let provider = OpenAITranscriptionsProvider::new(make_config()).expect("build");
        assert_eq!(
            provider.transcriptions_url(),
            "https://api.openai.com/v1/audio/transcriptions"
        );

        let mut cfg = make_config();
        cfg.base_url = "http://localhost:8000/v1/".to_string();
        let provider = OpenAITranscriptionsProvider::new(cfg).expect("build");
        assert_eq!(
            provider.transcriptions_url(),
            "http://localhost:8000/v1/audio/transcriptions"
        );
    }

    #[test]
    fn parse_response_handles_text_and_segments() {
        let text_only = r#"{"text": "hello world"}"#;
        let parsed = OpenAITranscriptionsProvider::parse_response(text_only);
        assert_eq!(parsed.text, "hello world");
        assert_eq!(parsed.language, "");

        let with_lang = r#"{"text": "  bonjour  ", "language": "fr"}"#;
        let parsed = OpenAITranscriptionsProvider::parse_response(with_lang);
        assert_eq!(parsed.text, "bonjour");
        assert_eq!(parsed.language, "fr");

        let segments_only = r#"{"segments": [{"text": "foo "}, {"text": "bar"}]}"#;
        let parsed = OpenAITranscriptionsProvider::parse_response(segments_only);
        assert_eq!(parsed.text, "foo bar");

        let plain = "just text";
        let parsed = OpenAITranscriptionsProvider::parse_response(plain);
        assert_eq!(parsed.text, "just text");
    }

    #[test]
    fn pcm16le_helper_is_consistent() {
        use crate::audio::pcm16le_bytes_to_floats;
        // Smoke check that the helper used by the realtime path matches the
        // canonical f32 round trip. Skip `i16::MIN` (the round-trip in
        // f32 isn't quite symmetric at the boundary).
        let raw: Vec<i16> = vec![0, 16_384, -16_384, i16::MAX, -i16::MAX];
        let mut bytes = Vec::new();
        for s in &raw {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let floats = pcm16le_bytes_to_floats(&bytes);
        assert_eq!(floats.len(), raw.len());
        for (orig, back) in raw.iter().zip(floats.iter()) {
            let expected = *orig as f32 / 32_768.0;
            let back_as_i16 = (back * 32_768.0).round() as i16;
            let orig_abs = orig.unsigned_abs();
            let back_abs = back_as_i16.unsigned_abs();
            let delta = orig_abs.max(back_abs) - orig_abs.min(back_abs);
            assert!(delta <= 1, "orig={orig} back={back_as_i16}");
            // Allow minor rounding.
            assert!((expected - back).abs() < 1e-3);
        }
    }

    // Helper that re-encodes a multipart Form into its byte representation.
    // We can't directly read `Form`'s fields, but the serialized body is
    // stable and contains the `name="..."` headers we want to assert on.
    fn form_into_bytes(form: Form) -> Vec<u8> {
        use futures::executor::block_on;
        use futures::StreamExt;
        block_on(async move {
            let mut out = Vec::new();
            let mut stream = form.into_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.expect("multipart chunk");
                out.extend_from_slice(&chunk);
            }
            out
        })
    }
}
