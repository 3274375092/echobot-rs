//! Base abstraction for an LLM provider.
//!
//! Mirrors `echobot/providers/base.py`. The trait is async; the default
//! `stream_generate` implementation falls back to the non-streaming
//! `generate` method, matching the Python behaviour.

use std::collections::HashMap;

use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use serde_json::Value;

use echobot_core::models::{message_content_to_text, LLMMessage, LLMResponse, LLMTool};

/// Common interface every LLM provider implements.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Sends a non-streaming chat completion request.
    async fn generate(
        &self,
        messages: &[LLMMessage],
        tools: Option<&[LLMTool]>,
        tool_choice: Option<&ToolChoice>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        extra_body: Option<&HashMap<String, Value>>,
    ) -> Result<LLMResponse, ProviderError>;

    /// Streams chat completion text chunks.
    ///
    /// The default implementation issues a non-streaming `generate` and
    /// yields the full text as one chunk. Providers with native streaming
    /// support should override this.
    async fn stream_generate<'a>(
        &'a self,
        messages: &'a [LLMMessage],
        tools: Option<&'a [LLMTool]>,
        tool_choice: Option<&'a ToolChoice>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        extra_body: Option<&'a HashMap<String, Value>>,
    ) -> Result<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + 'a>, ProviderError>
    where
        Self: 'a,
    {
        let response = self
            .generate(
                messages,
                tools,
                tool_choice,
                temperature,
                max_tokens,
                extra_body,
            )
            .await?;
        let text = message_content_to_text(&response.message.content);
        let stream = futures::stream::once(async move { Ok::<_, ProviderError>(text) })
            .filter(|s| {
                let s = match s {
                    Ok(s) => s,
                    Err(_) => return futures::future::ready(true),
                };
                futures::future::ready(!s.is_empty())
            });
        Ok(Box::new(stream))
    }
}

/// Tool choice: either a string shortcut (`"auto"`, `"none"`, `"required"`)
/// or a JSON object describing a specific function.
#[derive(Debug, Clone)]
pub enum ToolChoice {
    /// `"auto"`, `"none"`, `"required"`, or a custom provider string.
    Auto,
    None,
    Required,
    /// A custom tool-choice string (e.g. provider-specific values).
    Named(String),
    /// A structured object: e.g. `{"type": "function", "function": {"name": "x"}}`.
    Structured(Value),
}

impl ToolChoice {
    /// Serializes to the wire-format value.
    pub fn to_value(&self) -> Value {
        match self {
            ToolChoice::Auto => Value::String("auto".to_string()),
            ToolChoice::None => Value::String("none".to_string()),
            ToolChoice::Required => Value::String("required".to_string()),
            ToolChoice::Named(s) => Value::String(s.clone()),
            ToolChoice::Structured(v) => v.clone(),
        }
    }
}

/// Provider-level error type. Wraps a message and an optional source.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// HTTP transport / connection error.
    #[error("LLM provider network error: {0}")]
    Network(String),

    /// Provider returned a non-2xx status code with a detail body.
    #[error("LLM provider request failed: status={status}, detail={detail}")]
    HttpStatus { status: u16, detail: String },

    /// Response could not be parsed.
    #[error("LLM provider response error: {0}")]
    Response(String),

    /// Streaming chunk was malformed.
    #[error("LLM provider stream error: {0}")]
    Stream(String),

    /// Configuration is invalid (e.g. missing API key).
    #[error("LLM provider configuration error: {0}")]
    Config(String),

    /// The request was invalid before being sent.
    #[error("LLM provider request error: {0}")]
    Request(String),
}
