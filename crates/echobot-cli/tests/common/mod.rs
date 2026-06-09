//! Shared helpers for the `echobot-cli` integration tests.
//!
//! Exposes a stub [`LLMProvider`] implementation and a small builder
//! that wires a [`FullRuntimeContext`] around the stub. The tests use
//! this to drive the chat REPL end-to-end without touching the network
//! or any real model.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream;
use serde_json::Value;

use echobot_core::models::{LLMMessage, LLMResponse, LLMUsage, MessageRole};
use echobot_orchestration::roleplay::ProviderRoleplayLlm;
use echobot_orchestration::{ConversationCoordinator, DecisionEngine, RoleCardRegistry, RoleplayEngine};
use echobot_providers::{LLMProvider, ProviderError, ToolChoice};
use echobot_runtime::bootstrap::{
    build_runtime_context, RuntimeContext, RuntimeOptions, ToolRegistryFactoryPlaceholder,
};
use echobot_runtime::turns::ToolRegistryLike;
use echobot_skill::SkillRegistry;
use echobot_tools::memory::NoopMemorySupport;
use echobot_tools::shell::ShellSafetyMode;
use echobot_tools::{create_basic_tool_registry, BasicToolDeps, ToolRegistry};

use echobot_cli::bridge::RuntimeToolAdapter;
use echobot_cli::runtime_assembly::{FullRuntimeContext, ProviderDeciderAgent};

// ---------------------------------------------------------------------------
// Stub LLMProvider
// ---------------------------------------------------------------------------

/// A single recorded call to the stub provider.
#[derive(Debug, Clone)]
pub struct RecordedCall {
    /// The messages sent on the call.
    pub messages: Vec<LLMMessage>,
    /// The first user-text content (handy for assertions).
    pub first_user_text: Option<String>,
    /// Joined system-prompt text (handy for assertions).
    pub system_prompt: String,
    /// Tools that were passed (for tool-use assertions).
    pub tools: Option<Vec<echobot_core::models::LLMTool>>,
    /// Tool choice that was passed.
    pub tool_choice: Option<String>,
}

/// In-memory stub that never hits the network.
pub struct StubProvider {
    /// Canned responses, popped in FIFO order.
    queue: Mutex<Vec<StubReply>>,
    /// Every `generate` invocation is recorded here.
    calls: Mutex<Vec<RecordedCall>>,
    /// When the queue is empty, fall back to this reply.
    fallback: StubReply,
}

/// A single canned response.
#[derive(Debug, Clone)]
pub struct StubReply {
    /// The plain-text content of the assistant message.
    pub content: String,
}

impl StubReply {
    /// Builds a canned reply.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

impl StubProvider {
    /// Creates a new stub with the given queued replies and a fallback.
    pub fn new(queued: Vec<StubReply>, fallback: StubReply) -> Self {
        Self {
            queue: Mutex::new(queued),
            calls: Mutex::new(Vec::new()),
            fallback,
        }
    }

    /// Creates a stub that always returns the same canned reply.
    pub fn always(reply: StubReply) -> Self {
        Self::new(Vec::new(), reply)
    }

    /// Appends a canned reply to the end of the queue.
    pub fn enqueue(&self, reply: StubReply) {
        self.queue.lock().expect("queue mutex").push(reply);
    }

    /// Returns a snapshot of the recorded calls.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("calls mutex").clone()
    }

    /// Returns the number of times `generate` was called.
    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("calls mutex").len()
    }

    /// Pops the next queued reply or returns the fallback.
    fn next_reply(&self) -> StubReply {
        let mut q = self.queue.lock().expect("queue mutex");
        if q.is_empty() {
            self.fallback.clone()
        } else {
            q.remove(0)
        }
    }

    /// Records a single call.
    fn record(
        &self,
        messages: &[LLMMessage],
        tools: Option<&[echobot_core::models::LLMTool]>,
        tool_choice: Option<&ToolChoice>,
    ) {
        let first_user_text = messages
            .iter()
            .find(|m| matches!(m.role, MessageRole::User))
            .map(|m| m.content_text());
        let system_prompt = messages
            .iter()
            .filter(|m| matches!(m.role, MessageRole::System))
            .map(|m| m.content_text())
            .collect::<Vec<_>>()
            .join("\n");
        let tool_choice_str = tool_choice.map(|tc| match tc {
            ToolChoice::Auto => "auto".to_string(),
            ToolChoice::None => "none".to_string(),
            ToolChoice::Required => "required".to_string(),
            ToolChoice::Named(s) => format!("named:{s}"),
            ToolChoice::Structured(v) => format!("structured:{v}"),
        });
        self.calls.lock().expect("calls mutex").push(RecordedCall {
            messages: messages.to_vec(),
            first_user_text,
            system_prompt,
            tools: tools.map(|t| t.to_vec()),
            tool_choice: tool_choice_str,
        });
    }

    fn build_response(content: &str) -> LLMResponse {
        LLMResponse {
            message: LLMMessage::text(MessageRole::Assistant, content),
            model: "stub".to_string(),
            finish_reason: Some("stop".to_string()),
            usage: LLMUsage::default(),
            tool_calls: Vec::new(),
            raw_response: None,
        }
    }
}

#[async_trait]
impl LLMProvider for StubProvider {
    async fn generate(
        &self,
        messages: &[LLMMessage],
        tools: Option<&[echobot_core::models::LLMTool]>,
        tool_choice: Option<&ToolChoice>,
        _temperature: Option<f32>,
        _max_tokens: Option<u32>,
        _extra_body: Option<&HashMap<String, Value>>,
    ) -> Result<LLMResponse, ProviderError> {
        self.record(messages, tools, tool_choice);
        let reply = self.next_reply();
        Ok(Self::build_response(&reply.content))
    }

    async fn stream_generate<'a>(
        &'a self,
        messages: &'a [LLMMessage],
        tools: Option<&'a [echobot_core::models::LLMTool]>,
        tool_choice: Option<&'a ToolChoice>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        extra_body: Option<&'a HashMap<String, Value>>,
    ) -> Result<
        Box<
            dyn futures::stream::Stream<Item = Result<String, ProviderError>> + Send + 'a,
        >,
        ProviderError,
    > {
        let response = self
            .generate(messages, tools, tool_choice, temperature, max_tokens, extra_body)
            .await?;
        let text = response.message.content_text();
        let s = stream::once(async move { Ok::<_, ProviderError>(text) });
        Ok(Box::new(s))
    }
}

// ---------------------------------------------------------------------------
// TestContextBuilder
// ---------------------------------------------------------------------------

/// Builder for a [`FullRuntimeContext`] backed by a [`StubProvider`].
pub struct TestContextBuilder {
    options: RuntimeOptions,
    stub: Arc<StubProvider>,
}

impl TestContextBuilder {
    /// Creates a new builder with the given workspace and stub.
    pub fn new(workspace: PathBuf, stub: Arc<StubProvider>) -> Self {
        let options = RuntimeOptions {
            env_file: workspace
                .join(".env")
                .to_string_lossy()
                .to_string(),
            workspace: Some(workspace.clone()),
            no_memory: true,
            no_skills: true,
            no_heartbeat: true,
            heartbeat_interval: Some(3600),
            ..RuntimeOptions::default()
        };
        Self { options, stub }
    }

    /// Builds the [`FullRuntimeContext`]. Mirrors
    /// `assemble_runtime` but replaces every LLM consumer with the stub.
    pub async fn build(self) -> FullRuntimeContext {
        // 1. Build the runtime context normally.
        let runtime: RuntimeContext = build_runtime_context(self.options.clone(), false)
            .await
            .expect("build_runtime_context");

        // 2. Memory + skill registries (mirroring assemble_runtime).
        let memory_support: Option<Arc<dyn echobot_tools::memory::MemorySupport>> = if self
            .options
            .no_memory
        {
            None
        } else {
            Some(Arc::new(NoopMemorySupport))
        };
        let skill_registry: Option<Arc<SkillRegistry>> = if self.options.no_skills {
            None
        } else {
            let sr = SkillRegistry::discover(&runtime.workspace, "echobot", None, true);
            Some(Arc::new(sr))
        };

        // 3. Cron adapter.
        let cron_tool_service = Arc::new(
            echobot_cli::bridge::RuntimeCronAdapter::new(
                runtime.cron_service.clone(),
            ),
        );

        // 4. Tool registry factory. Closes over the per-session deps.
        let workspace_for_factory = runtime.workspace.clone();
        let attachment_for_factory = runtime.attachment_store.clone();
        let cron_for_factory = cron_tool_service.clone();
        let memory_for_factory = memory_support.clone();
        let tool_registry_factory: ToolRegistryFactoryPlaceholder = Box::new(
            move |session_name: &str, _scheduled_context: bool| {
                let workspace = workspace_for_factory.clone();
                let attachment = attachment_for_factory.clone();
                let cron = cron_for_factory.clone();
                let memory = memory_for_factory.clone();
                let session_name = session_name.to_string();
                Box::pin(async move {
                    let deps = BasicToolDeps {
                        workspace: Some(workspace),
                        attachment_store: Some(attachment),
                        supports_image_input: true,
                        memory_support: memory,
                        cron_service: Some(cron),
                        session_name: session_name.clone(),
                        allow_file_writes: true,
                        allow_cron_mutations: true,
                        allow_private_network: false,
                        shell_safety_mode: ShellSafetyMode::DangerFullAccess,
                    };
                    let registry: ToolRegistry = create_basic_tool_registry(deps);
                    let _ = registry.get("cron");
                    Some(Box::new(RuntimeToolAdapter(registry)) as Box<dyn ToolRegistryLike>)
                })
            },
        );

        // 5. Role card registry.
        let role_registry = Arc::new(RoleCardRegistry::discover(&runtime.workspace).await);

        // 6. Decider + roleplay engines wired to the stub.
        let decider_agent: Arc<dyn echobot_orchestration::decision::DeciderAgent> =
            Arc::new(ProviderDeciderAgent::new(self.stub.clone()));
        let decision_engine = Arc::new(DecisionEngine::new(Some(decider_agent), None));
        let roleplay_engine = Arc::new(RoleplayEngine::new(
            Arc::new(ProviderRoleplayLlm::new(self.stub.clone())),
            None,
            None,
            None,
        ));

        // 7. Coordinator.
        let coordinator = Arc::new(ConversationCoordinator::new(
            runtime.session_store.clone(),
            runtime.session_runner.clone(),
            decision_engine.clone(),
            roleplay_engine.clone(),
            role_registry.clone(),
            true,
            None,
        ));

        // 8. Replace the runtime's agent shim and session runner so the
        //    agent path also goes through the stub.
        let agent_shim: Arc<dyn echobot_runtime::turns::AgentCoreLike> =
            Arc::new(StubAgentCore::new(self.stub.clone()));
        let session_runner = Arc::new(echobot_runtime::session_runner::SessionAgentRunner::new(
            agent_shim,
            runtime.agent_session_store.clone(),
            None,
            None,
            None,
            None,
            24,
            runtime.trace_store.clone(),
        ));
        let mut runtime = runtime;
        runtime.session_runner = session_runner;
        runtime.provider = self.stub.clone();

        FullRuntimeContext {
            runtime,
            coordinator,
            role_registry,
            tool_registry_factory,
            cron_tool_service,
            memory_support,
            skill_registry,
        }
    }
}

// ---------------------------------------------------------------------------
// StubAgentCore — wraps a StubProvider as the runtime's AgentCoreLike.
// ---------------------------------------------------------------------------

struct StubAgentCore {
    provider: Arc<dyn LLMProvider>,
}

impl StubAgentCore {
    fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }
}

fn build_messages(
    user_input: &str,
    history: Option<&[echobot_core::models::LLMMessage]>,
    extra_system_messages: Option<&[String]>,
    transient_system_messages: Option<&[String]>,
) -> Vec<echobot_core::models::LLMMessage> {
    use echobot_core::models::{LLMMessage, MessageRole};
    let mut messages: Vec<LLMMessage> = Vec::new();
    if let Some(extras) = extra_system_messages {
        for text in extras {
            if !text.trim().is_empty() {
                messages.push(LLMMessage::text(MessageRole::System, text.clone()));
            }
        }
    }
    if let Some(transient) = transient_system_messages {
        for text in transient {
            if !text.trim().is_empty() {
                messages.push(LLMMessage::text(MessageRole::System, text.clone()));
            }
        }
    }
    if let Some(history) = history {
        for msg in history {
            if matches!(msg.role, MessageRole::System) {
                continue;
            }
            messages.push(msg.clone());
        }
    }
    messages.push(LLMMessage::text(MessageRole::User, user_input));
    messages
}

#[async_trait]
impl echobot_runtime::turns::AgentCoreLike for StubAgentCore {
    fn ask<'a>(
        &'a self,
        user_input: &'a str,
        _image_urls: Option<&'a [echobot_core::models::ImageInput]>,
        _file_attachments: Option<&'a [echobot_core::models::FileInput]>,
        history: Option<&'a [echobot_core::models::LLMMessage]>,
        _tools: Option<&'a [echobot_core::models::LLMTool]>,
        extra_system_messages: Option<&'a [String]>,
        transient_system_messages: Option<&'a [String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = echobot_runtime::error::Result<echobot_core::models::LLMResponse>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let messages =
                build_messages(user_input, history, extra_system_messages, transient_system_messages);
            self.provider
                .generate(
                    &messages,
                    None,
                    Some(&ToolChoice::Auto),
                    temperature,
                    max_tokens,
                    None,
                )
                .await
                .map_err(|e| echobot_runtime::error::Error::Wiring(e.to_string()))
        })
    }

    fn ask_with_tools<'a>(
        &'a self,
        user_input: &'a str,
        _tool_registry: &'a dyn ToolRegistryLike,
        _image_urls: Option<&'a [echobot_core::models::ImageInput]>,
        _file_attachments: Option<&'a [echobot_core::models::FileInput]>,
        history: Option<&'a [echobot_core::models::LLMMessage]>,
        _compressed_summary: &'a str,
        extra_system_messages: Option<&'a [String]>,
        transient_system_messages: Option<&'a [String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        _max_steps: usize,
        _trace_callback: Option<echobot_runtime::turns::TraceCallback>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = echobot_runtime::error::Result<echobot_runtime::turns::AgentRunResult>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let history_vec: Vec<echobot_core::models::LLMMessage> =
                history.map(|h| h.to_vec()).unwrap_or_default();
            let messages = build_messages(
                user_input,
                Some(&history_vec),
                extra_system_messages,
                transient_system_messages,
            );
            let response = self
                .provider
                .generate(
                    &messages,
                    None,
                    Some(&ToolChoice::Auto),
                    temperature,
                    max_tokens,
                    None,
                )
                .await
                .map_err(|e| echobot_runtime::error::Error::Wiring(e.to_string()))?;
            Ok(echobot_runtime::turns::AgentRunResult {
                response,
                new_messages: Vec::new(),
                history: history_vec,
                steps: 1,
                compressed_summary: String::new(),
                outbound_content_blocks: Vec::new(),
                status: "completed".to_string(),
                pending_user_input: None,
            })
        })
    }

    fn ask_with_memory<'a>(
        &'a self,
        user_input: &'a str,
        _image_urls: Option<&'a [echobot_core::models::ImageInput]>,
        _file_attachments: Option<&'a [echobot_core::models::FileInput]>,
        history: Option<&'a [echobot_core::models::LLMMessage]>,
        _compressed_summary: &'a str,
        extra_system_messages: Option<&'a [String]>,
        transient_system_messages: Option<&'a [String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        _trace_callback: Option<echobot_runtime::turns::TraceCallback>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = echobot_runtime::error::Result<echobot_runtime::turns::AgentRunResult>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let history_vec: Vec<echobot_core::models::LLMMessage> =
                history.map(|h| h.to_vec()).unwrap_or_default();
            let messages = build_messages(
                user_input,
                Some(&history_vec),
                extra_system_messages,
                transient_system_messages,
            );
            let response = self
                .provider
                .generate(
                    &messages,
                    None,
                    Some(&ToolChoice::Auto),
                    temperature,
                    max_tokens,
                    None,
                )
                .await
                .map_err(|e| echobot_runtime::error::Error::Wiring(e.to_string()))?;
            Ok(echobot_runtime::turns::AgentRunResult {
                response,
                new_messages: Vec::new(),
                history: history_vec,
                steps: 1,
                compressed_summary: String::new(),
                outbound_content_blocks: Vec::new(),
                status: "completed".to_string(),
                pending_user_input: None,
            })
        })
    }

    fn ask_with_skills<'a>(
        &'a self,
        user_input: &'a str,
        _skill_registry: &'a dyn echobot_runtime::turns::SkillRegistryLike,
        _tool_registry: Option<&'a dyn ToolRegistryLike>,
        _image_urls: Option<&'a [echobot_core::models::ImageInput]>,
        _file_attachments: Option<&'a [echobot_core::models::FileInput]>,
        history: Option<&'a [echobot_core::models::LLMMessage]>,
        _compressed_summary: &'a str,
        extra_system_messages: Option<&'a [String]>,
        transient_system_messages: Option<&'a [String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        _max_steps: usize,
        _trace_callback: Option<echobot_runtime::turns::TraceCallback>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = echobot_runtime::error::Result<echobot_runtime::turns::AgentRunResult>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let history_vec: Vec<echobot_core::models::LLMMessage> =
                history.map(|h| h.to_vec()).unwrap_or_default();
            let messages = build_messages(
                user_input,
                Some(&history_vec),
                extra_system_messages,
                transient_system_messages,
            );
            let response = self
                .provider
                .generate(
                    &messages,
                    None,
                    Some(&ToolChoice::Auto),
                    temperature,
                    max_tokens,
                    None,
                )
                .await
                .map_err(|e| echobot_runtime::error::Error::Wiring(e.to_string()))?;
            Ok(echobot_runtime::turns::AgentRunResult {
                response,
                new_messages: Vec::new(),
                history: history_vec,
                steps: 1,
                compressed_summary: String::new(),
                outbound_content_blocks: Vec::new(),
                status: "completed".to_string(),
                pending_user_input: None,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Workspace helper
// ---------------------------------------------------------------------------

/// Returns a fresh, writable workspace directory under the system temp
/// dir. The directory is uniquely named per call so parallel test runs
/// don't collide.
pub fn unique_workspace(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("echobot-cli-test-{tag}-{pid}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("create temp workspace");
    dir
}
