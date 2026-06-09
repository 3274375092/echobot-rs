//! `echobot-tts::providers::kokoro` — STUB provider.
//!
//! Phase 1 keeps the API surface but every `synthesize` call returns
//! `TtsError::NotImplemented`. Phase 3 will wire up `sherpa-rs` (or a
//! direct ONNX runtime) and download the Kokoro model on first use.
//!
//! This module is gated behind the `kokoro` cargo feature so a default
//! build does not pull in extra dependencies.

use async_trait::async_trait;

use crate::base::{
    TtsAudio, TtsError, TtsProvider, TtsProviderStatus, TtsSynthesisOptions, VoiceOption,
};

/// Default voice for the Kokoro provider (matches the Python port's
/// default of `zf_001`).
pub const DEFAULT_KOKORO_VOICE: &str = "zf_001";

/// Configuration for [`KokoroTtsProvider`]. Kept intentionally minimal
/// in v1: we only need it to construct the stub. The real config (model
/// directory, thread count, etc.) lands in phase 3.
#[derive(Debug, Clone)]
pub struct KokoroTtsConfig {
    pub default_voice: String,
}

impl Default for KokoroTtsConfig {
    fn default() -> Self {
        Self {
            default_voice: DEFAULT_KOKORO_VOICE.to_string(),
        }
    }
}

impl KokoroTtsConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_voice(mut self, voice: impl Into<String>) -> Self {
        self.default_voice = voice.into();
        self
    }
}

/// Stub provider. All synthesis calls return `TtsError::NotImplemented`.
pub struct KokoroTtsProvider {
    config: KokoroTtsConfig,
}

impl KokoroTtsProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: KokoroTtsConfig) -> Self {
        Self { config }
    }
}

impl Default for KokoroTtsProvider {
    fn default() -> Self {
        Self {
            config: KokoroTtsConfig::default(),
        }
    }
}

#[async_trait]
impl TtsProvider for KokoroTtsProvider {
    fn name(&self) -> &str {
        "kokoro"
    }

    fn label(&self) -> &str {
        "Sherpa Kokoro (stub)"
    }

    fn default_voice(&self) -> &str {
        &self.config.default_voice
    }

    fn status(&self) -> TtsProviderStatus {
        TtsProviderStatus {
            name: self.name().to_string(),
            label: self.label().to_string(),
            available: false,
            state: "unavailable".to_string(),
            detail:
                "kokoro TTS is not implemented in v1 \u{2014} wire sherpa-rs or download onnx model in phase 3"
                    .to_string(),
        }
    }

    async fn list_voices(&self) -> Result<Vec<VoiceOption>, TtsError> {
        // Even the stub can advertise its default voice.
        Ok(vec![VoiceOption::from_name(&self.config.default_voice)])
    }

    async fn synthesize(
        &self,
        _text: &str,
        _options: Option<&TtsSynthesisOptions>,
    ) -> Result<TtsAudio, TtsError> {
        Err(TtsError::NotImplemented(
            "kokoro TTS is not implemented in v1 \u{2014} wire sherpa-rs or download onnx model in phase 3"
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn synthesize_returns_not_implemented() {
        let provider = KokoroTtsProvider::new();
        let err = provider
            .synthesize("hello", None)
            .await
            .expect_err("stub should not synthesize");
        match err {
            TtsError::NotImplemented(msg) => assert!(msg.contains("phase 3")),
            other => panic!("expected NotImplemented, got {other:?}"),
        }
    }

    #[test]
    fn status_reports_unavailable() {
        let provider = KokoroTtsProvider::new();
        let s = provider.status();
        assert!(!s.available);
        assert_eq!(s.state, "unavailable");
        assert!(s.detail.contains("phase 3"));
    }

    #[test]
    fn default_voice_matches_python_port() {
        assert_eq!(KokoroTtsProvider::new().default_voice(), DEFAULT_KOKORO_VOICE);
    }
}
