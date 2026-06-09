//! `AsrService` — the top-level service that owns a set of ASR and VAD
//! providers and dispatches requests to the active one.
//!
//! Mirrors `echobot/asr/service.py`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::audio::read_wav_bytes;
use crate::base::{AsrError, AsrProvider, Result};
use crate::models::{AsrStatusSnapshot, ProviderStatusSnapshot, TranscriptionResult};
use crate::realtime::RealtimeAsrSession;
use crate::vad::VadProvider;

/// The configured ASR service.
pub struct AsrService {
    asr_providers: HashMap<String, Arc<dyn AsrProvider>>,
    vad_providers: HashMap<String, Arc<dyn VadProvider>>,
    selected_asr_provider: Mutex<String>,
    selected_vad_provider: Mutex<Option<String>>,
    sample_rate: u32,
}

impl AsrService {
    /// Build an `AsrService` from the individual pieces. Usually called via
    /// [`crate::base::AsrServiceBuilder::build`].
    pub fn from_parts(
        asr_providers: HashMap<String, Arc<dyn AsrProvider>>,
        vad_providers: HashMap<String, Arc<dyn VadProvider>>,
        selected_asr_provider: String,
        selected_vad_provider: Option<String>,
        sample_rate: u32,
    ) -> Self {
        Self {
            asr_providers,
            vad_providers,
            selected_asr_provider: Mutex::new(selected_asr_provider),
            selected_vad_provider: Mutex::new(selected_vad_provider),
            sample_rate,
        }
    }

    /// The name of the currently active ASR provider.
    pub async fn selected_asr_provider(&self) -> String {
        self.selected_asr_provider.lock().await.clone()
    }

    /// Sorted list of registered ASR provider names.
    pub fn asr_provider_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.asr_providers.keys().cloned().collect();
        names.sort();
        names
    }

    /// The target sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Run provider `on_startup` hooks for the active ASR and VAD.
    pub async fn on_startup(&self) -> Result<()> {
        let asr = self.active_asr_provider().await?;
        asr.on_startup().await?;
        if let Some(vad) = self.active_vad_provider().await? {
            vad.on_startup().await?;
        }
        Ok(())
    }

    /// Switch the active ASR provider. The new provider must already be
    /// registered.
    pub async fn set_selected_asr_provider(&self, provider_name: &str) -> Result<()> {
        let normalized = provider_name.trim();
        if normalized.is_empty() {
            return Err(AsrError::Config("ASR provider name must not be empty".to_string()));
        }
        if !self.asr_providers.contains_key(normalized) {
            return Err(AsrError::Config(format!(
                "unknown ASR provider: {normalized}"
            )));
        }
        *self.selected_asr_provider.lock().await = normalized.to_string();
        self.on_startup().await
    }

    /// Close all providers. Errors are swallowed (best effort) and
    /// reported via `tracing::warn!` only.
    pub async fn close(&self) -> Result<()> {
        let mut errors: Vec<String> = Vec::new();
        for provider in self.asr_providers.values() {
            if let Err(e) = provider.close().await {
                errors.push(format!("{}: {e}", provider.name()));
            }
        }
        for provider in self.vad_providers.values() {
            if let Err(e) = provider.close().await {
                errors.push(format!("{}: {e}", provider.name()));
            }
        }
        if !errors.is_empty() {
            tracing::warn!("ASR service close reported errors: {}", errors.join("; "));
        }
        Ok(())
    }

    /// Build a status snapshot for the service.
    pub async fn status_snapshot(&self) -> Result<AsrStatusSnapshot> {
        let selected_asr = self.selected_asr_provider().await;
        let selected_vad = self.selected_vad_provider.lock().await.clone();

        let mut asr_statuses: Vec<ProviderStatusSnapshot> = Vec::new();
        for name in self.asr_provider_names() {
            if let Some(provider) = self.asr_providers.get(&name) {
                let is_selected = name == selected_asr;
                match provider.status_snapshot().await {
                    Ok(s) => asr_statuses.push(ProviderStatusSnapshot {
                        // Use the registration name so callers can match on
                        // it; the underlying provider's name() is still
                        // available via `s.name` if needed.
                        name: name.clone(),
                        selected: is_selected,
                        ..s
                    }),
                    Err(e) => {
                        asr_statuses.push(ProviderStatusSnapshot {
                            kind: "asr".to_string(),
                            name: name.clone(),
                            label: provider.label().to_string(),
                            selected: is_selected,
                            available: false,
                            state: "error".to_string(),
                            detail: format!("status snapshot failed: {e}"),
                            resource_directory: String::new(),
                        });
                    }
                }
            }
        }

        let mut vad_names: Vec<&String> = self.vad_providers.keys().collect();
        vad_names.sort();
        let mut vad_statuses: Vec<ProviderStatusSnapshot> = Vec::new();
        for name in vad_names {
            if let Some(provider) = self.vad_providers.get(name) {
                let is_selected = selected_vad.as_deref() == Some(name);
                match provider.status_snapshot().await {
                    Ok(s) => vad_statuses.push(ProviderStatusSnapshot {
                        name: name.clone(),
                        selected: is_selected,
                        ..s
                    }),
                    Err(e) => {
                        vad_statuses.push(ProviderStatusSnapshot {
                            kind: "vad".to_string(),
                            name: name.clone(),
                            label: provider.label().to_string(),
                            selected: is_selected,
                            available: false,
                            state: "error".to_string(),
                            detail: format!("status snapshot failed: {e}"),
                            resource_directory: String::new(),
                        });
                    }
                }
            }
        }

        let active_asr_status = asr_statuses
            .iter()
            .find(|s| s.name == selected_asr)
            .cloned()
            .ok_or_else(|| AsrError::Internal("active ASR provider status is missing".to_string()))?;

        let active_vad_status = selected_vad
            .as_ref()
            .and_then(|name| vad_statuses.iter().find(|s| &s.name == name).cloned());

        let detail = build_service_detail(&active_asr_status, active_vad_status.as_ref());

        Ok(AsrStatusSnapshot {
            available: active_asr_status.available,
            state: active_asr_status.state.clone(),
            detail,
            sample_rate: self.sample_rate,
            selected_asr_provider: selected_asr,
            selected_vad_provider: selected_vad.clone().unwrap_or_default(),
            always_listen_supported: active_vad_status
                .as_ref()
                .map(|s| s.available)
                .unwrap_or(false),
            asr_providers: asr_statuses,
            vad_providers: vad_statuses,
        })
    }

    /// Decode a WAV byte buffer and transcribe it with the active ASR
    /// provider.
    pub async fn transcribe_wav_bytes(&self, audio_bytes: &[u8]) -> Result<TranscriptionResult> {
        let provider = self.active_asr_provider().await?;
        let status = provider.status_snapshot().await?;
        if !status.available {
            return Err(AsrError::NotReady(status.detail));
        }
        let sample_rate = self.sample_rate;
        let bytes = audio_bytes.to_vec();
        let samples = tokio::task::spawn_blocking(move || read_wav_bytes(&bytes, sample_rate))
            .await
            .map_err(|e| AsrError::Internal(format!("blocking task join failed: {e}")))??;
        provider.transcribe_samples(&samples).await
    }

    /// Create a realtime streaming session backed by the active ASR
    /// provider and the active VAD provider.
    pub async fn create_realtime_session(&self) -> Result<RealtimeAsrSession> {
        let asr = self.active_asr_provider().await?;
        let vad = self
            .active_vad_provider()
            .await?
            .ok_or_else(|| AsrError::Config("VAD provider is not configured".to_string()))?;
        let asr_status = asr.status_snapshot().await?;
        if !asr_status.available {
            return Err(AsrError::NotReady(asr_status.detail));
        }
        let vad_status = vad.status_snapshot().await?;
        if !vad_status.available {
            return Err(AsrError::NotReady(vad_status.detail));
        }
        let vad_session = vad.create_session().await?;
        Ok(RealtimeAsrSession::new(asr, vad_session))
    }

    async fn active_asr_provider(&self) -> Result<Arc<dyn AsrProvider>> {
        let name = self.selected_asr_provider.lock().await.clone();
        self.asr_providers
            .get(&name)
            .cloned()
            .ok_or_else(|| AsrError::Internal(format!("active ASR provider {name} not registered")))
    }

    async fn active_vad_provider(&self) -> Result<Option<Arc<dyn VadProvider>>> {
        let guard = self.selected_vad_provider.lock().await;
        match guard.as_ref() {
            Some(name) => Ok(self.vad_providers.get(name).cloned()),
            None => Ok(None),
        }
    }
}

fn build_service_detail(
    active_asr: &ProviderStatusSnapshot,
    active_vad: Option<&ProviderStatusSnapshot>,
) -> String {
    if !active_asr.available {
        return active_asr.detail.clone();
    }
    let base = if active_asr.detail.is_empty() {
        "ASR is ready.".to_string()
    } else {
        active_asr.detail.clone()
    };
    match active_vad {
        None => format!("{base} Always-listen disabled: no VAD provider configured."),
        Some(v) if !v.available => format!("{base} Always-listen unavailable: {v_detail}", v_detail = v.detail),
        Some(_) => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::AsrError;
    use crate::models::TranscriptionResult;
    use crate::providers::openai::{OpenAITranscriptionsConfig, OpenAITranscriptionsProvider};
    use crate::providers::sherpa::{SherpaSenseVoiceConfig, SherpaSenseVoiceProvider};
    use std::time::Duration;

    fn dummy_openai() -> Arc<dyn AsrProvider> {
        let cfg = OpenAITranscriptionsConfig {
            sample_rate: 16_000,
            api_key: "test".into(),
            model: "whisper-1".into(),
            base_url: "http://localhost:9999/v1".into(),
            timeout: Duration::from_secs(1),
            ..Default::default()
        };
        Arc::new(OpenAITranscriptionsProvider::new(cfg).expect("build"))
    }

    fn dummy_sherpa_stub() -> Arc<dyn AsrProvider> {
        Arc::new(SherpaSenseVoiceProvider::new(SherpaSenseVoiceConfig::default()).expect("build"))
    }

    #[tokio::test]
    async fn service_dispatches_to_selected_provider() {
        let mut asr = HashMap::new();
        asr.insert("openai".to_string(), dummy_openai());
        asr.insert("sherpa".to_string(), dummy_sherpa_stub());

        let service = AsrService::from_parts(asr, HashMap::new(), "openai".to_string(), None, 16_000);

        let openai_name = service.selected_asr_provider().await;
        assert_eq!(openai_name, "openai");

        // Switch providers.
        service
            .set_selected_asr_provider("sherpa")
            .await
            .expect("switch");
        let sherpa_name = service.selected_asr_provider().await;
        assert_eq!(sherpa_name, "sherpa");
    }

    #[tokio::test]
    async fn service_rejects_unknown_provider() {
        let mut asr = HashMap::new();
        asr.insert("openai".to_string(), dummy_openai());
        let service = AsrService::from_parts(asr, HashMap::new(), "openai".to_string(), None, 16_000);
        let err = service
            .set_selected_asr_provider("nope")
            .await
            .expect_err("should fail");
        assert!(matches!(err, AsrError::Config(_)));
    }

    #[tokio::test]
    async fn service_lists_provider_names_sorted() {
        let mut asr = HashMap::new();
        asr.insert("zeta".to_string(), dummy_openai());
        asr.insert("alpha".to_string(), dummy_sherpa_stub());
        let service = AsrService::from_parts(asr, HashMap::new(), "alpha".to_string(), None, 16_000);
        let names = service.asr_provider_names();
        assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[tokio::test]
    async fn status_snapshot_includes_all_providers() {
        let mut asr = HashMap::new();
        asr.insert("openai".to_string(), dummy_openai());
        asr.insert("sherpa".to_string(), dummy_sherpa_stub());
        let service = AsrService::from_parts(asr, HashMap::new(), "sherpa".to_string(), None, 16_000);
        let snapshot = service.status_snapshot().await.expect("snapshot");
        assert_eq!(snapshot.asr_providers.len(), 2);
        let names: Vec<&str> = snapshot
            .asr_providers
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(names.contains(&"openai"));
        assert!(names.contains(&"sherpa"));
        // The active provider should be flagged.
        let sherpa_status = snapshot
            .asr_providers
            .iter()
            .find(|s| s.name == "sherpa")
            .expect("sherpa status");
        assert!(sherpa_status.selected);
        assert!(!sherpa_status.available, "sherpa stub is never available");
    }

    // Compile-time check: dummy_openai satisfies the trait at the type level.
    #[allow(dead_code)]
    fn _assert_provider_trait_object(p: Arc<dyn AsrProvider>) -> Arc<dyn AsrProvider> {
        p
    }

    #[allow(dead_code)]
    fn _assert_transcription_result_default() -> TranscriptionResult {
        TranscriptionResult::default()
    }
}
