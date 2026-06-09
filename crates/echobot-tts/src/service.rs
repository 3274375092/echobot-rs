//! `echobot-tts::service` — the [`TtsService`] facade.
//!
//! `TtsService` owns a set of providers keyed by name, plus a default.
//! It centralizes the cross-cutting concerns every caller needs:
//!
//! * text normalization (via [`crate::text::normalize_text_for_tts`]);
//! * synthesis-option building (via [`crate::synthesis`]);
//! * dispatch to the right provider;
//! * lifecycle management (close all providers cleanly).
//!
//! Mirrors the Python `TTSService` in `echobot/tts/service.py` 1:1.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::base::{
    TtsAudio, TtsError, TtsProvider, TtsProviderStatus, TtsSynthesisOptions, VoiceOption,
};
use crate::synthesis::build_tts_synthesis_options;
use crate::text::normalize_text_for_tts;

/// The default provider name, used when a caller does not pick one.
pub const DEFAULT_PROVIDER_NAME: &str = "edge";

/// Multi-provider TTS facade. Cheap to clone (it's an `Arc` inside-out).
#[derive(Clone)]
pub struct TtsService {
    providers: Arc<BTreeMap<String, Arc<dyn TtsProvider>>>,
    default_provider: String,
}

impl TtsService {
    /// Build a service from a name -> provider map and a default.
    /// Returns `TtsError::InvalidConfig` when the map is empty or the
    /// default is not present.
    pub fn new(
        providers: BTreeMap<String, Arc<dyn TtsProvider>>,
        default_provider: impl Into<String>,
    ) -> Result<Self, TtsError> {
        if providers.is_empty() {
            return Err(TtsError::config("at least one TTS provider is required"));
        }
        let default_provider = default_provider.into();
        if !providers.contains_key(&default_provider) {
            return Err(TtsError::config(format!(
                "unknown default TTS provider: {default_provider}"
            )));
        }
        Ok(Self {
            providers: Arc::new(providers),
            default_provider,
        })
    }

    /// Default provider name.
    pub fn default_provider_name(&self) -> &str {
        &self.default_provider
    }

    /// Sorted list of provider names.
    pub fn provider_names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Look up a provider by name (defaults to the service default).
    pub fn provider(&self, name: Option<&str>) -> Result<Arc<dyn TtsProvider>, TtsError> {
        let key = name.unwrap_or(&self.default_provider);
        self.providers
            .get(key)
            .cloned()
            .ok_or_else(|| TtsError::config(format!("unknown TTS provider: {key}")))
    }

    /// Default voice for the given provider.
    pub fn default_voice_for(&self, provider: Option<&str>) -> Result<String, TtsError> {
        Ok(self.provider(provider)?.default_voice().to_string())
    }

    /// Status for a single provider.
    pub fn provider_status(&self, name: &str) -> Result<TtsProviderStatus, TtsError> {
        Ok(self.provider(Some(name))?.status())
    }

    /// Status for every registered provider.
    pub fn providers_status(&self) -> Vec<TtsProviderStatus> {
        self.providers
            .values()
            .map(|p| p.status())
            .collect()
    }

    /// List voices for the named provider (or the default).
    pub async fn list_voices(
        &self,
        provider: Option<&str>,
    ) -> Result<Vec<VoiceOption>, TtsError> {
        let p = self.provider(provider)?;
        p.list_voices().await
    }

    /// Synthesize speech. `text` is normalized; an empty result is an
    /// error.
    pub async fn synthesize(
        &self,
        text: &str,
        provider: Option<&str>,
        voice: Option<&str>,
        rate: Option<&str>,
        volume: Option<&str>,
        pitch: Option<&str>,
    ) -> Result<TtsAudio, TtsError> {
        let normalized = normalize_text_for_tts(text);
        if normalized.is_empty() {
            return Err(TtsError::argument("TTS text must not be empty"));
        }
        let provider = self.provider(provider)?;
        let options = build_tts_synthesis_options(voice, rate, volume, pitch);
        provider.synthesize(&normalized, Some(&options)).await
    }

    /// Close every provider. Errors are logged and swallowed (matches
    /// the Python `return_exceptions=True` behaviour).
    pub async fn close(&self) {
        for (name, provider) in self.providers.iter() {
            if let Err(err) = provider.close().await {
                tracing::warn!("failed to close TTS provider {name}: {err}");
            }
        }
    }
}

impl std::fmt::Debug for TtsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TtsService")
            .field("default_provider", &self.default_provider)
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A stub provider that records which provider name was used.
    struct CountingProvider {
        name: &'static str,
        label: &'static str,
        default_voice: &'static str,
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl TtsProvider for CountingProvider {
        fn name(&self) -> &str {
            self.name
        }
        fn label(&self) -> &str {
            self.label
        }
        fn default_voice(&self) -> &str {
            self.default_voice
        }
        async fn synthesize(
            &self,
            text: &str,
            _options: Option<&TtsSynthesisOptions>,
        ) -> Result<TtsAudio, TtsError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(TtsAudio {
                audio_bytes: text.as_bytes().to_vec(),
                content_type: "audio/mpeg".to_string(),
                file_extension: "mp3".to_string(),
                provider: self.name.to_string(),
                voice: self.default_voice.to_string(),
            })
        }
    }

    fn build_service() -> (TtsService, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let edge_count = Arc::new(AtomicUsize::new(0));
        let openai_count = Arc::new(AtomicUsize::new(0));
        let mut providers: BTreeMap<String, Arc<dyn TtsProvider>> = BTreeMap::new();
        providers.insert(
            "edge".to_string(),
            Arc::new(CountingProvider {
                name: "edge",
                label: "Edge TTS",
                default_voice: "zh-CN-XiaoxiaoNeural",
                counter: edge_count.clone(),
            }),
        );
        providers.insert(
            "openai-compatible".to_string(),
            Arc::new(CountingProvider {
                name: "openai-compatible",
                label: "OpenAI TTS",
                default_voice: "alloy",
                counter: openai_count.clone(),
            }),
        );
        let service = TtsService::new(providers, "edge").expect("service builds");
        (service, edge_count, openai_count)
    }

    #[tokio::test]
    async fn dispatches_to_named_provider() {
        let (svc, edge, openai) = build_service();
        let audio = svc
            .synthesize(
                "hello",
                Some("openai-compatible"),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("synthesize ok");
        assert_eq!(audio.provider, "openai-compatible");
        assert_eq!(openai.load(Ordering::SeqCst), 1);
        assert_eq!(edge.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn dispatches_to_default_when_unspecified() {
        let (svc, edge, openai) = build_service();
        let audio = svc
            .synthesize("hi", None, None, None, None, None)
            .await
            .expect("synthesize ok");
        assert_eq!(audio.provider, "edge");
        assert_eq!(edge.load(Ordering::SeqCst), 1);
        assert_eq!(openai.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_normalized_text_rejected() {
        let (svc, _, _) = build_service();
        // A bare markdown fence with empty content normalizes to the
        // empty string; the service should refuse to synthesize it.
        let err = svc
            .synthesize("```python\n```", None, None, None, None, None)
            .await
            .err()
            .expect("empty after normalize");
        match err {
            TtsError::InvalidArgument(msg) => assert!(msg.contains("empty")),
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_provider_returns_config_error() {
        let (svc, _, _) = build_service();
        let err = svc
            .provider(Some("does-not-exist"))
            .err()
            .expect("missing provider");
        match err {
            TtsError::InvalidConfig(msg) => assert!(msg.contains("does-not-exist")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn new_rejects_empty_map() {
        let providers: BTreeMap<String, Arc<dyn TtsProvider>> = BTreeMap::new();
        let err = TtsService::new(providers, "edge").expect_err("empty map");
        match err {
            TtsError::InvalidConfig(msg) => assert!(msg.contains("at least one")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn new_rejects_unknown_default() {
        let mut providers: BTreeMap<String, Arc<dyn TtsProvider>> = BTreeMap::new();
        providers.insert(
            "edge".to_string(),
            Arc::new(CountingProvider {
                name: "edge",
                label: "Edge TTS",
                default_voice: "alloy",
                counter: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let err = TtsService::new(providers, "missing").expect_err("missing default");
        match err {
            TtsError::InvalidConfig(msg) => assert!(msg.contains("missing")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }
}
