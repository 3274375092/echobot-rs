//! Sherpa-onnx SenseVoice provider — **STUB for v1**.
//!
//! The Python implementation (see `echobot/asr/providers/sherpa_sense_voice.py`)
//! wraps `sherpa_onnx.OfflineRecognizer.from_sense_voice(...)` and auto-
//! downloads the SenseVoice model bundle on first use.
//!
//! ## Why this is a stub
//!
//! The Rust crate [`sherpa-rs`](https://crates.io/crates/sherpa-rs) does
//! exist on crates.io (latest 0.6.8 at the time of writing) and exposes a
//! `SenseVoiceRecognizer`. However:
//!
//! * its default `download-binaries` feature pulls native
//!   `sherpa-onnx` shared libraries from a GitHub release at build time,
//!   which is brittle in offline / locked-down build environments,
//! * the build-time download adds hundreds of MB of native artifacts to
//!   the crate's footprint, and
//! * integrating the model auto-download and configuration surface would
//!   duplicate the Python `SenseVoiceModelManager` for marginal gain in
//!   v1 (where the `openai-transcriptions` provider is the supported
//!   one).
//!
//! v1 therefore ships a stub that exposes the same constructor
//! surface and configuration shape as the real provider would, but
//! returns `AsrError::NotImplemented` from every operation that would
//! touch the recognizer. The shape will let a follow-up PR wire
//! `sherpa-rs` (or a custom FFI binding) without changing the public
//! API of the crate.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::base::{AsrError, AsrProvider, Result};
use crate::models::{AsrConfig, AsrResult, ProviderStatusSnapshot, TranscriptionResult};

/// Default URL for the SenseVoice model bundle used by the Python port.
pub const DEFAULT_SENSE_VOICE_MODEL_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/\
     sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09.tar.bz2";

/// Stub configuration for the SenseVoice provider.
#[derive(Debug, Clone)]
pub struct SherpaSenseVoiceConfig {
    /// Target sample rate (SenseVoice wants 16 kHz).
    pub sample_rate: u32,
    /// If true, the provider would auto-download the model on first use.
    pub auto_download: bool,
    /// Optional override for the on-disk model root directory.
    pub model_root_dir: Option<PathBuf>,
    /// ONNX execution provider (`"cpu"`, `"cuda"`, `"coreml"`, …).
    pub execution_provider: String,
    /// Number of inference threads.
    pub num_threads: u32,
    /// Language hint (`"auto"`, `"zh"`, `"en"`, …).
    pub language: String,
    /// Whether to apply inverse text normalization.
    pub use_itn: bool,
    /// URL to download the model from if `auto_download` is true.
    pub model_url: String,
    /// Timeout for the (hypothetical) model download.
    pub download_timeout_seconds: f64,
}

impl Default for SherpaSenseVoiceConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            auto_download: true,
            model_root_dir: None,
            execution_provider: "cpu".to_string(),
            num_threads: 2,
            language: "auto".to_string(),
            use_itn: false,
            model_url: DEFAULT_SENSE_VOICE_MODEL_URL.to_string(),
            download_timeout_seconds: 600.0,
        }
    }
}

/// Sherpa-onnx SenseVoice provider stub.
///
/// See the module-level docstring for the rationale. The struct accepts
/// the same configuration shape as the Python provider and reports a
/// stable status snapshot; it just refuses to do transcription.
pub struct SherpaSenseVoiceProvider {
    config: SherpaSenseVoiceConfig,
    /// Tracks whether `on_startup` has been called. Used so the status
    /// snapshot can show "not implemented" once the user has tried to
    /// initialize.
    startup_called: Arc<Mutex<bool>>,
}

impl SherpaSenseVoiceProvider {
    /// Build a stub provider with the given configuration.
    pub fn new(config: SherpaSenseVoiceConfig) -> Result<Self> {
        if config.sample_rate == 0 {
            return Err(AsrError::Config(
                "ASR sample_rate must be positive".to_string(),
            ));
        }
        Ok(Self {
            config,
            startup_called: Arc::new(Mutex::new(false)),
        })
    }

    /// Returns a stable human-readable detail string describing why the
    /// provider is not implemented.
    pub fn not_implemented_detail(&self) -> String {
        "sherpa-onnx SenseVoice is not wired in v1 — see RUST_PORT.md".to_string()
    }
}

#[async_trait]
impl AsrProvider for SherpaSenseVoiceProvider {
    fn name(&self) -> &str {
        "sherpa-sense-voice"
    }

    fn label(&self) -> &str {
        "Sherpa SenseVoice"
    }

    async fn on_startup(&self) -> Result<()> {
        *self.startup_called.lock() = true;
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }

    async fn status_snapshot(&self) -> Result<ProviderStatusSnapshot> {
        let startup_called = *self.startup_called.lock();
        let (state, detail) = if startup_called {
            (
                "unavailable",
                self.not_implemented_detail(),
            )
        } else {
            (
                "unavailable",
                format!(
                    "{} (provider not yet started; call on_startup() to see the not-implemented detail)",
                    self.not_implemented_detail()
                ),
            )
        };
        Ok(ProviderStatusSnapshot {
            kind: "asr".to_string(),
            name: self.name().to_string(),
            label: self.label().to_string(),
            selected: false,
            available: false,
            state: state.to_string(),
            detail,
            resource_directory: self
                .config
                .model_root_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        })
    }

    async fn transcribe_samples(&self, _samples: &[f32]) -> Result<TranscriptionResult> {
        Err(AsrError::NotImplemented(self.not_implemented_detail()))
    }

    async fn transcribe_with_config(
        &self,
        _samples: &[f32],
        _config: &AsrConfig,
    ) -> Result<AsrResult> {
        Err(AsrError::NotImplemented(self.not_implemented_detail()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_reports_not_implemented() {
        let provider =
            SherpaSenseVoiceProvider::new(SherpaSenseVoiceConfig::default()).expect("build");
        let detail = provider.not_implemented_detail();
        assert!(detail.contains("not wired in v1"));
    }

    #[test]
    fn stub_status_is_unavailable() {
        let provider =
            SherpaSenseVoiceProvider::new(SherpaSenseVoiceConfig::default()).expect("build");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let snapshot = rt.block_on(provider.status_snapshot()).expect("snapshot");
        assert_eq!(snapshot.name, "sherpa-sense-voice");
        assert_eq!(snapshot.state, "unavailable");
        assert!(!snapshot.available);
    }

    #[test]
    fn stub_transcribe_returns_not_implemented() {
        let provider =
            SherpaSenseVoiceProvider::new(SherpaSenseVoiceConfig::default()).expect("build");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(provider.transcribe_samples(&[0.0_f32; 16]));
        assert!(matches!(result, Err(AsrError::NotImplemented(_))));
    }
}
