//! Concrete ASR provider implementations.
//!
//! * [`sherpa`] — SenseVoice via sherpa-onnx. **STUB for v1** (see the
//!   file's docstring for the rationale).
//! * [`openai`] — OpenAI-compatible Transcriptions API.

pub mod openai;
pub mod sherpa;

pub use openai::{OpenAITranscriptionsConfig, OpenAITranscriptionsProvider};
pub use sherpa::{SherpaSenseVoiceConfig, SherpaSenseVoiceProvider, DEFAULT_SENSE_VOICE_MODEL_URL};
