//! `BaseTool` trait + `ToolRegistry` + `ToolResult` types.
//!
//! Ports the Python `echobot/tools/base.py` module. Tools are async and
//! receive a JSON [`serde_json::Value`] of arguments, returning either a
//! raw payload (string / number / bool / null / object / array) or a
//! [`ToolExecutionOutput`] that can carry promoted images, outbound
//! content blocks, trace events, and loop-control signals.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

use echobot_core::models::{
    is_message_content_empty, normalize_message_content, LLMMessage, LLMTool, MessageContent,
    MessageRole, ToolCall,
};
use echobot_core::Error;

// Re-export the core `ToolError` so downstream code can `use
// echobot_tools::ToolError` (or `echobot_tools::base::ToolError`)
// without depending on `echobot_core` directly.
pub use echobot_core::ToolError;

// ---------------------------------------------------------------------------
// Payload / output types
// ---------------------------------------------------------------------------

/// A raw tool output payload — anything JSON-serializable.
pub type ToolPayload = Value;

/// A single trace event emitted by a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolTraceEvent {
    /// Event name (e.g. `plan_updated`, `user_input_requested`).
    pub event: String,
    /// Free-form event payload.
    #[serde(default)]
    pub data: Value,
}

/// A signal that the tool loop should stop / pause after this tool returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolLoopControl {
    /// Loop action (e.g. `await_user_input`).
    pub action: String,
    /// Status string (defaults to `"completed"`).
    pub status: String,
    /// Response content to surface to the user.
    #[serde(default)]
    pub response_content: MessageContent,
    /// Additional metadata (e.g. the user-input request details).
    #[serde(default)]
    pub metadata: Value,
}

/// Structured tool output. Tools that need to surface images / files /
/// trace events / loop-control signals return this; the simpler
/// `ToolPayload` alias is also accepted via `From<Value>` conversion in
/// the [`BaseTool`] implementations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolExecutionOutput {
    /// The primary payload (will be wrapped in `{ok, result}` on the wire).
    pub data: ToolPayload,
    /// Image URLs promoted into the model context.
    #[serde(default)]
    pub promoted_image_urls: Vec<echobot_core::models::ImageUrlPayload>,
    /// Content blocks the runtime/CLI should deliver outbound (e.g. files
    /// to the user). Stored as their wire-format `Value` to avoid a
    /// hard `Serialize` bound on the core `MessageContentBlock` enum.
    #[serde(default)]
    pub outbound_content_blocks: Vec<Value>,
    /// Structured trace events the runtime/CLI should record.
    #[serde(default)]
    pub trace_events: Vec<ToolTraceEvent>,
    /// Optional loop-control signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<ToolLoopControl>,
}

impl ToolExecutionOutput {
    /// Builds an empty success result (for stubs that have nothing to say).
    pub fn empty() -> Self {
        Self {
            data: Value::Null,
            promoted_image_urls: Vec::new(),
            outbound_content_blocks: Vec::new(),
            trace_events: Vec::new(),
            control: None,
        }
    }

    /// Builds an output that carries just a JSON payload.
    pub fn from_payload(data: impl Into<Value>) -> Self {
        Self {
            data: data.into(),
            promoted_image_urls: Vec::new(),
            outbound_content_blocks: Vec::new(),
            trace_events: Vec::new(),
            control: None,
        }
    }
}

impl From<Value> for ToolExecutionOutput {
    fn from(value: Value) -> Self {
        Self::from_payload(value)
    }
}

/// The final result returned to the runtime after a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolResult {
    /// The original tool-call id.
    pub call_id: String,
    /// The tool name.
    pub tool_name: String,
    /// Pre-serialised `{ok, result}` payload.
    pub content: String,
    /// True if the tool errored out.
    #[serde(default)]
    pub is_error: bool,
    /// Image URLs promoted into the model context.
    #[serde(default)]
    pub promoted_image_urls: Vec<echobot_core::models::ImageUrlPayload>,
    /// Outbound content blocks the runtime/CLI should deliver.
    #[serde(default)]
    pub outbound_content_blocks: Vec<Value>,
    /// Structured trace events.
    #[serde(default)]
    pub trace_events: Vec<ToolTraceEvent>,
    /// Optional loop-control signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<ToolLoopControl>,
}

impl ToolResult {
    /// Builds a tool message from this result for the LLM conversation.
    pub fn to_message(&self) -> LLMMessage {
        LLMMessage {
            role: MessageRole::Tool,
            content: MessageContent::Text(self.content.clone()),
            name: None,
            tool_call_id: Some(self.call_id.clone()),
            tool_calls: Vec::new(),
            reasoning_content: String::new(),
            reasoning_field: Default::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// BaseTool trait
// ---------------------------------------------------------------------------

/// Common interface for all tools. Tools are `Send + Sync` so they can be
/// shared across the agent's async tasks. The agent is expected to box
/// concrete tools as `Arc<dyn BaseTool>`.
#[async_trait]
pub trait BaseTool: Send + Sync {
    /// The tool's unique name (matches what the LLM calls).
    fn name(&self) -> &str;

    /// Human-readable description shown to the LLM.
    fn description(&self) -> &str;

    /// JSON schema describing the tool's parameters.
    fn parameters(&self) -> Value;

    /// Convenience: builds the wire-format [`LLMTool`] definition.
    fn to_llm_tool(&self) -> LLMTool {
        LLMTool::new(self.name(), self.description(), self.parameters())
    }

    /// Executes the tool with the given arguments.
    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error>;
}

// ---------------------------------------------------------------------------
// ToolRegistry
// ---------------------------------------------------------------------------

/// Holds the registered tools keyed by name.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn BaseTool>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("names", &self.names())
            .finish()
    }
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
        }
    }
}

impl ToolRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Creates a registry pre-populated with `tools`.
    pub fn from_tools<I>(tools: I) -> Self
    where
        I: IntoIterator<Item = Arc<dyn BaseTool>>,
    {
        let mut registry = Self::new();
        for tool in tools {
            // Silently ignore duplicate registrations so callers can
            // safely union a few partial registries.
            let _ = registry.register(tool);
        }
        registry
    }

    /// Registers a tool, returning an error on duplicate names.
    pub fn register(&mut self, tool: Arc<dyn BaseTool>) -> Result<(), Error> {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            return Err(ToolError::Execution {
                name,
                message: "duplicate tool name".to_string(),
            }
            .into());
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Returns a clone of the tool by name, or `None`.
    pub fn get(&self, name: &str) -> Option<Arc<dyn BaseTool>> {
        self.tools.get(name).cloned()
    }

    /// Returns the registered tool names, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    /// Returns the LLM tool definitions for every registered tool.
    pub fn to_llm_tools(&self) -> Vec<LLMTool> {
        let mut tools: Vec<LLMTool> = self.tools.values().map(|t| t.to_llm_tool()).collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }

    /// Returns true when the registry has no tools.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Returns the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Executes a single tool call, returning a fully-populated
    /// [`ToolResult`].
    pub async fn execute(&self, tool_call: &ToolCall) -> ToolResult {
        let tool = match self.get(&tool_call.name) {
            Some(t) => t,
            None => {
                return self.error_result(
                    tool_call,
                    &format!("Tool not found: {}", tool_call.name),
                );
            }
        };

        let arguments = match parse_arguments(&tool_call.arguments) {
            Ok(v) => v,
            Err(message) => return self.error_result(tool_call, &message),
        };

        let output = match tool.run(arguments).await {
            Ok(o) => o,
            Err(err) => {
                return self.error_result(tool_call, &err.to_string());
            }
        };

        let execution = normalize_execution_output(output);
        ToolResult {
            call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content: build_payload(&execution.data, false),
            is_error: false,
            promoted_image_urls: execution.promoted_image_urls,
            outbound_content_blocks: execution.outbound_content_blocks,
            trace_events: execution.trace_events,
            control: execution.control,
        }
    }

    /// Executes a list of tool calls sequentially, short-circuiting on
    /// the first tool that emits a loop-control signal.
    pub async fn execute_tool_calls(&self, tool_calls: &[ToolCall]) -> Vec<ToolResult> {
        let mut results = Vec::with_capacity(tool_calls.len());
        for call in tool_calls {
            let result = self.execute(call).await;
            let had_control = result.control.is_some();
            results.push(result);
            if had_control {
                break;
            }
        }
        results
    }

    fn error_result(&self, tool_call: &ToolCall, message: &str) -> ToolResult {
        let data = serde_json::json!({ "error": message });
        ToolResult {
            call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content: build_payload(&data, true),
            is_error: true,
            promoted_image_urls: Vec::new(),
            outbound_content_blocks: Vec::new(),
            trace_events: Vec::new(),
            control: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Parses the raw string `arguments` from a [`ToolCall`] into a JSON
/// object. Returns an error string (not a `Result`) so the caller can
/// surface it to the LLM verbatim.
pub fn parse_arguments(raw_arguments: &str) -> std::result::Result<Value, String> {
    let cleaned = raw_arguments.trim();
    if cleaned.is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    let parsed: Value = serde_json::from_str(cleaned)
        .map_err(|e| format!("Invalid JSON tool arguments: {}", e))?;
    if !parsed.is_object() {
        return Err("Tool arguments must be a JSON object".to_string());
    }
    Ok(parsed)
}

fn normalize_execution_output(output: ToolExecutionOutput) -> ToolExecutionOutput {
    let promoted_image_urls = output
        .promoted_image_urls
        .into_iter()
        .filter(|p| !p.url.trim().is_empty())
        .collect();
    let trace_events = output
        .trace_events
        .into_iter()
        .filter_map(|e| {
            let event = e.event.trim().to_string();
            if event.is_empty() {
                None
            } else {
                Some(ToolTraceEvent { event, data: e.data })
            }
        })
        .collect();
    let control = output.control.and_then(|c| {
        let action = c.action.trim().to_string();
        if action.is_empty() {
            None
        } else {
            let status = if c.status.trim().is_empty() {
                "completed".to_string()
            } else {
                c.status.trim().to_string()
            };
            let response_content = if is_message_content_empty(&c.response_content) {
                MessageContent::Text(String::new())
            } else {
                normalize_message_content(
                    &serde_json::to_value(&c.response_content).unwrap_or(Value::Null),
                )
            };
            Some(ToolLoopControl {
                action,
                status,
                response_content,
                metadata: c.metadata,
            })
        }
    });

    ToolExecutionOutput {
        data: output.data,
        promoted_image_urls,
        outbound_content_blocks: output.outbound_content_blocks,
        trace_events,
        control,
    }
}

/// Builds the wire-format payload string `{ok, result}` (or
/// `{ok: false, error}` when the tool errored and the data already
/// contains an `error` key).
pub fn build_payload(data: &Value, is_error: bool) -> String {
    let payload = if is_error {
        if let Some(obj) = data.as_object() {
            if let Some(err) = obj.get("error") {
                serde_json::json!({ "ok": false, "error": err })
            } else {
                serde_json::json!({ "ok": false, "result": data })
            }
        } else {
            serde_json::json!({ "ok": false, "result": data })
        }
    } else {
        serde_json::json!({ "ok": true, "result": data })
    };
    serde_json::to_string(&payload)
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"serialise\"}".to_string())
}

/// Helper for tools that need to read a required string argument.
pub fn require_string<'a>(args: &'a Value, key: &str) -> std::result::Result<&'a str, String> {
    let value = args
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim())
        .unwrap_or("");
    if value.is_empty() {
        Err(format!("{key} is required"))
    } else {
        Ok(value)
    }
}

/// Helper that returns the string at `key` or a default.
pub fn optional_string<'a>(args: &'a Value, key: &str, default: &'a str) -> &'a str {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
}

/// Helper that returns a positive `i64` argument or an error string.
pub fn require_positive_int(
    args: &Value,
    key: &str,
    default: i64,
) -> std::result::Result<i64, String> {
    let raw = match args.get(key) {
        None | Some(Value::Null) => default,
        Some(v) => v
            .as_i64()
            .ok_or_else(|| format!("{key} must be an integer"))?,
    };
    if raw <= 0 {
        return Err(format!("{key} must be greater than 0"));
    }
    Ok(raw)
}

/// Helper that returns a positive `f64` argument or an error string.
pub fn require_positive_float(
    args: &Value,
    key: &str,
    default: f64,
) -> std::result::Result<f64, String> {
    let raw = match args.get(key) {
        None | Some(Value::Null) => default,
        Some(v) => v
            .as_f64()
            .ok_or_else(|| format!("{key} must be a number"))?,
    };
    if raw <= 0.0 {
        return Err(format!("{key} must be greater than 0"));
    }
    Ok(raw)
}

/// Truncates a string to `max_chars`, returning the truncated text and
/// a `truncated` flag.
pub fn truncate_text(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let mut out = String::new();
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    (out, true)
}

// Tiny alias re-exports so downstream `use` statements can stay short.
pub type ImageUrl = echobot_core::models::ImageUrlPayload;
pub type FileAttachment = echobot_core::models::FileAttachmentPayload;

// Make sure the unused-import lint stays quiet even when downstream
// tools only use a subset of these re-exports.
#[allow(dead_code)]
fn _silence_unused() {
    let _ = Error::Tool(ToolError::NotFound("x".to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    /// A tiny custom tool used to exercise the registry.
    struct EchoTool;

    #[async_trait]
    impl BaseTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Returns its argument as a JSON payload."
        }
        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"],
            })
        }
        async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
            let text = require_string(&arguments, "text")
                .map_err(|e| Error::Tool(ToolError::Execution { name: "echo".into(), message: e }))?;
            Ok(ToolExecutionOutput::from_payload(json!({ "echo": text })))
        }
    }

    /// Second tool used to verify multiple registrations.
    struct ReverseTool;

    #[async_trait]
    impl BaseTool for ReverseTool {
        fn name(&self) -> &str {
            "reverse"
        }
        fn description(&self) -> &str {
            "Reverses the input text."
        }
        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"],
            })
        }
        async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
            let text = require_string(&arguments, "text")
                .map_err(|e| Error::Tool(ToolError::Execution { name: "reverse".into(), message: e }))?;
            Ok(ToolExecutionOutput::from_payload(json!({
                "reversed": text.chars().rev().collect::<String>()
            })))
        }
    }

    #[test]
    fn registry_starts_empty() {
        let registry = ToolRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.names().is_empty());
    }

    #[test]
    fn registry_registers_and_lists_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool)).expect("register echo");
        registry.register(Arc::new(ReverseTool)).expect("register reverse");
        assert_eq!(registry.len(), 2);
        // Names are returned in sorted order.
        assert_eq!(registry.names(), vec!["echo".to_string(), "reverse".to_string()]);
        // get() returns an Arc<dyn BaseTool> that we can invoke.
        let echo = registry.get("echo").expect("echo tool present");
        assert_eq!(echo.name(), "echo");
    }

    #[test]
    fn registry_rejects_duplicate_names() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(EchoTool))
            .expect("first register");
        // Re-registering the same name must fail.
        let err = registry.register(Arc::new(EchoTool)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate") || msg.contains("Echo") || msg.contains("echo"),
            "error should mention the duplicate name: {msg}"
        );
    }

    #[tokio::test]
    async fn registry_invokes_custom_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool)).unwrap();
        registry.register(Arc::new(ReverseTool)).unwrap();

        let call = ToolCall::new("call_1", "echo", r#"{"text":"hello"}"#);
        let result = registry.execute(&call).await;
        assert_eq!(result.call_id, "call_1");
        assert_eq!(result.tool_name, "echo");
        assert!(!result.is_error, "expected success: {:?}", result.content);
        // The on-wire payload should include {"ok": true, "result": {"echo": "hello"}}.
        let parsed: Value = serde_json::from_str(&result.content).expect("payload is JSON");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["result"]["echo"], "hello");

        // Second tool: reverse.
        let call = ToolCall::new("call_2", "reverse", r#"{"text":"abc"}"#);
        let result = registry.execute(&call).await;
        let parsed: Value = serde_json::from_str(&result.content).expect("payload is JSON");
        assert_eq!(parsed["result"]["reversed"], "cba");
    }

    #[tokio::test]
    async fn registry_returns_error_for_unknown_tool() {
        let registry = ToolRegistry::new();
        let call = ToolCall::new("call_x", "no_such_tool", "{}");
        let result = registry.execute(&call).await;
        assert!(result.is_error);
        assert!(result.content.contains("Tool not found"));
    }

    #[tokio::test]
    async fn registry_returns_error_for_invalid_arguments() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool)).unwrap();
        // Arguments are not a JSON object.
        let call = ToolCall::new("call_bad", "echo", r#""just a string""#);
        let result = registry.execute(&call).await;
        assert!(result.is_error, "expected invalid-args error");
        assert!(result.content.contains("JSON object"));

        // Arguments are valid JSON but missing the required field.
        let call = ToolCall::new("call_bad2", "echo", "{}");
        let result = registry.execute(&call).await;
        assert!(result.is_error, "expected missing-args error");
        assert!(result.content.contains("text is required"));
    }

    #[test]
    fn from_tools_helper_skips_duplicates() {
        // The `from_tools` helper is the lenient constructor used to
        // merge partial registries; duplicates must be ignored rather
        // than erroring.
        let registry = ToolRegistry::from_tools(vec![
            Arc::new(EchoTool) as Arc<dyn BaseTool>,
            Arc::new(EchoTool) as Arc<dyn BaseTool>,
            Arc::new(ReverseTool) as Arc<dyn BaseTool>,
        ]);
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.names(), vec!["echo".to_string(), "reverse".to_string()]);
    }
}
