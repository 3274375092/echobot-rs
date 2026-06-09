//! Full runtime assembly.
//!
//! `echobot_runtime::bootstrap::build_runtime_context` produces a
//! runtime-only context (provider, sessions, scheduling, settings, agent
//! shim, session runner). This module layers the orchestration pieces on
//! top: role card registry, decision engine, roleplay engine, the
//! conversation coordinator, the tool registry factory, and the cron
//! tool adapter. The resulting [`FullRuntimeContext`] is the value the
//! `chat` subcommand drives.
//!
//! This split is forced by Rust's module dependency rules: the
//! `echobot-orchestration` crate depends on the `echobot-runtime` crate
//! (it uses `SessionAgentRunner`, `CoordinatorLike`, etc.), so the
//! runtime crate cannot import orchestration types without creating a
//! cycle. Putting the cross-crate assembly in the CLI crate — which
//! depends on both — keeps the graph acyclic.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Future;

use echobot_core::models::LLMMessage;
use echobot_orchestration::{
    ConversationCoordinator, DecisionEngine, ProviderRoleplayLlm, RoleCardRegistry,
    RoleplayEngine, RouteMode,
};
use echobot_providers::LLMProvider;
use echobot_runtime::bootstrap::{RuntimeContext, RuntimeOptions, ToolRegistryFactoryPlaceholder};
use echobot_runtime::scheduled_tasks::{build_cron_job_executor, build_heartbeat_executor};
use echobot_runtime::turns::ToolRegistryLike;
use echobot_skill::SkillRegistry;
use echobot_tools::memory::{MemorySupport, NoopMemorySupport};
use echobot_tools::shell::ShellSafetyMode;
use echobot_tools::{create_basic_tool_registry, BasicToolDeps, ToolRegistry};

use crate::bridge::{RuntimeCronAdapter, RuntimeToolAdapter};

/// The full runtime context. Extends the runtime's [`RuntimeContext`]
/// with the orchestration pieces the CLI drives.
pub struct FullRuntimeContext {
    /// The runtime-side context.
    pub runtime: RuntimeContext,
    /// The orchestrator.
    pub coordinator: Arc<ConversationCoordinator>,
    /// Role card registry.
    pub role_registry: Arc<RoleCardRegistry>,
    /// Tool registry factory (used by the chat loop to build per-session
    /// tool registries on demand).
    pub tool_registry_factory: ToolRegistryFactoryPlaceholder,
    /// Cron tool service adapter.
    pub cron_tool_service: Arc<RuntimeCronAdapter>,
    /// Memory support (None when `--no-memory`).
    pub memory_support: Option<Arc<dyn MemorySupport>>,
    /// Skill registry (None when `--no-skills`).
    pub skill_registry: Option<Arc<SkillRegistry>>,
}

/// Assembles a full runtime context. Calls the runtime's
/// [`RuntimeContext::bootstrap`] for the lower-level pieces, then wires
/// the orchestration on top.
pub async fn assemble_runtime(
    options: RuntimeOptions,
    load_session_state: bool,
) -> anyhow::Result<FullRuntimeContext> {
    let runtime = echobot_runtime::bootstrap::build_runtime_context(options.clone(), load_session_state)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // 1. Memory support (placeholder — phase 2 wires ReMeLight).
    let memory_support: Option<Arc<dyn MemorySupport>> = if options.no_memory {
        None
    } else {
        Some(Arc::new(NoopMemorySupport))
    };

    // 2. Skill registry.
    let skill_registry: Option<Arc<SkillRegistry>> = if options.no_skills {
        None
    } else {
        let sr = SkillRegistry::discover(&runtime.workspace, "echobot", None, true);
        if !sr.warnings.is_empty() {
            for w in &sr.warnings {
                tracing::warn!(warning = %w, "skill discovery");
            }
        }
        Some(Arc::new(sr))
    };

    // 3. Cron tool adapter (wraps the runtime cron service in the trait
    //    object the `CronTool` consumes).
    let cron_tool_service = Arc::new(RuntimeCronAdapter::new(runtime.cron_service.clone()));

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
                // Make sure the cron tool is registered when the
                // service is wired. The factory always supplies a cron
                // service so the tool is always present in v1.
                let _ = registry.get("cron");
                Some(Box::new(RuntimeToolAdapter(registry)) as Box<dyn ToolRegistryLike>)
            })
        },
    );

    // 5. Role card registry.
    let role_registry = Arc::new(RoleCardRegistry::discover(&runtime.workspace).await);

    // 6. Decider + roleplay engines.
    let decider_agent: Arc<dyn echobot_orchestration::decision::DeciderAgent> =
        Arc::new(ProviderDeciderAgent::new(runtime.provider.clone()));
    let decision_engine = Arc::new(DecisionEngine::new(Some(decider_agent), None));
    let roleplay_engine = Arc::new(RoleplayEngine::new(
        Arc::new(ProviderRoleplayLlm::new(runtime.provider.clone())),
        options.temperature,
        options.max_tokens,
        None,
    ));

    // 7. Coordinator.
    let coordinator = Arc::new(ConversationCoordinator::new(
        runtime.session_store.clone(),
        runtime.session_runner.clone(),
        decision_engine.clone(),
        roleplay_engine.clone(),
        role_registry.clone(),
        options.delegated_ack_enabled.unwrap_or(true),
        None,
    ));

    // 8. Wire the cron service's `on_job` callback to the coordinator.
    //    The runtime exposes `on_job` as a public field; we use
    //    `Arc::get_mut` because the assembly layer is the only owner
    //    of the cron-service arc at this point.
    {
        let mut cron_arc = runtime.cron_service.clone();
        if let Some(inner) = std::sync::Arc::get_mut(&mut cron_arc) {
            let notify: echobot_runtime::scheduled_tasks::ScheduleNotifier =
                std::sync::Arc::new(|_session, _source, _job_name, _visible| {
                    Box::pin(async move { Ok(()) })
                });
            let exec = build_cron_job_executor(
                runtime.session_runner.clone(),
                coordinator.clone(),
                notify,
            );
            // Wrap the `impl Fn` returned by `build_cron_job_executor`
            // in an `Arc<dyn Fn>` so it fits the `JobExecutor` type.
            inner.on_job = Some(std::sync::Arc::new(move |job| exec(job)));
        }
    }

    // 9. Wire heartbeat executor (same `Arc::get_mut` pattern).
    if let Some(hb_arc_unwrapped) = runtime.heartbeat_service.as_ref() {
        // Re-wrap the HeartbeatService in an Arc so we can call
        // `Arc::get_mut` to mutate its `on_execute` field. We clone the
        // relevant fields into a fresh service that we own exclusively.
        let mut hb_arc = std::sync::Arc::new(
            echobot_runtime::scheduling::heartbeat::HeartbeatService::new(
                hb_arc_unwrapped.heartbeat_file.clone(),
                hb_arc_unwrapped.provider.clone(),
                None,
                None,
                hb_arc_unwrapped.interval_seconds,
                hb_arc_unwrapped.enabled,
            ),
        );
        if let Some(inner) = std::sync::Arc::get_mut(&mut hb_arc) {
            let exec = build_heartbeat_executor(runtime.session_runner.clone());
            let exec_arc: echobot_runtime::scheduling::heartbeat::HeartbeatExecutor =
                std::sync::Arc::new(move |tasks: String| exec(tasks));
            inner.on_execute = Some(exec_arc);
        }
    }

    Ok(FullRuntimeContext {
        runtime,
        coordinator,
        role_registry,
        tool_registry_factory,
        cron_tool_service,
        memory_support,
        skill_registry,
    })
}

// ---------------------------------------------------------------------------
// Decider adapter: bridges the LLM provider to the DecisionEngine's
// DeciderAgent trait. The orchestration crate doesn't depend on the
// provider directly, so we add the bridge here.
// ---------------------------------------------------------------------------

/// Adapter that uses the main LLM provider as the decision-layer
/// decider (mirrors the Python `AgentCore` one-shot ask).
pub struct ProviderDeciderAgent {
    provider: Arc<dyn LLMProvider>,
}

impl ProviderDeciderAgent {
    /// Creates a new adapter.
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl echobot_orchestration::decision::DeciderAgent for ProviderDeciderAgent {
    async fn ask(
        &self,
        user_input: &str,
        history: Option<&[LLMMessage]>,
        extra_system_messages: Option<&[String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<echobot_orchestration::decision::DeciderAgentResponse, anyhow::Error> {
        use echobot_core::models::{LLMMessage, MessageRole};
        let mut messages: Vec<LLMMessage> = Vec::new();
        if let Some(extras) = extra_system_messages {
            for text in extras {
                messages.push(LLMMessage::text(MessageRole::System, text.clone()));
            }
        }
        if let Some(h) = history {
            for msg in h {
                if matches!(msg.role, MessageRole::System) {
                    continue;
                }
                messages.push(msg.clone());
            }
        }
        messages.push(LLMMessage::text(MessageRole::User, user_input));
        let response = self
            .provider
            .generate(
                &messages,
                None,
                Some(&echobot_providers::ToolChoice::Auto),
                temperature,
                max_tokens,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(echobot_orchestration::decision::DeciderAgentResponse {
            content: response.message.content_text(),
            finish_reason: response.finish_reason.clone(),
        })
    }
}

// `RouteMode` is re-exported from the orchestration crate so the CLI can
// reference it without depending on orchestration directly in its public
// surface.
#[allow(dead_code)]
fn _force_pathbufs(_p: PathBuf) {}

// silence unused-import warnings for things that are part of the public
// surface but not directly used in this file.
#[allow(unused_imports)]
use {build_cron_job_executor as _, RouteMode as _, ToolRegistryFactoryPlaceholder as _};

// silence the "trait alias imported but not used" lint for Pin/Future
// (they're re-exported indirectly via async_trait).
#[allow(dead_code)]
fn _ensure_pin_future_in_scope(_f: Pin<Box<dyn Future<Output = ()> + Send>>) {}
