//! Strongly-typed LLM message / response models.
//!
//! These mirror the Python `echobot/models.py` dataclasses. Wire-format field
//! names match the OpenAI / OpenAI-compatible chat completions protocol.
//!
//! Content blocks that are genuinely dynamic (image payloads, file attachments)
//! use [`serde_json::Value`]; everything else is a typed enum variant.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ModelError, Result};

/// Identifier strings for content block types in the OpenAI-compatible wire
/// format. Match the Python constants in `echobot/models.py`.
pub const TEXT_CONTENT_BLOCK_TYPE: &str = "text";
pub const IMAGE_URL_CONTENT_BLOCK_TYPE: &str = "image_url";
pub const FILE_ATTACHMENT_CONTENT_BLOCK_TYPE: &str = "file_attachment";

/// A single role an [`LLMMessage`] may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    /// Returns the role as the lowercase string the wire format expects.
    pub fn as_str(self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }
}

/// Which field the provider uses to surface reasoning text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningField {
    /// `reasoning_content` (DeepSeek-style).
    #[default]
    ReasoningContent,
    /// `reasoning` (Anthropic-style).
    Reasoning,
}

impl ReasoningField {
    /// Returns the field name as it appears on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningField::ReasoningContent => "reasoning_content",
            ReasoningField::Reasoning => "reasoning",
        }
    }

    /// Parses a field name; falls back to [`ReasoningField::ReasoningContent`].
    pub fn from_field_name(name: &str) -> Self {
        match name {
            "reasoning" => ReasoningField::Reasoning,
            _ => ReasoningField::ReasoningContent,
        }
    }
}

// ---------------------------------------------------------------------------
// Content blocks
// ---------------------------------------------------------------------------

/// A typed text content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}

impl TextContentBlock {
    /// Builds a new text block; returns `None` if the text is empty after
    /// trimming (matches the Python behavior of dropping empty blocks).
    pub fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        if text.trim().is_empty() {
            return None;
        }
        Some(Self {
            kind: TEXT_CONTENT_BLOCK_TYPE.to_string(),
            text,
        })
    }
}

/// An `image_url` content block. The inner `url` / `preview_url` /
/// `attachment_id` shape mirrors the Python dynamic dict exactly so it
/// serialises to the same JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageUrlBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub image_url: ImageUrlPayload,
}

/// Dynamic image URL payload (`url`, optional `preview_url`, optional
/// `attachment_id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ImageUrlPayload {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<String>,
}

/// A `file_attachment` content block. The payload is intentionally dynamic
/// because backends may add bespoke fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAttachmentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub file_attachment: FileAttachmentPayload,
}

/// File attachment payload sent to the LLM in a content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileAttachmentPayload {
    #[serde(default = "default_file_name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

fn default_file_name() -> String {
    "file".to_string()
}

/// A free-form content block. Anything we don't recognize round-trips as raw
/// JSON so we never lose information when talking to a non-standard backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawContentBlock(Value);

impl RawContentBlock {
    /// Wraps a raw [`Value`].
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    /// Borrows the underlying JSON.
    pub fn value(&self) -> &Value {
        &self.0
    }

    /// Consumes the wrapper and returns the underlying JSON.
    pub fn into_value(self) -> Value {
        self.0
    }
}

/// Strongly-typed union of all known content block shapes. Use
/// [`MessageContentBlock::from_value`] to decode arbitrary blocks from JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageContentBlock {
    /// A plain text block.
    Text(TextContentBlock),
    /// An `image_url` block.
    ImageUrl(ImageUrlBlock),
    /// A `file_attachment` block.
    FileAttachment(FileAttachmentBlock),
    /// Any other block — stored verbatim.
    Raw(RawContentBlock),
}

impl MessageContentBlock {
    /// Returns the block's `type` field, or `"unknown"` for raw blocks without
    /// a `type`.
    pub fn block_type(&self) -> &str {
        match self {
            MessageContentBlock::Text(b) => &b.kind,
            MessageContentBlock::ImageUrl(b) => &b.kind,
            MessageContentBlock::FileAttachment(b) => &b.kind,
            MessageContentBlock::Raw(v) => v
                .0
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        }
    }

    /// Serializes the block to its wire-format JSON value.
    pub fn to_value(&self) -> Value {
        match self {
            MessageContentBlock::Text(b) => serde_json::to_value(b).unwrap_or(Value::Null),
            MessageContentBlock::ImageUrl(b) => serde_json::to_value(b).unwrap_or(Value::Null),
            MessageContentBlock::FileAttachment(b) => {
                serde_json::to_value(b).unwrap_or(Value::Null)
            }
            MessageContentBlock::Raw(v) => v.0.clone(),
        }
    }

    /// Parses a block from a JSON value. Returns `None` for blocks that fail
    /// to validate (e.g. missing `type`, empty `text`).
    pub fn from_value(value: Value) -> Option<Self> {
        let mut map = match value {
            Value::Object(m) => m,
            _ => return None,
        };
        let block_type = map
            .remove("type")
            .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
            .unwrap_or_default();
        if block_type.is_empty() {
            return None;
        }
        match block_type.as_str() {
            TEXT_CONTENT_BLOCK_TYPE => {
                let text = map
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if text.is_empty() {
                    return None;
                }
                let mut obj = serde_json::Map::new();
                obj.insert("type".into(), Value::String(block_type));
                obj.insert("text".into(), Value::String(text));
                let parsed: TextContentBlock =
                    serde_json::from_value(Value::Object(obj)).ok()?;
                Some(MessageContentBlock::Text(parsed))
            }
            IMAGE_URL_CONTENT_BLOCK_TYPE => {
                let payload = map
                    .remove("image_url")
                    .and_then(|v| normalize_image_input(&v))?;
                let mut obj = serde_json::Map::new();
                obj.insert("type".into(), Value::String(block_type));
                obj.insert("image_url".into(), serde_json::to_value(payload).ok()?);
                let parsed: ImageUrlBlock =
                    serde_json::from_value(Value::Object(obj)).ok()?;
                Some(MessageContentBlock::ImageUrl(parsed))
            }
            FILE_ATTACHMENT_CONTENT_BLOCK_TYPE => {
                let payload = map
                    .remove("file_attachment")
                    .and_then(|v| normalize_file_attachment_input(&v))?;
                let mut obj = serde_json::Map::new();
                obj.insert("type".into(), Value::String(block_type));
                obj.insert(
                    "file_attachment".into(),
                    serde_json::to_value(payload).ok()?,
                );
                let parsed: FileAttachmentBlock =
                    serde_json::from_value(Value::Object(obj)).ok()?;
                Some(MessageContentBlock::FileAttachment(parsed))
            }
            _ => {
                let mut obj = serde_json::Map::new();
                obj.insert("type".into(), Value::String(block_type));
                for (k, v) in map {
                    obj.insert(k, v);
                }
                Some(MessageContentBlock::Raw(RawContentBlock(Value::Object(
                    obj,
                ))))
            }
        }
    }
}

/// Content of an LLM message. Either a plain string or a list of blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageContent {
    /// Plain text.
    Text(String),
    /// List of typed / raw content blocks.
    Blocks(Vec<MessageContentBlock>),
}

impl Serialize for MessageContent {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            MessageContent::Text(s) => serializer.serialize_str(s),
            MessageContent::Blocks(blocks) => {
                use serde::ser::SerializeSeq;
                let mut seq = serializer.serialize_seq(Some(blocks.len()))?;
                for block in blocks {
                    seq.serialize_element(&block.to_value())?;
                }
                seq.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for MessageContent {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(normalize_message_content(&value))
    }
}

impl MessageContent {
    /// Returns the plain-text form: either the string itself, or a
    /// concatenation of all `text` blocks (images become `[image]`, files
    /// become a one-line summary).
    pub fn to_text(&self) -> String {
        message_content_to_text(self)
    }

    /// Returns the list of image URLs in the content, in order.
    pub fn image_urls(&self) -> Vec<String> {
        message_content_image_urls(self)
    }

    /// Returns the list of file attachment payloads in the content, in order.
    pub fn file_attachments(&self) -> Vec<FileAttachmentPayload> {
        message_content_file_attachments(self)
    }

    /// True if the content has no visible text, no images and no files.
    pub fn is_empty(&self) -> bool {
        is_message_content_empty(self)
    }
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Text(String::new())
    }
}

impl From<&str> for MessageContent {
    fn from(value: &str) -> Self {
        MessageContent::Text(value.to_string())
    }
}

impl From<String> for MessageContent {
    fn from(value: String) -> Self {
        MessageContent::Text(value)
    }
}

// ---------------------------------------------------------------------------
// Tool call / tool / response / usage
// ---------------------------------------------------------------------------

/// A single tool call the model asked the client to invoke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolCall {
    /// Unique id assigned by the model.
    pub id: String,
    /// Function name to call.
    pub name: String,
    /// Pre-serialised JSON arguments (kept as `String` to match Python).
    pub arguments: String,
}

impl ToolCall {
    /// Builds a new [`ToolCall`].
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    /// Returns the wire-format JSON value (matches the Python `to_dict`).
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "type": "function",
            "function": {
                "name": self.name,
                "arguments": self.arguments,
            }
        })
    }
}

/// A single message in the chat history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LLMMessage {
    /// The role of the speaker.
    pub role: MessageRole,

    /// Plain text or content blocks.
    #[serde(default)]
    pub content: MessageContent,

    /// Optional name (rarely used; for function-calling scenarios).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Tool-call id this message is responding to (for `role == "tool"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Tool calls the assistant wants to invoke.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,

    /// Reasoning text the model produced alongside its answer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning_content: String,

    /// Which field name to use when echoing reasoning back to the wire.
    #[serde(default, skip_serializing_if = "is_default_reasoning_field")]
    pub reasoning_field: ReasoningField,
}

fn is_default_reasoning_field(field: &ReasoningField) -> bool {
    *field == ReasoningField::default()
}

impl LLMMessage {
    /// Builds a simple text message.
    pub fn text(role: MessageRole, text: impl Into<String>) -> Self {
        Self {
            role,
            content: MessageContent::Text(text.into()),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: String::new(),
            reasoning_field: ReasoningField::default(),
        }
    }

    /// Builds an assistant message with tool calls.
    pub fn assistant_with_tool_calls(
        text: impl Into<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: MessageContent::Text(text.into()),
            name: None,
            tool_call_id: None,
            tool_calls,
            reasoning_content: String::new(),
            reasoning_field: ReasoningField::default(),
        }
    }

    /// Convenience: the plain-text representation of the content.
    pub fn content_text(&self) -> String {
        message_content_to_text(&self.content)
    }

    /// Serializes the message to its wire-format JSON value.
    pub fn to_value(&self) -> Value {
        let mut data = serde_json::Map::new();
        data.insert("role".into(), Value::String(self.role.as_str().to_string()));
        let content_value = match &self.content {
            MessageContent::Text(s) => Value::String(s.clone()),
            MessageContent::Blocks(blocks) => {
                let items: Vec<Value> = blocks.iter().map(|b| b.to_value()).collect();
                Value::Array(items)
            }
        };
        data.insert("content".into(), content_value);

        if let Some(name) = &self.name {
            if !name.is_empty() {
                data.insert("name".into(), Value::String(name.clone()));
            }
        }
        if let Some(id) = &self.tool_call_id {
            if !id.is_empty() {
                data.insert("tool_call_id".into(), Value::String(id.clone()));
            }
        }
        if !self.tool_calls.is_empty() {
            let arr: Vec<Value> = self.tool_calls.iter().map(|t| t.to_value()).collect();
            data.insert("tool_calls".into(), Value::Array(arr));
        }
        if self.role == MessageRole::Assistant && !self.reasoning_content.is_empty() {
            data.insert(
                self.reasoning_field.as_str().into(),
                Value::String(self.reasoning_content.clone()),
            );
        }
        Value::Object(data)
    }
}

/// Definition of a tool the model may call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LLMTool {
    /// Tool (function) name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON schema for the tool's parameters.
    pub parameters: Value,
}

impl LLMTool {
    /// Builds a new tool definition.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// Serializes to wire format (`{type: "function", function: {...}}`).
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }
}

/// Token usage reported by the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LLMUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub prompt_cache_hit_tokens: u64,
    #[serde(default)]
    pub prompt_cache_miss_tokens: u64,
}

impl LLMUsage {
    /// Parses usage info from a provider response. Tolerates both
    /// `prompt_tokens` (OpenAI) and `input_tokens` (Anthropic) etc.
    pub fn from_value(data: Option<&Value>) -> Self {
        let Some(data) = data else {
            return Self::default();
        };
        let Some(obj) = data.as_object() else {
            return Self::default();
        };

        let prompt_tokens = first_usage_int(obj, &["prompt_tokens", "input_tokens"]).unwrap_or(0);
        let completion_tokens =
            first_usage_int(obj, &["completion_tokens", "output_tokens"]).unwrap_or(0);
        let total_tokens = usage_int(obj, "total_tokens")
            .unwrap_or_else(|| prompt_tokens + completion_tokens);

        let prompt_cache_hit_tokens = usage_int(obj, "prompt_cache_hit_tokens").unwrap_or_else(|| {
            nested_usage_int(obj, "prompt_tokens_details", "cached_tokens")
                .or_else(|| nested_usage_int(obj, "input_tokens_details", "cached_tokens"))
                .unwrap_or(0)
        });

        let prompt_cache_miss_tokens = usage_int(obj, "prompt_cache_miss_tokens")
            .unwrap_or_else(|| prompt_tokens.saturating_sub(prompt_cache_hit_tokens));

        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            prompt_cache_hit_tokens,
            prompt_cache_miss_tokens,
        }
    }

    /// Hit rate as a percentage rounded to two decimals, or `None` if there
    /// were no prompt tokens to measure against.
    pub fn prompt_cache_hit_rate_percent(&self) -> Option<f64> {
        if self.prompt_tokens == 0 {
            return None;
        }
        let rate = (self.prompt_cache_hit_tokens as f64 / self.prompt_tokens as f64) * 100.0;
        Some((rate * 100.0).round() / 100.0)
    }

    /// Serializes to the same shape as the Python `to_dict` (including the
    /// computed hit rate percent).
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "total_tokens": self.total_tokens,
            "prompt_cache_hit_tokens": self.prompt_cache_hit_tokens,
            "prompt_cache_miss_tokens": self.prompt_cache_miss_tokens,
            "prompt_cache_hit_rate_percent": self.prompt_cache_hit_rate_percent(),
        })
    }
}

fn first_usage_int(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(v) = usage_int(obj, key) {
            return Some(v);
        }
    }
    None
}

fn usage_int(obj: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    obj.get(key).and_then(|v| v.as_u64())
}

fn nested_usage_int(
    obj: &serde_json::Map<String, Value>,
    outer: &str,
    inner: &str,
) -> Option<u64> {
    let nested = obj.get(outer)?.as_object()?;
    usage_int(nested, inner)
}

/// The full response from a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LLMResponse {
    /// The assistant's message.
    pub message: LLMMessage,
    /// Model identifier reported by the provider.
    pub model: String,
    /// Why the model stopped (e.g. `stop`, `length`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Reported token usage.
    #[serde(default)]
    pub usage: LLMUsage,
    /// Convenience duplicate of `message.tool_calls`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// The full raw provider response (kept for debugging / advanced users).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_response: Option<Value>,
}

impl LLMResponse {
    /// Convenience: the reasoning text in the assistant message.
    pub fn reasoning_content(&self) -> &str {
        &self.message.reasoning_content
    }
}

// ---------------------------------------------------------------------------
// Content access helpers
// ---------------------------------------------------------------------------

/// Accepts either a JSON object (with `url` / `preview_url` / `attachment_id`)
/// or a string. Returns the normalized payload, or `None` if the input was
/// empty/invalid.
pub fn normalize_image_input(value: &Value) -> Option<ImageUrlPayload> {
    match value {
        Value::Object(map) => {
            let url = map
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if url.is_empty() {
                return None;
            }
            let preview_url = map
                .get("preview_url")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let attachment_id = map
                .get("attachment_id")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Some(ImageUrlPayload {
                url,
                preview_url,
                attachment_id,
            })
        }
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                Some(ImageUrlPayload {
                    url: s.to_string(),
                    preview_url: None,
                    attachment_id: None,
                })
            }
        }
        _ => None,
    }
}

/// Accepts either a JSON object (with `attachment_id` etc.) or a string.
/// Returns the normalized payload, or `None` if the input was empty/invalid.
pub fn normalize_file_attachment_input(value: &Value) -> Option<FileAttachmentPayload> {
    match value {
        Value::Object(map) => {
            let attachment_id = map
                .get("attachment_id")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let download_url = map
                .get("download_url")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let name = map
                .get("name")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "file".to_string());
            let workspace_path = map
                .get("workspace_path")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let content_type = map
                .get("content_type")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let size_bytes = map
                .get("size_bytes")
                .and_then(|v| match v {
                    Value::Number(n) => n.as_u64(),
                    _ => None,
                })
                .filter(|n| *n > 0);

            if attachment_id.is_none()
                && download_url.is_none()
                && name == "file"
                && workspace_path.is_none()
            {
                return None;
            }

            Some(FileAttachmentPayload {
                name,
                attachment_id,
                download_url,
                workspace_path,
                content_type,
                size_bytes,
            })
        }
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                Some(FileAttachmentPayload {
                    name: "file".to_string(),
                    attachment_id: Some(s.to_string()),
                    download_url: None,
                    workspace_path: None,
                    content_type: None,
                    size_bytes: None,
                })
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Content access helpers
// ---------------------------------------------------------------------------

/// Returns the list of content blocks contained in `content`, normalizing
/// strings into a single text block.
pub fn message_content_blocks(content: &MessageContent) -> Vec<MessageContentBlock> {
    match content {
        MessageContent::Text(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Vec::new();
            }
            vec![MessageContentBlock::Text(TextContentBlock {
                kind: TEXT_CONTENT_BLOCK_TYPE.to_string(),
                text: trimmed.to_string(),
            })]
        }
        MessageContent::Blocks(blocks) => blocks.clone(),
    }
}

/// Returns the plain-text representation of `content`, joining text blocks
/// with `\n\n`. Images become `[image]`, files become a one-line summary.
pub fn message_content_to_text(content: &MessageContent) -> String {
    let blocks = message_content_blocks(content);
    let mut parts: Vec<String> = Vec::new();
    for block in &blocks {
        match block {
            MessageContentBlock::Text(t) => {
                let trimmed = t.text.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
            MessageContentBlock::ImageUrl(_) => {
                parts.push("[image]".to_string());
            }
            MessageContentBlock::FileAttachment(f) => {
                let summary = file_attachment_payload_summary(&f.file_attachment);
                if !summary.is_empty() {
                    parts.push(summary);
                }
            }
            MessageContentBlock::Raw(v) => {
                let block_type = v
                    .value()
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if !block_type.is_empty() {
                    parts.push(format!("[{block_type}]"));
                }
            }
        }
    }
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Returns all image URLs (in declaration order) inside `content`.
pub fn message_content_image_urls(content: &MessageContent) -> Vec<String> {
    let blocks = message_content_blocks(content);
    let mut urls = Vec::new();
    for block in &blocks {
        if let MessageContentBlock::ImageUrl(img) = block {
            let url = img.image_url.url.trim();
            if !url.is_empty() {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

/// Returns all file attachment payloads (in declaration order) inside `content`.
pub fn message_content_file_attachments(content: &MessageContent) -> Vec<FileAttachmentPayload> {
    let blocks = message_content_blocks(content);
    let mut out = Vec::new();
    for block in &blocks {
        if let MessageContentBlock::FileAttachment(f) = block {
            out.push(f.file_attachment.clone());
        }
    }
    out
}

/// Returns true if the content has no visible text, no images and no files.
pub fn is_message_content_empty(content: &MessageContent) -> bool {
    let blocks = message_content_blocks(content);
    if blocks.is_empty() {
        return true;
    }
    let text = message_content_to_text(content);
    let images = message_content_image_urls(content);
    let files = message_content_file_attachments(content);
    images.is_empty() && files.is_empty() && text.trim().is_empty()
}

/// Normalises message content into the canonical [`MessageContent`] form
/// (either a `Text` string with whitespace stripped or a `Blocks` list).
pub fn normalize_message_content(value: &Value) -> MessageContent {
    match value {
        Value::String(s) => MessageContent::Text(s.clone()),
        Value::Array(items) => {
            let mut blocks: Vec<MessageContentBlock> = Vec::new();
            for item in items {
                if let Some(block) = MessageContentBlock::from_value(item.clone()) {
                    blocks.push(block);
                }
            }
            MessageContent::Blocks(blocks)
        }
        Value::Null => MessageContent::Text(String::new()),
        other => MessageContent::Text(other.to_string()),
    }
}

/// Builds a list of `MessageContentBlock`s from text + image URLs + file
/// attachments. Returns [`MessageContent::Text`] if there are no images or
/// files (preserving the Python shortcut for the common case).
pub fn build_message_content(
    text: &str,
    image_urls: Option<&[Value]>,
    file_attachments: Option<&[Value]>,
) -> MessageContent {
    let cleaned_text = text.trim().to_string();
    let cleaned_images: Vec<ImageUrlPayload> = image_urls
        .unwrap_or(&[])
        .iter()
        .filter_map(normalize_image_input)
        .collect();
    let cleaned_files: Vec<FileAttachmentPayload> = file_attachments
        .unwrap_or(&[])
        .iter()
        .filter_map(normalize_file_attachment_input)
        .collect();

    if cleaned_images.is_empty() && cleaned_files.is_empty() {
        return MessageContent::Text(cleaned_text);
    }

    let mut blocks: Vec<MessageContentBlock> = Vec::new();
    if !cleaned_text.is_empty() {
        if let Some(block) = TextContentBlock::new(cleaned_text) {
            blocks.push(MessageContentBlock::Text(block));
        }
    }
    for file in cleaned_files {
        blocks.push(MessageContentBlock::FileAttachment(FileAttachmentBlock {
            kind: FILE_ATTACHMENT_CONTENT_BLOCK_TYPE.to_string(),
            file_attachment: file,
        }));
    }
    for image in cleaned_images {
        blocks.push(MessageContentBlock::ImageUrl(ImageUrlBlock {
            kind: IMAGE_URL_CONTENT_BLOCK_TYPE.to_string(),
            image_url: image,
        }));
    }
    MessageContent::Blocks(blocks)
}

/// Returns a one-line summary of a file attachment for log output and
/// fallback text rendering.
pub fn file_attachment_summary(value: &Value) -> String {
    let Some(normalized) = normalize_file_attachment_input(value) else {
        return String::new();
    };
    file_attachment_payload_summary(&normalized)
}

/// Same as [`file_attachment_summary`] but takes a typed payload directly.
pub fn file_attachment_payload_summary(payload: &FileAttachmentPayload) -> String {
    let name = if payload.name.trim().is_empty() {
        "file"
    } else {
        payload.name.trim()
    };
    let mut details = vec![name.to_string()];
    if let Some(p) = &payload.workspace_path {
        if !p.is_empty() {
            details.push(format!("path={p}"));
        }
    }
    if let Some(ct) = &payload.content_type {
        if !ct.is_empty() {
            details.push(format!("type={ct}"));
        }
    }
    if let Some(size) = payload.size_bytes {
        details.push(format!("size={size} bytes"));
    }
    format!("file: {}", details.join(" | "))
}

/// Converts a [`FileAttachmentPayload`] to its JSON value form.
pub fn file_attachment_payload_to_value(payload: &FileAttachmentPayload) -> Value {
    serde_json::to_value(payload).unwrap_or(Value::Null)
}

/// Image input accepted by [`build_message_content`]: either a URL string
/// or a JSON object.
pub type ImageInput = Value;

/// File input accepted by [`build_message_content`]: either an attachment id
/// string or a JSON object.
pub type FileInput = Value;

/// Validates that the supplied content block has a known shape. Used by code
/// paths that need to enforce a stricter contract than the dynamic JSON.
pub fn validate_block(block: &MessageContentBlock) -> Result<()> {
    if matches!(block, MessageContentBlock::Raw(_)) {
        return Err(Error::Model(ModelError::UnknownContentBlockType(
            block.block_type().to_string(),
        )));
    }
    Ok(())
}

/// Crate-local alias so `use crate::error::Error` works from this module.
use crate::error::Error;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_message_serializes_to_wire_format() {
        let msg = LLMMessage::text(MessageRole::User, "hello");
        let value = msg.to_value();
        assert_eq!(value["role"], "user");
        assert_eq!(value["content"], "hello");
        assert!(value.get("tool_calls").is_none());
        assert!(value.get("name").is_none());
    }

    #[test]
    fn assistant_message_with_tool_calls_serializes_correctly() {
        let msg = LLMMessage::assistant_with_tool_calls(
            "",
            vec![ToolCall::new("call_1", "ping", "{\"x\":1}")],
        );
        let value = msg.to_value();
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["tool_calls"][0]["function"]["name"], "ping");
    }

    #[test]
    fn reasoning_field_uses_configured_name() {
        let mut msg = LLMMessage::text(MessageRole::Assistant, "answer");
        msg.reasoning_content = "thought".to_string();
        msg.reasoning_field = ReasoningField::Reasoning;
        let value = msg.to_value();
        assert_eq!(value["reasoning"], "thought");
        assert!(value.get("reasoning_content").is_none());
    }

    #[test]
    fn build_message_content_shortcuts_to_string_when_no_media() {
        let c = build_message_content("hi", None, None);
        match c {
            MessageContent::Text(t) => assert_eq!(t, "hi"),
            _ => panic!("expected text content"),
        }
    }

    #[test]
    fn build_message_content_includes_images_and_files() {
        let images = vec![json!("https://example.com/a.png")];
        let files = vec![json!("att_123")];
        let c = build_message_content("look", Some(&images), Some(&files));
        let blocks = match c {
            MessageContent::Blocks(b) => b,
            _ => panic!("expected blocks"),
        };
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[0], MessageContentBlock::Text(_)));
        assert!(matches!(blocks[1], MessageContentBlock::FileAttachment(_)));
        assert!(matches!(blocks[2], MessageContentBlock::ImageUrl(_)));
    }

    #[test]
    fn content_to_text_includes_image_marker() {
        let images = vec![json!("https://example.com/a.png")];
        let c = build_message_content("hi", Some(&images), None);
        let text = c.to_text();
        assert!(text.contains("[image]"));
        assert!(text.contains("hi"));
    }

    #[test]
    fn usage_parses_openai_shape() {
        let v = json!({
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15,
            "prompt_tokens_details": {"cached_tokens": 3},
        });
        let u = LLMUsage::from_value(Some(&v));
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 5);
        assert_eq!(u.total_tokens, 15);
        assert_eq!(u.prompt_cache_hit_tokens, 3);
        assert_eq!(u.prompt_cache_miss_tokens, 7);
    }

    #[test]
    fn usage_parses_anthropic_shape() {
        let v = json!({
            "input_tokens": 7,
            "output_tokens": 4,
            "input_tokens_details": {"cached_tokens": 2},
        });
        let u = LLMUsage::from_value(Some(&v));
        assert_eq!(u.prompt_tokens, 7);
        assert_eq!(u.completion_tokens, 4);
        assert_eq!(u.prompt_cache_hit_tokens, 2);
        assert_eq!(u.prompt_cache_miss_tokens, 5);
    }
}
