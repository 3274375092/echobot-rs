//! `echobot-cli` library entrypoint.
//!
//! Exposes the CLI's modules to integration tests and to other crates
//! that want to embed the chat REPL or the runtime-assembly helpers.
//! The actual CLI binary lives in `src/main.rs` and simply calls into
//! these modules.

pub mod app;
pub mod bridge;
pub mod chat;
pub mod common;
pub mod gateway;
pub mod runtime_assembly;
