//! `echobot-tts` — text-to-speech subsystem for the EchoBot Rust port.
//!
//! Mirrors the Python `echobot/tts/` package 1:1. The crate ships a
//! trait-based provider abstraction and three concrete providers:
//!
//! * [`providers::edge::EdgeTtsProvider`]             — Microsoft Edge
//!   "read aloud" WebSocket API (free, no auth).
//! * [`providers::openai_compatible::OpenAICompatibleTtsProvider`]
//!   — any OpenAI `/audio/speech`-compatible HTTP endpoint.
//! * [`providers::kokoro::KokoroTtsProvider`]         — STUB behind the
//!   `kokoro` cargo feature; phase 3 will wire `sherpa-rs`.
//!
//! Use [`service::TtsService`] for a multi-provider facade. Use
//! [`factory::build_default_tts_service`] to construct a service from
//! env vars.
//!
//! ## Layered layout
//!
//! * [`base`]       — `TtsProvider` trait, request / response DTOs, the
//!   error type, voice and status types.
//! * [`text`]       — text normalization (Markdown strip, emoji
//!   stripping, whitespace collapse).
//! * [`synthesis`]  — synthesis-option helpers (parse rate, format
//!   Edge-style percent deltas).
//! * [`service`]    — `TtsService` facade (multi-provider dispatch,
//!   lifecycle).
//! * [`factory`]    — env-driven service builder.
//! * [`providers`]  — concrete TTS provider implementations.

pub mod base;
pub mod factory;
pub mod providers;
pub mod service;
pub mod synthesis;
pub mod text;

pub use base::{
    TtsAudio, TtsError, TtsProvider, TtsProviderStatus, TtsSynthesisOptions, VoiceOption,
};
pub use factory::{build_default_tts_service, build_minimal_tts_service};
pub use providers::edge::{EdgeTtsConfig, EdgeTtsProvider, DEFAULT_EDGE_VOICE};
pub use providers::openai_compatible::{
    OpenAICompatibleTtsConfig, OpenAICompatibleTtsProvider, DEFAULT_OPENAI_COMPATIBLE_TTS_RESPONSE_FORMAT,
    DEFAULT_OPENAI_COMPATIBLE_TTS_VOICE, OFFICIAL_OPENAI_TTS_VOICES,
};
pub use service::{TtsService, DEFAULT_PROVIDER_NAME};
