//! OpenAI-compatible chat completions provider.
//!
//! Speaks the OpenAI `/v1/chat/completions` wire format, including:
//!
//! * `tools` / `tool_choice`
//! * `temperature` / `max_tokens`
//! * `stream: true` (SSE)
//! * `response_format`, `extra_body` (merged into the request payload)
//! * `tools[].function.arguments` and tool-call parsing
//! * reasoning fields (`reasoning_content` / `reasoning`)
//! * `<think>...</think>` tag extraction
//! * `attachment://` image-URL rewriting via the [`AttachmentStore`]
//!
//! The implementation uses `reqwest` for both blocking-style JSON POSTs and
//! async streaming (SSE bytes are parsed line by line). The
//! [`OpenAICompatibleProvider`] is `Send + Sync` and reusable across
//! concurrent calls.

use std::collections::HashMap;

use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde_json::Value;

use echobot_core::attachments::{AttachmentStore, ATTACHMENT_URL_PREFIX};
use echobot_core::models::{
    file_attachment_summary, message_content_to_text, normalize_message_content, FileAttachmentPayload,
    ImageUrlPayload, LLMMessage, LLMResponse, LLMTool, LLMUsage, MessageRole,
    ReasoningField, ToolCall, FILE_ATTACHMENT_CONTENT_BLOCK_TYPE,
};

use crate::base::{LLMProvider, ProviderError, ToolChoice};
use crate::settings::OpenAICompatibleSettings;

const THINKING_TAG_PATTERN: &str = r"<think>(.*?)</think>";
const REASONING_RESPONSE_FIELDS: &[&str] = &["reasoning_content", "reasoning"];

/// OpenAI-compatible chat completions provider.
#[derive(Debug, Clone)]
pub struct OpenAICompatibleProvider {
    settings: OpenAICompatibleSettings,
    client: Client,
    attachment_store: Option<AttachmentStore>,
}

impl OpenAICompatibleProvider {
    /// Constructs a provider with the given settings and an optional
    /// [`AttachmentStore`] used to resolve `attachment://` URLs.
    pub fn new(settings: OpenAICompatibleSettings, attachment_store: Option<AttachmentStore>) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .timeout(settings.timeout)
            .build()
            .map_err(|e| ProviderError::Config(format!("failed to build reqwest client: {e}")))?;
        Ok(Self {
            settings,
            client,
            attachment_store,
        })
    }

    /// Returns the underlying settings.
    pub fn settings(&self) -> &OpenAICompatibleSettings {
        &self.settings
    }

    /// Returns the configured HTTP client (handy for tests).
    pub fn http_client(&self) -> &Client {
        &self.client
    }

    /// Sets the attachment store after construction.
    pub fn set_attachment_store(&mut self, store: Option<AttachmentStore>) {
        self.attachment_store = store;
    }

    /// Builds the JSON payload for a chat completions request.
    ///
    /// The argument list mirrors the Python provider's `build_payload`
    /// 1:1 (see `echobot.providers.openai_compatible.OpenAICompatibleProvider.build_payload`).
    /// Clippy's `too_many_arguments` lint is intentionally suppressed on this
    /// method so the call site stays trivially portable between the Python
    /// and Rust ports.
    #[allow(clippy::too_many_arguments)]
    pub fn build_payload(
        &self,
        messages: &[LLMMessage],
        tools: Option<&[LLMTool]>,
        tool_choice: Option<&ToolChoice>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        extra_body: Option<&HashMap<String, Value>>,
        stream: bool,
    ) -> Value {
        let merged = merge_system_messages(messages);
        let messages_value: Vec<Value> = merged
            .iter()
            .map(|m| self.message_payload(m))
            .collect();

        let mut payload = serde_json::Map::new();
        payload.insert("model".into(), Value::String(self.settings.model.clone()));
        payload.insert("messages".into(), Value::Array(messages_value));

        if let Some(tools) = tools {
            if !tools.is_empty() {
                let arr: Vec<Value> = tools.iter().map(|t| t.to_value()).collect();
                payload.insert("tools".into(), Value::Array(arr));
            }
        }
        if let Some(choice) = tool_choice {
            payload.insert("tool_choice".into(), choice.to_value());
        }
        if let Some(t) = temperature {
            payload.insert("temperature".into(), Value::from(t));
        }
        if let Some(n) = max_tokens {
            payload.insert("max_tokens".into(), Value::from(n));
        }
        if stream {
            payload.insert("stream".into(), Value::Bool(true));
        }

        for (k, v) in &self.settings.extra_body {
            payload.insert(k.clone(), v.clone());
        }
        if let Some(extras) = extra_body {
            for (k, v) in extras {
                payload.insert(k.clone(), v.clone());
            }
        }

        Value::Object(payload)
    }

    /// Converts a single [`LLMMessage`] into the wire-format JSON object.
    pub fn message_payload(&self, message: &LLMMessage) -> Value {
        let mut payload = message.to_value();

        if let Some(content) = payload.get_mut("content") {
            if let Value::Array(blocks) = content {
                let mut resolved: Vec<Value> = Vec::with_capacity(blocks.len());
                for block in blocks.iter() {
                    if let Some(replacement) = self.resolve_block(block) {
                        resolved.push(replacement);
                    }
                }
                *content = Value::Array(resolved);
            }
        }
        payload
    }

    fn resolve_block(&self, block: &Value) -> Option<Value> {
        let block_type = block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if block_type == FILE_ATTACHMENT_CONTENT_BLOCK_TYPE {
            let file_attachment = block.get("file_attachment")?;
            let text = self.file_attachment_text(file_attachment);
            if text.is_empty() {
                return None;
            }
            return Some(serde_json::json!({
                "type": "text",
                "text": text,
            }));
        }
        if block_type == "image_url" {
            let image_url = block.get("image_url")?;
            let url = self.resolve_image_url(image_url);
            return Some(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": url },
            }));
        }
        Some(block.clone())
    }

    fn resolve_image_url(&self, image_url: &Value) -> String {
        let attachment_id = image_url
            .get("attachment_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let raw_url = image_url
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();

        let effective_id = if !attachment_id.is_empty() {
            attachment_id
        } else if let Some(stripped) = raw_url.strip_prefix(ATTACHMENT_URL_PREFIX) {
            stripped.to_string()
        } else {
            String::new()
        };

        if !effective_id.is_empty() {
            match &self.attachment_store {
                Some(store) => store
                    .image_attachment_data_url(&effective_id)
                    .unwrap_or_else(|_| raw_url.clone()),
                None => raw_url,
            }
        } else {
            raw_url
        }
    }

    fn file_attachment_text(&self, file_attachment: &Value) -> String {
        let summary = file_attachment_summary(file_attachment);
        if summary.is_empty() {
            return String::new();
        }
        format!(
            "The user attached a local file for this request.\n{}\nUse the available file or workspace tools if you need to inspect it.",
            summary
        )
    }

    /// Parses a JSON response body into an [`LLMResponse`].
    pub fn parse_response(&self, data: &Value) -> Result<LLMResponse, ProviderError> {
        let Some(choices) = data.get("choices").and_then(Value::as_array) else {
            return Err(ProviderError::Response(
                "LLM provider response is missing choices".to_string(),
            ));
        };
        if choices.is_empty() {
            return Err(ProviderError::Response(
                "LLM provider response has empty choices".to_string(),
            ));
        }
        let choice = &choices[0];
        let message_data = choice.get("message").cloned().unwrap_or(Value::Null);

        let mut tool_calls: Vec<ToolCall> = Vec::new();
        if let Some(arr) = message_data.get("tool_calls").and_then(Value::as_array) {
            for item in arr {
                if !item.is_object() {
                    continue;
                }
                let function_data = item
                    .get("function")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                tool_calls.push(ToolCall {
                    id: item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    name: function_data
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    arguments: function_data
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }

        let raw_content = message_data
            .get("content")
            .cloned()
            .unwrap_or(Value::String(String::new()));
        let (cleaned_content, tag_reasoning) = extract_thinking_tags_from_content(&raw_content);
        let (reasoning_content, reasoning_field) = extract_reasoning_content(&message_data);
        let reasoning_content = if !reasoning_content.is_empty() {
            reasoning_content
        } else {
            tag_reasoning
        };

        let role_str = message_data
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("assistant");
        let role = parse_role(role_str);

        let normalized_content = normalize_message_content(&cleaned_content);
        let assistant_message = LLMMessage {
            role,
            content: normalized_content,
            name: None,
            tool_call_id: None,
            tool_calls: tool_calls.clone(),
            reasoning_content,
            reasoning_field,
        };

        let usage = LLMUsage::from_value(data.get("usage"));

        Ok(LLMResponse {
            message: assistant_message,
            model: data
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(&self.settings.model)
                .to_string(),
            finish_reason: choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .map(|s| s.to_string()),
            usage,
            tool_calls,
            raw_response: Some(data.clone()),
        })
    }

    /// Builds the HTTP headers for a request.
    pub fn request_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.settings.api_key))
                .unwrap_or(HeaderValue::from_static("")),
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        for (k, v) in &self.settings.extra_headers {
            if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
                headers.insert(name, value);
            }
        }
        headers
    }

    /// Sends a JSON POST and returns the parsed JSON response.
    pub async fn post_json(&self, payload: &Value) -> Result<Value, ProviderError> {
        let url = format!("{}/chat/completions", self.settings.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .headers(self.request_headers())
            .json(payload)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let detail = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read body>".to_string());
            return Err(ProviderError::HttpStatus {
                status: status.as_u16(),
                detail,
            });
        }
        response
            .json::<Value>()
            .await
            .map_err(|e| ProviderError::Response(format!("invalid JSON: {e}")))
    }

    /// Streams SSE chunks from a chat completions request and returns a
    /// stream of text chunks. The returned stream does not surface HTTP
    /// errors as `Err` items (the Python implementation raises an
    /// exception on the first error); callers should use
    /// [`Self::post_json`] for error-aware access.
    pub fn stream_text_chunks<'a>(
        &'a self,
        payload: &'a Value,
    ) -> impl Stream<Item = String> + 'a {
        let url = format!(
            "{}/chat/completions",
            self.settings.base_url.trim_end_matches('/')
        );
        let client = self.client.clone();
        let headers = self.request_headers();
        async_stream::stream! {
            let response = match client
                .post(&url)
                .headers(headers)
                .json(payload)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "stream send failed");
                    return;
                }
            };
            let status = response.status();
            if !status.is_success() {
                let detail = response.text().await.unwrap_or_default();
                tracing::warn!(status = %status.as_u16(), detail = %detail, "stream returned non-2xx");
                return;
            }
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "stream chunk error");
                        return;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line: String = buffer.drain(..=pos).collect();
                    let line = line.trim_end_matches(['\n', '\r']).to_string();
                    if let Some(text) = parse_sse_line(&line) {
                        if !text.is_empty() {
                            yield text;
                        }
                    }
                }
            }
        }
    }

    /// Identical to [`Self::stream_text_chunks`] but the payload is owned,
    /// so the returned stream does not need to borrow it.
    pub fn stream_text_chunks_owned(
        &self,
        payload: Value,
    ) -> impl Stream<Item = String> + '_ {
        let url = format!(
            "{}/chat/completions",
            self.settings.base_url.trim_end_matches('/')
        );
        let client = self.client.clone();
        let headers = self.request_headers();
        async_stream::stream! {
            let response = match client
                .post(&url)
                .headers(headers)
                .json(&payload)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "stream send failed");
                    return;
                }
            };
            let status = response.status();
            if !status.is_success() {
                let detail = response.text().await.unwrap_or_default();
                tracing::warn!(status = %status.as_u16(), detail = %detail, "stream returned non-2xx");
                return;
            }
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "stream chunk error");
                        return;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line: String = buffer.drain(..=pos).collect();
                    let line = line.trim_end_matches(['\n', '\r']).to_string();
                    if let Some(text) = parse_sse_line(&line) {
                        if !text.is_empty() {
                            yield text;
                        }
                    }
                }
            }
        }
    }

    /// Parses a single SSE line (already stripped of `\r\n`).
    pub fn parse_sse_line(&self, line: &str) -> Option<String> {
        parse_sse_line(line)
    }
}

#[async_trait]
impl LLMProvider for OpenAICompatibleProvider {
    async fn generate(
        &self,
        messages: &[LLMMessage],
        tools: Option<&[LLMTool]>,
        tool_choice: Option<&ToolChoice>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        extra_body: Option<&HashMap<String, Value>>,
    ) -> Result<LLMResponse, ProviderError> {
        let payload = self.build_payload(messages, tools, tool_choice, temperature, max_tokens, extra_body, false);
        let data = self.post_json(&payload).await?;
        self.parse_response(&data)
    }

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
        let has_tools = tools.map(|t| !t.is_empty()).unwrap_or(false);
        if has_tools {
            // Mirror Python: when tools are present, fall back to a
            // non-streaming call and yield the full content as one chunk.
            let response = self
                .generate(messages, tools, tool_choice, temperature, max_tokens, extra_body)
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
            return Ok(Box::new(stream));
        }
        let payload = self.build_payload(messages, None, tool_choice, temperature, max_tokens, extra_body, true);
        // Adapt the text-only stream into a `Result<_, ProviderError>` one.
        // We use the owned-payload variant so the returned stream does
        // not need to borrow `payload` (and outlive it).
        let text_stream = self.stream_text_chunks_owned(payload);
        let mapped = text_stream.map(Ok::<_, ProviderError>);
        Ok(Box::new(Box::pin(mapped)))
    }
}

// ---------------------------------------------------------------------------
// Helpers exposed for tests / external callers
// ---------------------------------------------------------------------------

/// Splits the supplied `attachment_url` into `(attachment_id, raw_url)`.
pub fn split_attachment_url(attachment_url: &str) -> Option<(String, String)> {
    attachment_url
        .strip_prefix(ATTACHMENT_URL_PREFIX)
        .map(|stripped| (stripped.to_string(), attachment_url.to_string()))
}

/// Builds an `ImageUrlPayload` from a URL string, used by callers that
/// synthesize message content from raw URLs.
pub fn image_payload_from_url(url: impl Into<String>) -> ImageUrlPayload {
    ImageUrlPayload {
        url: url.into(),
        preview_url: None,
        attachment_id: None,
    }
}

/// Builds a `FileAttachmentPayload` from an attachment id.
pub fn file_payload_from_id(id: impl Into<String>) -> FileAttachmentPayload {
    FileAttachmentPayload {
        name: "file".to_string(),
        attachment_id: Some(id.into()),
        download_url: None,
        workspace_path: None,
        content_type: None,
        size_bytes: None,
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Merges consecutive leading system messages into one. Some backends (e.g.
/// vLLM) reject requests that contain more than one system message or a
/// system message that isn't at position 0.
pub fn merge_system_messages(messages: &[LLMMessage]) -> Vec<LLMMessage> {
    if messages.is_empty() {
        return messages.to_vec();
    }
    let mut system_parts: Vec<String> = Vec::new();
    let mut rest_start = 0;
    for (i, msg) in messages.iter().enumerate() {
        if msg.role == MessageRole::System {
            system_parts.push(message_content_to_text(&msg.content));
            rest_start = i + 1;
        } else {
            break;
        }
    }
    if system_parts.len() <= 1 {
        return messages.to_vec();
    }
    let merged = LLMMessage::text(MessageRole::System, system_parts.join("\n\n"));
    let mut out = Vec::with_capacity(1 + messages.len() - rest_start);
    out.push(merged);
    out.extend(messages[rest_start..].iter().cloned());
    out
}

fn parse_role(value: &str) -> MessageRole {
    match value.trim().to_lowercase().as_str() {
        "system" => MessageRole::System,
        "user" => MessageRole::User,
        "tool" => MessageRole::Tool,
        _ => MessageRole::Assistant,
    }
}

fn extract_reasoning_content(data: &Value) -> (String, ReasoningField) {
    if let Some(obj) = data.as_object() {
        // Look up the fields in priority order. Iterating over the map
        // would let serde_json's ordering (alphabetical) pick the wrong
        // field, so we probe each known field directly.
        for field in REASONING_RESPONSE_FIELDS {
            if let Some(value) = obj.get(*field).and_then(Value::as_str) {
                if !value.is_empty() {
                    return (value.to_string(), ReasoningField::from_field_name(field));
                }
            }
        }
    }
    (String::new(), ReasoningField::default())
}

fn extract_thinking_tags_from_content(value: &Value) -> (Value, String) {
    match value {
        Value::String(s) => {
            let re = match regex::Regex::new(THINKING_TAG_PATTERN) {
                Ok(re) => re,
                Err(_) => return (value.clone(), String::new()),
            };
            if !re.is_match(s) {
                if s.contains("</think>") {
                    let cleaned = regex::Regex::new(r"</think>\s*$")
                        .map(|r| r.replace_all(s, "").trim().to_string())
                        .unwrap_or_else(|_| s.to_string());
                    return (Value::String(cleaned), String::new());
                }
                return (value.clone(), String::new());
            }
            let reasoning_content: String = re
                .captures_iter(s)
                .filter_map(|caps| caps.get(1).map(|c| c.as_str().trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let mut cleaned = re.replace_all(s, "").to_string();
            if let Ok(tail) = regex::Regex::new(r"</think>\s*$") {
                cleaned = tail.replace_all(&cleaned, "").trim().to_string();
            }
            (Value::String(cleaned), reasoning_content)
        }
        Value::Array(items) => {
            let mut cleaned_blocks: Vec<Value> = Vec::new();
            let mut reasoning_parts: Vec<String> = Vec::new();
            for block in items {
                if !block.is_object() {
                    cleaned_blocks.push(block.clone());
                    continue;
                }
                let block_type = block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                match block_type.as_str() {
                    "think" => {
                        if let Some(t) = block.get("think").and_then(Value::as_str) {
                            let t = t.trim();
                            if !t.is_empty() {
                                reasoning_parts.push(t.to_string());
                            }
                        }
                    }
                    "reasoning" => {
                        let text = block
                            .get("reasoning")
                            .or_else(|| block.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !text.is_empty() {
                            reasoning_parts.push(text);
                        }
                    }
                    _ => cleaned_blocks.push(block.clone()),
                }
            }
            (Value::Array(cleaned_blocks), reasoning_parts.join("\n"))
        }
        _ => (value.clone(), String::new()),
    }
}

fn parse_sse_line(line: &str) -> Option<String> {
    let payload_text = line.strip_prefix("data:")?.trim();
    if payload_text.is_empty() || payload_text == "[DONE]" {
        return None;
    }
    let data: Value = match serde_json::from_str(payload_text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, line = payload_text, "invalid stream JSON");
            return None;
        }
    };
    if let Some(err) = data.get("error") {
        let detail = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(payload_text);
        tracing::warn!(detail = %detail, "stream returned error payload");
        return None;
    }
    let choice = data
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())?;
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        tracing::warn!("stream hit max_tokens limit");
    }
    let content = choice.get("delta")?.get("content")?;
    match content {
        Value::String(s) => {
            let (cleaned, _reasoning) =
                extract_thinking_tags_from_content(&Value::String(s.clone()));
            if let Value::String(s) = cleaned {
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echobot_core::models::{MessageContent, ToolCall};

    fn settings() -> OpenAICompatibleSettings {
        OpenAICompatibleSettings {
            api_key: "k".to_string(),
            model: "m".to_string(),
            base_url: "https://example.com/v1".to_string(),
            timeout: std::time::Duration::from_secs(30),
            extra_headers: HashMap::new(),
            extra_body: HashMap::new(),
        }
    }

    #[test]
    fn build_payload_minimal() {
        let provider = OpenAICompatibleProvider::new(settings(), None).unwrap();
        let messages = vec![LLMMessage::text(MessageRole::User, "hi")];
        let payload = provider.build_payload(&messages, None, None, None, None, None, false);
        assert_eq!(payload["model"], "m");
        assert_eq!(payload["messages"][0]["content"], "hi");
        assert!(payload.get("stream").is_none());
    }

    #[test]
    fn build_payload_with_stream() {
        let provider = OpenAICompatibleProvider::new(settings(), None).unwrap();
        let messages = vec![LLMMessage::text(MessageRole::User, "hi")];
        let payload = provider.build_payload(&messages, None, None, None, None, None, true);
        assert_eq!(payload["stream"], true);
    }

    #[test]
    fn build_payload_merges_extra_body() {
        let mut s = settings();
        s.extra_body.insert("foo".into(), serde_json::json!(42));
        let provider = OpenAICompatibleProvider::new(s, None).unwrap();
        let messages = vec![LLMMessage::text(MessageRole::User, "hi")];
        let mut extras = HashMap::new();
        extras.insert("bar".into(), serde_json::json!("x"));
        let payload = provider.build_payload(&messages, None, None, None, None, Some(&extras), false);
        assert_eq!(payload["foo"], 42);
        assert_eq!(payload["bar"], "x");
    }

    #[test]
    fn merges_consecutive_system_messages() {
        let messages = vec![
            LLMMessage::text(MessageRole::System, "first"),
            LLMMessage::text(MessageRole::System, "second"),
            LLMMessage::text(MessageRole::User, "hi"),
        ];
        let merged = merge_system_messages(&messages);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].role, MessageRole::System);
        let combined = match &merged[0].content {
            MessageContent::Text(s) => s.clone(),
            _ => panic!("expected text"),
        };
        assert!(combined.contains("first"));
        assert!(combined.contains("second"));
    }

    #[test]
    fn parses_response_with_tool_calls() {
        let provider = OpenAICompatibleProvider::new(settings(), None).unwrap();
        let body = serde_json::json!({
            "model": "m",
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": "thinking",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "echo", "arguments": "{}" }
                    }]
                }
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 2 }
        });
        let response = provider.parse_response(&body).unwrap();
        assert_eq!(response.message.role, MessageRole::Assistant);
        assert_eq!(response.tool_calls, vec![ToolCall::new("call_1", "echo", "{}")]);
        assert_eq!(response.usage.prompt_tokens, 1);
    }

    #[test]
    fn extracts_thinking_tags_from_string_content() {
        let value = serde_json::json!("<think>reasoning</think>\n\nanswer");
        let (cleaned, reasoning) = extract_thinking_tags_from_content(&value);
        assert_eq!(cleaned, serde_json::json!("answer"));
        assert_eq!(reasoning, "reasoning");
    }

    #[test]
    fn extract_reasoning_content_prefers_first_known_field() {
        // `REASONING_RESPONSE_FIELDS` iterates `reasoning_content` first,
        // so even when `reasoning` is also present, the explicit
        // `reasoning_content` value wins.
        let body = serde_json::json!({"reasoning": "hello", "reasoning_content": "ignored"});
        let (text, field) = extract_reasoning_content(&body);
        assert_eq!(text, "ignored");
        assert_eq!(field, ReasoningField::ReasoningContent);

        let body = serde_json::json!({"reasoning": "hello"});
        let (text, field) = extract_reasoning_content(&body);
        assert_eq!(text, "hello");
        assert_eq!(field, ReasoningField::Reasoning);
    }

    #[test]
    fn parse_sse_line_extracts_text() {
        let line = r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#;
        assert_eq!(parse_sse_line(line).as_deref(), Some("hi"));
        assert!(parse_sse_line("data: [DONE]").is_none());
    }
}
