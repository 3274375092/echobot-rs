//! `echobot-app` provides the HTTP front-end for the EchoBot Rust port.
//! It wires the orchestration layer together with the TTS and ASR
//! capabilities and exposes them over an `axum`-based HTTP server,
//! reusing `tower` and `tower-http` middleware (tracing, CORS, static
//! file serving via `include_dir`).
//!
//! The server hosts the chat endpoints, audio upload/streaming routes,
//! and static assets that the Python `echobot/app` module provides.
//! Conversational state is tracked in `dashmap` structures keyed by
//! `uuid`, while request metadata uses `chrono` timestamps.
//!
//! This crate is currently a skeleton; concrete handlers, routes, and
//! request/response types will be added in subsequent phases.

pub fn placeholder() -> &'static str {
    "ok"
}
