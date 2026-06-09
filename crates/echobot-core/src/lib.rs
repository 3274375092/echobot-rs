//! `echobot-core` provides the shared domain primitives for the EchoBot Rust
//! port: data models, error types, configuration, attachments, image handling,
//! naming helpers, and turn inputs used by every other crate.
//!
//! ## Layout
//!
//! * [`models`]   — LLM message / response / tool-call types and content helpers.
//! * [`config`]   — env-file loading + typed `AppConfig`.
//! * [`attachments`] — on-disk `AttachmentStore` for images and files.
//! * [`images`]   — image budget + normalization helpers (decode pipeline is
//!   out of scope for the initial port; the budget/result types define the
//!   contract the pipeline implements).
//! * [`naming`]   — slug helpers.
//! * [`turn_inputs`] — turn-input → content-block resolution.
//! * [`error`]    — `thiserror`-based top-level error type with re-exports for
//!   the per-module sub-errors.

pub mod attachments;
pub mod config;
pub mod error;
pub mod images;
pub mod models;
pub mod naming;
pub mod turn_inputs;

pub use error::{AttachmentError, ConfigError, Error, ImageError, ModelError, Result, ToolError};
