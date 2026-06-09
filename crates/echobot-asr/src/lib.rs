//! `echobot-asr` provides the automatic speech recognition (ASR) and
//! voice-activity-detection (VAD) capabilities for the EchoBot Rust port.
//!
//! ## Layout
//!
//! * [`base`]        — `AsrProvider` async trait, `AsrError`, `AsrServiceBuilder`.
//! * [`models`]      — `AsrConfig`, `AsrSegment`, `AsrWord`, `AsrResult`,
//!   `TranscriptionResult`, status snapshot types.
//! * [`service`]     — `AsrService`: a registry of ASR + VAD providers that
//!   dispatches to the active one.
//! * [`audio`]       — WAV encode / decode (`hound`), general audio decode
//!   (`symphonia`), linear resampling, PCM-16 LE helper.
//! * [`vad`]         — `VadProvider` / `VadSession` trait surface (the
//!   concrete Silero provider lands in a follow-up).
//! * [`realtime`]    — `RealtimeAsrSession` that pairs an `AsrProvider` with
//!   a VAD session and streams transcript events.
//! * [`factory`]     — `build_default_asr_service` + env helpers.
//! * [`providers`]   — concrete ASR provider implementations:
//!   * `sherpa` — SenseVoice (stub for v1, see the module doc).
//!   * `openai` — OpenAI-compatible Transcriptions API.
//!
//! Audio decoding and remote HTTP calls are CPU / network heavy; the
//! service dispatches them through `tokio::task::spawn_blocking` so the
//! async runtime stays responsive.

pub mod audio;
pub mod base;
pub mod factory;
pub mod models;
pub mod providers;
pub mod realtime;
pub mod service;
pub mod vad;

pub use base::{AsrError, AsrProvider, AsrServiceBuilder, Result as AsrResultErr};
pub use factory::{
    build_default_asr_service, build_default_openai_provider, env_flag, env_float, env_int,
    env_optional_float, env_text, optional_provider_name, resolve_optional_path,
    DEFAULT_ASR_PROVIDER, DEFAULT_VAD_PROVIDER,
};
pub use models::{
    AsrConfig, AsrResult, AsrSegment, AsrStatusSnapshot, AsrWord, ProviderStatusSnapshot,
    TranscriptionResult,
};
pub use providers::openai::{
    OpenAITranscriptionsConfig, OpenAITranscriptionsProvider, DEFAULT_BASE_URL,
    MULTIPART_FILE_FIELD, MULTIPART_MODEL_FIELD,
};
pub use providers::sherpa::{
    SherpaSenseVoiceConfig, SherpaSenseVoiceProvider, DEFAULT_SENSE_VOICE_MODEL_URL,
};
pub use realtime::{RealtimeAsrEvent, RealtimeAsrSession};
pub use service::AsrService;
pub use vad::{SpeechSegment, VadProvider, VadSession, VadStepResult};
