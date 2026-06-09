//! Realtime streaming ASR session.
//!
//! Mirrors `echobot/asr/realtime.py`. A `RealtimeAsrSession` pairs an
//! `AsrProvider` with a VAD session: each new audio chunk is fed to the
//! VAD; when the VAD reports a finished segment, that segment is sent
//! through the ASR provider and the resulting text is surfaced as a
//! `transcript` event.

use std::sync::Arc;

use serde::Serialize;

use crate::base::{AsrProvider, Result};
use crate::vad::{VadSession, VadStepResult};

/// One event surfaced to the consumer (the UI / gateway layer).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeAsrEvent {
    /// The VAD just transitioned from silence to speech.
    SpeechStart,
    /// The VAD just transitioned from speech to silence (only emitted
    /// after at least one transcript was produced).
    SpeechEnd,
    /// A transcribed segment.
    Transcript {
        text: String,
        language: String,
        start_ms: u64,
    },
}

/// Realtime ASR session. Owns a VAD session and an `Arc` to the ASR
/// provider.
pub struct RealtimeAsrSession {
    asr_provider: Arc<dyn AsrProvider>,
    vad_session: Box<dyn VadSession>,
}

impl RealtimeAsrSession {
    /// Build a new realtime session.
    pub fn new(asr_provider: Arc<dyn AsrProvider>, vad_session: Box<dyn VadSession>) -> Self {
        Self {
            asr_provider,
            vad_session,
        }
    }

    /// Feed the next audio chunk (raw PCM-16 LE bytes) into the session
    /// and return any events the VAD + ASR produced.
    ///
    /// The VAD step is intentionally synchronous — a single VAD pass on a
    /// short audio chunk is well under a millisecond on CPU and the trait
    /// signature is `fn(&mut self, &[u8])`. ASR is dispatched on the
    /// current task; providers that do their own blocking work (e.g. the
    /// OpenAI provider) already use `spawn_blocking` internally.
    pub async fn accept_audio_bytes(
        &mut self,
        audio_bytes: &[u8],
    ) -> Result<Vec<RealtimeAsrEvent>> {
        let step_result = self.vad_session.accept_audio_bytes(audio_bytes);
        self.build_events(step_result).await
    }

    /// Flush any pending audio through the VAD and ASR.
    pub async fn flush(&mut self) -> Result<Vec<RealtimeAsrEvent>> {
        let step_result = self.vad_session.flush();
        self.build_events(step_result).await
    }

    /// Reset the VAD to its initial state. Pending segments are dropped.
    pub fn reset(&mut self) {
        self.vad_session.reset();
    }

    async fn build_events(&self, step: VadStepResult) -> Result<Vec<RealtimeAsrEvent>> {
        let mut events: Vec<RealtimeAsrEvent> = Vec::new();
        if step.speech_started {
            events.push(RealtimeAsrEvent::SpeechStart);
        }

        let mut transcript_emitted = false;
        for segment in step.segments {
            // The VAD segment samples are already mono f32 at the
            // provider's sample rate.
            let result = self
                .asr_provider
                .transcribe_samples(&segment.samples)
                .await?;
            if result.text.is_empty() {
                continue;
            }
            transcript_emitted = true;
            events.push(RealtimeAsrEvent::Transcript {
                text: result.text,
                language: result.language,
                start_ms: segment.start_ms,
            });
        }

        if step.speech_ended && transcript_emitted {
            events.push(RealtimeAsrEvent::SpeechEnd);
        }
        Ok(events)
    }
}
