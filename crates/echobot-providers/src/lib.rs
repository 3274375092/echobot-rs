//! `echobot-providers` defines the LLM provider abstraction (a common trait
//! plus request/response types) and ships an OpenAI-compatible provider that
//! can be pointed at any endpoint speaking the chat completions protocol.
//!
//! ## Layout
//!
//! * [`base`]             — `LLMProvider` async trait, `ToolChoice`, `ProviderError`.
//! * [`settings`]         — `OpenAICompatibleSettings` (env + struct).
//! * [`openai_compatible`] — the `OpenAICompatibleProvider` implementation
//!   (HTTP via `reqwest`, SSE via `bytes_stream` + `futures::Stream`).

pub mod base;
pub mod openai_compatible;
pub mod settings;

pub use base::{LLMProvider, ProviderError, ToolChoice};
pub use openai_compatible::OpenAICompatibleProvider;
pub use settings::OpenAICompatibleSettings;
