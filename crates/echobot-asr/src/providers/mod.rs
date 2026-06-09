//! Concrete ASR provider implementations.
//!
//! * [`sherpa`] — SenseVoice via sherpa-onnx. The default build ships a
//!   stub; build with `--features sherpa-rs` to opt into the real
//!   `sherpa_rs::sense_voice::SenseVoiceRecognizer`-backed provider.
//! * [`openai`] — OpenAI-compatible Transcriptions API.

pub mod openai;
pub mod sherpa;

pub use openai::{OpenAITranscriptionsConfig, OpenAITranscriptionsProvider};
pub use sherpa::{SherpaSenseVoiceConfig, SherpaSenseVoiceProvider, DEFAULT_SENSE_VOICE_MODEL_URL};
