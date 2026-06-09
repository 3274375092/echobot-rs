//! `echobot-tts::providers` — concrete TTS provider implementations.
//!
//! Each provider lives in its own submodule. The factory in
//! `crate::factory` picks one based on configuration.
//!
//! Providers:
//! * [`edge`]              — Microsoft Edge "read aloud" WebSocket API.
//! * [`openai_compatible`] — any OpenAI `/audio/speech`-compatible HTTP
//!   endpoint.
//! * [`kokoro`]            — STUB; gated by the `kokoro` cargo feature.

pub mod edge;
pub mod openai_compatible;

#[cfg(feature = "kokoro")]
pub mod kokoro;

pub use edge::EdgeTtsProvider;
pub use openai_compatible::OpenAICompatibleTtsProvider;

#[cfg(feature = "kokoro")]
pub use kokoro::KokoroTtsProvider;
