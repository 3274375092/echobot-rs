//! Base abstraction for an ASR provider and the shared `AsrError` type.
//!
//! Mirrors `echobot/asr/providers/base.py` and `echobot/asr/sherpa.py`. The
//! `AsrProvider` trait is async; the default `on_startup` and `close` are
//! no-ops so simple providers only have to implement `status_snapshot` and
//! `transcribe`.

use async_trait::async_trait;
use thiserror::Error;

use crate::models::{AsrConfig, AsrResult, ProviderStatusSnapshot, TranscriptionResult};

/// Result alias for ASR operations.
pub type Result<T> = std::result::Result<T, AsrError>;

/// Top-level error type for the ASR subsystem.
///
/// Sub-variants are matched on directly when an outer layer needs to react
/// differently (e.g. `NotImplemented` for stubs); the message carries enough
/// detail for logs and for surfacing back to the user.
#[derive(Debug, Error)]
pub enum AsrError {
    /// Audio decoding failed (symphonia, hound, or our own resampler).
    #[error("ASR audio decode error: {0}")]
    Audio(String),

    /// An I/O error happened while reading or writing audio on disk.
    #[error("ASR I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// HTTP transport / connection error talking to a remote provider.
    #[error("ASR provider network error: {0}")]
    Network(String),

    /// Remote provider returned a non-2xx status.
    #[error("ASR provider request failed: status={status}, detail={detail}")]
    HttpStatus { status: u16, detail: String },

    /// Provider response could not be parsed.
    #[error("ASR provider response error: {0}")]
    Response(String),

    /// Configuration is missing or invalid (e.g. no API key, no model URL).
    #[error("ASR provider configuration error: {0}")]
    Config(String),

    /// Provider is not ready yet (model still downloading, etc.).
    #[error("ASR provider not ready: {0}")]
    NotReady(String),

    /// The provider is a deliberate stub (see `providers/sherpa.rs`).
    #[error("ASR provider not implemented: {0}")]
    NotImplemented(String),

    /// Reqwest / multipart error from `reqwest`.
    #[error("ASR provider transport error: {0}")]
    Reqwest(#[from] reqwest::Error),

    /// JSON (de)serialization error.
    #[error("ASR JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Multipart form construction failed.
    #[error("ASR multipart error: {0}")]
    Multipart(String),

    /// Catch-all for unexpected internal failures.
    #[error("ASR internal error: {0}")]
    Internal(String),
}

/// Common interface every ASR provider implements.
///
/// `transcribe_samples` takes a mono 16 kHz float buffer (the same PCM
/// representation used by SenseVoice). The service layer decodes arbitrary
/// audio formats down to that representation before dispatching.
#[async_trait]
pub trait AsrProvider: Send + Sync {
    /// Stable machine name (e.g. `"sherpa-sense-voice"`).
    fn name(&self) -> &str;

    /// Human-readable label.
    fn label(&self) -> &str;

    /// Hook for one-time async work (kick off a model download, build a
    /// HTTP client, etc.). Default is a no-op.
    async fn on_startup(&self) -> Result<()> {
        Ok(())
    }

    /// Release any resources (HTTP clients, file handles, background tasks).
    /// Default is a no-op.
    async fn close(&self) -> Result<()> {
        Ok(())
    }

    /// Build a status snapshot describing whether the provider is ready.
    async fn status_snapshot(&self) -> Result<ProviderStatusSnapshot>;

    /// Transcribe a mono float PCM buffer.
    ///
    /// The default implementation routes through the structured
    /// `transcribe_with_config` method so providers can choose to override
    /// either one.
    async fn transcribe_samples(&self, samples: &[f32]) -> Result<TranscriptionResult> {
        let _ = samples;
        Err(AsrError::NotImplemented(format!(
            "{} provider does not implement sample transcription",
            self.name()
        )))
    }

    /// Transcribe a mono float PCM buffer with a config object.
    ///
    /// Providers that accept an `AsrConfig` should override this; the default
    /// forwards to `transcribe_samples` and ignores the config.
    async fn transcribe_with_config(
        &self,
        samples: &[f32],
        _config: &AsrConfig,
    ) -> Result<AsrResult> {
        let result = self.transcribe_samples(samples).await?;
        Ok(AsrResult {
            text: result.text,
            language: result.language,
            segments: Vec::new(),
        })
    }
}

/// Builder for an `AsrService`.
///
/// This is the equivalent of the Python `factory.py` builder. It's a small
/// struct so the user can mix in providers and config values incrementally;
/// `build` validates that the required names resolve.
pub struct AsrServiceBuilder {
    /// `name -> provider` map.
    asr_providers: Vec<(String, std::sync::Arc<dyn AsrProvider>)>,
    /// `name -> VAD provider` map.
    vad_providers: Vec<(String, std::sync::Arc<dyn crate::vad::VadProvider>)>,
    /// Name of the ASR provider that should be active.
    selected_asr_provider: Option<String>,
    /// Name of the VAD provider that should be active (None = disabled).
    selected_vad_provider: Option<String>,
    /// Target sample rate for the service.
    sample_rate: u32,
}

impl std::fmt::Debug for AsrServiceBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsrServiceBuilder")
            .field("asr_providers", &self.asr_providers.len())
            .field("vad_providers", &self.vad_providers.len())
            .field("selected_asr_provider", &self.selected_asr_provider)
            .field("selected_vad_provider", &self.selected_vad_provider)
            .field("sample_rate", &self.sample_rate)
            .finish()
    }
}

impl Default for AsrServiceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AsrServiceBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            asr_providers: Vec::new(),
            vad_providers: Vec::new(),
            selected_asr_provider: None,
            selected_vad_provider: None,
            sample_rate: 16_000,
        }
    }

    /// Register an ASR provider under a name.
    pub fn with_asr_provider(
        mut self,
        name: impl Into<String>,
        provider: std::sync::Arc<dyn AsrProvider>,
    ) -> Self {
        self.asr_providers.push((name.into(), provider));
        self
    }

    /// Register a VAD provider under a name.
    pub fn with_vad_provider(
        mut self,
        name: impl Into<String>,
        provider: std::sync::Arc<dyn crate::vad::VadProvider>,
    ) -> Self {
        self.vad_providers.push((name.into(), provider));
        self
    }

    /// Set the name of the active ASR provider.
    pub fn selected_asr(mut self, name: impl Into<String>) -> Self {
        self.selected_asr_provider = Some(name.into());
        self
    }

    /// Set the name of the active VAD provider, or disable VAD.
    pub fn selected_vad(mut self, name: Option<String>) -> Self {
        self.selected_vad_provider = name;
        self
    }

    /// Set the target sample rate.
    pub fn sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Build the service. Returns an error if no ASR providers were
    /// registered, if the selected provider name is unknown, or if the
    /// sample rate is not positive.
    pub fn build(self) -> Result<crate::service::AsrService> {
        if self.asr_providers.is_empty() {
            return Err(AsrError::Config(
                "at least one ASR provider is required".to_string(),
            ));
        }
        if self.sample_rate == 0 {
            return Err(AsrError::Config(
                "ASR sample_rate must be positive".to_string(),
            ));
        }

        let mut asr_map = std::collections::HashMap::new();
        for (name, provider) in self.asr_providers {
            asr_map.insert(name, provider);
        }
        let mut vad_map = std::collections::HashMap::new();
        for (name, provider) in self.vad_providers {
            vad_map.insert(name, provider);
        }

        let selected_asr_provider = self
            .selected_asr_provider
            .or_else(|| asr_map.keys().next().cloned())
            .ok_or_else(|| AsrError::Config("no ASR provider selected".to_string()))?;
        if !asr_map.contains_key(&selected_asr_provider) {
            return Err(AsrError::Config(format!(
                "unknown ASR provider: {selected_asr_provider}"
            )));
        }

        let selected_vad_provider = match self.selected_vad_provider {
            Some(name) if !name.is_empty() => {
                if !vad_map.contains_key(&name) {
                    return Err(AsrError::Config(format!(
                        "unknown VAD provider: {name}"
                    )));
                }
                Some(name)
            }
            _ => None,
        };

        Ok(crate::service::AsrService::from_parts(
            asr_map,
            vad_map,
            selected_asr_provider,
            selected_vad_provider,
            self.sample_rate,
        ))
    }
}
