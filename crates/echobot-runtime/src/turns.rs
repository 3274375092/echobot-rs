//! Per-turn agent execution.
//!
//! Mirrors `echobot/runtime/turns.py`. The real `AgentCore`/`ToolRegistry`/
//! `SkillRegistry` types are not ported yet (they live in
//! `echobot-core` / `echobot-tools` / `echobot-skill`); to keep this slice
//! compilable, this module defines local trait aliases
//! ([`AgentCoreLike`], [`ToolRegistryLike`], [`SkillRegistryLike`]) that the
//! real types will implement once the agent / tools / skill crates are
//! filled in. The orchestration crate's `ConversationCoordinator` will end
//! up implementing the same trait shape.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use echobot_core::models::{FileInput, ImageInput, LLMMessage, LLMResponse};

use crate::error::Result;

/// A callback used to emit agent-trace events. Mirrors the Python
/// `TraceCallback`.
pub type TraceCallback = Arc<
    dyn Fn(String, serde_json::Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

/// Result of a single agent run. Matches the Python `AgentRunResult`.
#[derive(Debug, Clone)]
pub struct AgentRunResult {
    /// The final assistant response.
    pub response: LLMResponse,
    /// Messages added during this turn (user prompt + assistant / tool / ...).
    pub new_messages: Vec<LLMMessage>,
    /// Updated history (the persistent history after the turn).
    pub history: Vec<LLMMessage>,
    /// Number of model steps (e.g. tool loop iterations).
    pub steps: usize,
    /// Compressed-history summary (if memory compaction ran).
    pub compressed_summary: String,
    /// Outbound content blocks surfaced by tool calls.
    pub outbound_content_blocks: Vec<serde_json::Value>,
    /// Run status (e.g. `"completed"`, `"needs_user_input"`).
    pub status: String,
    /// Optional metadata when a tool requested user input.
    pub pending_user_input: Option<serde_json::Value>,
}

/// Subset of `AgentCore` the runner actually needs. The real
/// `echobot_core::AgentCore` will implement this trait once it's ported.
///
/// The method signatures mirror the Python `AgentCore` 1:1, so each
/// `ask_with_*` entry point necessarily takes a long argument list.
/// Clippy's `too_many_arguments` lint is intentionally suppressed on the
/// trait so the wire-up between the Python and Rust ports stays
/// trivially comparable.
#[allow(clippy::too_many_arguments)]
pub trait AgentCoreLike: Send + Sync {
    /// Ask the model a one-shot question (no tools / no memory).
    fn ask<'a>(
        &'a self,
        user_input: &'a str,
        image_urls: Option<&'a [ImageInput]>,
        file_attachments: Option<&'a [FileInput]>,
        history: Option<&'a [LLMMessage]>,
        tools: Option<&'a [echobot_core::models::LLMTool]>,
        extra_system_messages: Option<&'a [String]>,
        transient_system_messages: Option<&'a [String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Pin<Box<dyn Future<Output = Result<LLMResponse>> + Send + 'a>>;

    /// Ask with a tool registry. The full tool loop is the agent's
    /// responsibility.
    fn ask_with_tools<'a>(
        &'a self,
        user_input: &'a str,
        tool_registry: &'a dyn ToolRegistryLike,
        image_urls: Option<&'a [ImageInput]>,
        file_attachments: Option<&'a [FileInput]>,
        history: Option<&'a [LLMMessage]>,
        compressed_summary: &'a str,
        extra_system_messages: Option<&'a [String]>,
        transient_system_messages: Option<&'a [String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        max_steps: usize,
        trace_callback: Option<TraceCallback>,
    ) -> Pin<Box<dyn Future<Output = Result<AgentRunResult>> + Send + 'a>>;

    /// Ask with memory + (optionally) tools + (optionally) skills.
    fn ask_with_memory<'a>(
        &'a self,
        user_input: &'a str,
        image_urls: Option<&'a [ImageInput]>,
        file_attachments: Option<&'a [FileInput]>,
        history: Option<&'a [LLMMessage]>,
        compressed_summary: &'a str,
        extra_system_messages: Option<&'a [String]>,
        transient_system_messages: Option<&'a [String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        trace_callback: Option<TraceCallback>,
    ) -> Pin<Box<dyn Future<Output = Result<AgentRunResult>> + Send + 'a>>;

    /// Ask with skills, which may layer their own tools on top of the
    /// `tool_registry`.
    fn ask_with_skills<'a>(
        &'a self,
        user_input: &'a str,
        skill_registry: &'a dyn SkillRegistryLike,
        tool_registry: Option<&'a dyn ToolRegistryLike>,
        image_urls: Option<&'a [ImageInput]>,
        file_attachments: Option<&'a [FileInput]>,
        history: Option<&'a [LLMMessage]>,
        compressed_summary: &'a str,
        extra_system_messages: Option<&'a [String]>,
        transient_system_messages: Option<&'a [String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        max_steps: usize,
        trace_callback: Option<TraceCallback>,
    ) -> Pin<Box<dyn Future<Output = Result<AgentRunResult>> + Send + 'a>>;
}

/// Trait alias for the tool-registry contract the runner needs. The real
/// `echobot_tools::ToolRegistry` will implement this trait once it's ported.
pub trait ToolRegistryLike: Send + Sync {
    /// Returns the names of every registered tool.
    fn names(&self) -> Vec<String>;
}

/// Trait alias for the skill-registry contract the runner needs. The real
/// `echobot_skill::SkillRegistry` will implement this trait once it's
/// ported.
pub trait SkillRegistryLike: Send + Sync {
    /// Returns the names of skills that should be considered active given
    /// `history`.
    fn active_skill_names(&self, history: Option<&[LLMMessage]>) -> Vec<String>;
    /// Returns the names of skills explicitly mentioned in `user_input`
    /// (e.g. `/skill-name`).
    fn explicit_skill_names(&self, user_input: &str) -> Vec<String>;
}

/// Picks the right `ask_with_*` entry point on `agent`, matching the Python
/// behavior in `echobot/runtime/turns.py`.
///
/// The argument list mirrors the Python helper 1:1; see the
/// `AgentCoreLike` trait note above for why `too_many_arguments` is
/// intentionally suppressed.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_turn(
    agent: &dyn AgentCoreLike,
    prompt: &str,
    history: Vec<LLMMessage>,
    image_urls: Option<&[ImageInput]>,
    file_attachments: Option<&[FileInput]>,
    compressed_summary: &str,
    skill_registry: Option<&dyn SkillRegistryLike>,
    tool_registry: Option<&dyn ToolRegistryLike>,
    extra_system_messages: Option<&[String]>,
    transient_system_messages: Option<&[String]>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    max_steps: usize,
    trace_callback: Option<TraceCallback>,
) -> Result<AgentRunResult> {
    if let Some(skills) = skill_registry {
        return agent
            .ask_with_skills(
                prompt,
                skills,
                tool_registry,
                image_urls,
                file_attachments,
                Some(&history),
                compressed_summary,
                extra_system_messages,
                transient_system_messages,
                temperature,
                max_tokens,
                max_steps,
                trace_callback,
            )
            .await;
    }
    if let Some(tools) = tool_registry {
        return agent
            .ask_with_tools(
                prompt,
                tools,
                image_urls,
                file_attachments,
                Some(&history),
                compressed_summary,
                extra_system_messages,
                transient_system_messages,
                temperature,
                max_tokens,
                max_steps,
                trace_callback,
            )
            .await;
    }
    agent
        .ask_with_memory(
            prompt,
            image_urls,
            file_attachments,
            Some(&history),
            compressed_summary,
            extra_system_messages,
            transient_system_messages,
            temperature,
            max_tokens,
            trace_callback,
        )
        .await
}
