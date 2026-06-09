//! `echobot-tts` provides the text-to-speech capabilities for the EchoBot
//! Rust port. It exposes traits and provider implementations that turn text
//! into synthesized audio (typically delivered as raw PCM or encoded audio
//! frames), wrapping external TTS services via `reqwest` and serializing
//! results with `serde`/`serde_json`.
//!
//! This crate is currently a skeleton; concrete providers and audio
//! formatting helpers will be added in subsequent phases.

pub fn placeholder() -> &'static str {
    "ok"
}
