//! Voice Activity Detection (VAD) trait surface.
//!
//! Mirrors the Python `echobot/asr/vad/base.py`. The port only implements the
//! trait surface; the concrete Silero VAD provider lives in
//! `providers/silero.rs` in a follow-up. For v1, the ASR service can be used
//! without a VAD provider by setting `selected_vad_provider = None`.

use async_trait::async_trait;

use crate::base::Result;
use crate::models::ProviderStatusSnapshot;

/// A chunk of detected speech with its start time.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechSegment {
    /// Mono `f32` PCM samples for the segment, at the provider's sample rate.
    pub samples: Vec<f32>,
    /// Start time of the segment, in milliseconds, relative to the start of
    /// the VAD session.
    pub start_ms: u64,
}

/// Result of feeding a chunk of audio (or flushing) into a VAD session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VadStepResult {
    /// `true` on the chunk where speech first became active.
    pub speech_started: bool,
    /// `true` on the chunk where active speech ended.
    pub speech_ended: bool,
    /// Any speech segments produced by this step.
    pub segments: Vec<SpeechSegment>,
}

/// A long-lived VAD detection session.
///
/// Implementations are typically `!Send` (e.g. wrapping a sherpa-onnx
/// detector) and live inside a `Box<dyn VadSession>` on the realtime
/// session. The ASR service treats this trait as the only thing it needs
/// to know.
pub trait VadSession: Send {
    /// Feed the next chunk of raw PCM-16 LE audio bytes (any sample rate
    /// the provider was constructed with) and return what the detector
    /// decided.
    fn accept_audio_bytes(&mut self, audio_bytes: &[u8]) -> VadStepResult;

    /// Flush any pending audio through the detector.
    fn flush(&mut self) -> VadStepResult;

    /// Reset the detector to its initial state.
    fn reset(&mut self);
}

/// Factory trait for VAD providers.
#[async_trait]
pub trait VadProvider: Send + Sync {
    /// Stable machine name.
    fn name(&self) -> &str;
    /// Human-readable label.
    fn label(&self) -> &str;
    /// Target sample rate the provider operates at.
    fn sample_rate(&self) -> u32;
    /// One-time async work (e.g. download a model). Default is a no-op.
    async fn on_startup(&self) -> Result<()> {
        Ok(())
    }
    /// Release resources.
    async fn close(&self) -> Result<()> {
        Ok(())
    }
    /// Build a status snapshot.
    async fn status_snapshot(&self) -> Result<ProviderStatusSnapshot>;
    /// Create a new VAD session.
    async fn create_session(&self) -> Result<Box<dyn VadSession>>;
}
