//! `echobot-asr` provides the automatic speech recognition capabilities for
//! the EchoBot Rust port. It exposes traits and provider implementations
//! that decode audio input (WAV via `hound`, additional formats via
//! `symphonia`) and produce transcripts, wrapping external ASR services
//! through `reqwest`.
//!
//! Internal state shared between decode stages is protected with
//! `parking_lot` synchronization primitives.
//!
//! This crate is currently a skeleton; concrete providers and decode
//! pipelines will be added in subsequent phases.

pub fn placeholder() -> &'static str {
    "ok"
}
