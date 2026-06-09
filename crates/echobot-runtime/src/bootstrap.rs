//! Runtime bootstrap.
//!
//! Mirrors `echobot/runtime/bootstrap.py`. The Python version assembles the
//! full runtime: provider, agent core, session stores, trace store, tool
//! registry factory, skill registry, cron + heartbeat services, role
//! registry, decision engine, roleplay engine, and the conversation
//! coordinator.
//!
//! In the Rust port, several of those types are not yet ported
//! (`AgentCore`, `ToolRegistry`, `SkillRegistry`, the orchestration
//! coordinator / engines / role registry, ...). The bootstrap module
//! therefore:
//!
//! * Defines the same struct shapes ([`RuntimeOptions`], [`RuntimeContext`])
//!   so callers can already hold a context value.
//! * Provides a [`build_runtime_context`] function that returns a partial
//!   runtime context (sessions, scheduling, settings, session runner).
//!   The CLI layer is responsible for wiring the orchestration pieces on
//!   top; see `echobot_cli::runtime_assembly::assemble_runtime` for the
//!   full entrypoint used by the `chat` / `app` / `gateway` subcommands.

use std::path::PathBuf;
use std::sync::Arc;

use crate::agent_traces::AgentTraceStore;
use crate::error::{Error, Result};
use crate::scheduling::cron::CronService;
use crate::scheduling::heartbeat::HeartbeatService;
use crate::session_runner::SessionAgentRunner;
use crate::sessions::{ChatSession, SessionStore};
use crate::settings::{RuntimeConfigSnapshot, RuntimeControls, RuntimeSettingsManager};
use crate::turns::{AgentCoreLike, SkillRegistryLike, ToolRegistryLike};

// ---------------------------------------------------------------------------
// Lightweight placeholders for not-yet-ported types.
//
// These let us name the `RuntimeContext` fields without pulling in the real
// implementations. Once the `echobot-orchestration` / `echobot-tools` /
// `echobot-skill` crates land their types, the placeholder modules can be
// deleted and the context can hold the real types directly.
// ---------------------------------------------------------------------------

/// Placeholder for `echobot_core::AttachmentStore`.
pub type AttachmentStorePlaceholder = Arc<echobot_core::attachments::AttachmentStore>;

/// Placeholder for the provider used by the runtime.
pub type ProviderPlaceholder = Arc<dyn echobot_providers::LLMProvider>;

/// Placeholder for the tool-registry factory the CLI uses to build a
/// per-session registry.
pub type ToolRegistryFactoryPlaceholder = crate::session_runner::ToolRegistryFactory;

/// Builds the runtime context from `options`.
///
/// This returns the runtime-side pieces (provider, agent shim, session
/// stores, trace store, cron service, optional heartbeat service, session
/// runner, runtime controls, settings manager). The CLI's
/// `runtime_assembly` module layers the orchestration components on top to
/// produce a complete context for the chat / app / gateway subcommands.
pub async fn build_runtime_context(
    options: RuntimeOptions,
    load_session_state: bool,
) -> Result<RuntimeContext> {
    // 1. Load .env (best-effort; missing file is fine).
    let env_path = PathBuf::from(&options.env_file);
    if env_path.exists() {
        let _ = dotenvy::from_path_override(&env_path);
    } else {
        let _ = dotenvy::dotenv();
    }

    // 2. Workspace.
    let workspace = options
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // 3. Build the LLM provider from env.
    let provider_settings = echobot_providers::OpenAICompatibleSettings::from_env(Some("LLM_"))
        .map_err(|e| Error::Wiring(format!("failed to load LLM settings: {e}")))?;
    let echobot_dir = workspace.join(".echobot");
    let attachment_store = Arc::new(echobot_core::attachments::AttachmentStore::new(
        echobot_dir.join("attachments"),
        None,
        None,
    ));
    let provider = echobot_providers::OpenAICompatibleProvider::new(
        provider_settings,
        Some((*attachment_store).clone()),
    )
    .map_err(|e| Error::Wiring(format!("failed to build LLM provider: {e}")))?;
    let provider: Arc<dyn echobot_providers::LLMProvider> = Arc::new(provider);

    // 4. Agent shim — the full tool/skill loop is layered on by the
    //    `SessionAgentRunner` when tools / skills are available. The shim
    //    here covers one-shot ask + the various `ask_with_*` entry points.
    let agent: Arc<dyn AgentCoreLike> = Arc::new(SimpleAgentCore {
        provider: provider.clone(),
    });

    // 5. Session stores + trace store.
    let session_store = SessionStore::new(echobot_dir.join("sessions"));
    let agent_session_store = SessionStore::new(echobot_dir.join("agent_sessions"));
    let trace_store = AgentTraceStore::new(echobot_dir.join("agent_traces"));

    // 6. Cron + heartbeat services.
    let cron_store_path = echobot_dir.join("cron").join("jobs.json");
    let cron_service = Arc::new(CronService::new(&cron_store_path, None, Some(1.0)));
    let heartbeat_file_path = echobot_dir.join("HEARTBEAT.md");
    let heartbeat_interval_seconds = options.heartbeat_interval.unwrap_or(1800).max(1);
    let heartbeat_service = if options.no_heartbeat {
        None
    } else {
        Some(HeartbeatService::new(
            &heartbeat_file_path,
            provider.clone(),
            None,
            None,
            heartbeat_interval_seconds,
            true,
        ))
    };

    // 7. Runtime controls + settings manager.
    let runtime_controls = Arc::new(tokio::sync::Mutex::new(RuntimeControls {
        shell_safety_mode: "danger-full-access".to_string(),
        file_write_enabled: true,
        cron_mutation_enabled: true,
        web_private_network_enabled: false,
    }));
    let controls_coordinator = Arc::new(tokio::sync::Mutex::new(
        RuntimeControlsCoordinator::new(runtime_controls.clone(), true),
    ));
    let settings_manager = Arc::new(RuntimeSettingsManager::new(
        &workspace,
        controls_coordinator.clone(),
        runtime_controls.clone(),
    ));

    // 8. Session agent runner.
    let session_runner = Arc::new(SessionAgentRunner::new(
        agent.clone(),
        agent_session_store.clone(),
        None,
        None,
        options.temperature,
        options.max_tokens,
        24,
        Some(trace_store.clone()),
    ));

    // 9. Optional session loading.
    let session = if load_session_state {
        Some(load_initial_session(&session_store, &options).await?)
    } else {
        None
    };

    let default_runtime_config = RuntimeConfigSnapshot {
        delegated_ack_enabled: true,
        shell_safety_mode: "danger-full-access".to_string(),
        file_write_enabled: true,
        cron_mutation_enabled: true,
        web_private_network_enabled: false,
    };

    Ok(RuntimeContext {
        workspace,
        attachment_store,
        supports_image_input: true,
        agent,
        provider,
        session_store,
        agent_session_store,
        session,
        tool_registry: None,
        skill_registry: None,
        cron_service,
        heartbeat_service,
        session_runner,
        coordinator: None,
        role_registry: None,
        memory_support: None,
        heartbeat_file_path,
        heartbeat_interval_seconds,
        tool_registry_factory: None,
        runtime_controls,
        default_runtime_config,
        settings_manager: Some(settings_manager),
        trace_store: Some(trace_store),
    })
}

async fn load_initial_session(
    session_store: &SessionStore,
    options: &RuntimeOptions,
) -> Result<ChatSession> {
    if let Some(name) = &options.new_session {
        let session = session_store
            .create_session(Some(name))
            .await
            .map_err(|e| Error::Wiring(format!("failed to create session '{name}': {e}")))?;
        return Ok(session);
    }
    if let Some(name) = &options.session {
        let session = session_store.load_or_create_session(name).await?;
        session_store.set_current_session(&session.name).await?;
        return Ok(session);
    }
    let session = session_store.load_current_session().await?;
    Ok(session)
}

// ---------------------------------------------------------------------------
// Simple AgentCore shim (used by the runtime-side `build_runtime_context`).
// The CLI assembly may replace this with a richer one if needed.
// ---------------------------------------------------------------------------

struct SimpleAgentCore {
    provider: Arc<dyn echobot_providers::LLMProvider>,
}

fn build_messages_with_system(
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

fn history_from_optional(
    history: Option<&[echobot_core::models::LLMMessage]>,
) -> Vec<echobot_core::models::LLMMessage> {
    history.map(|h| h.to_vec()).unwrap_or_default()
}

#[async_trait::async_trait]
impl AgentCoreLike for SimpleAgentCore {
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
            dyn std::future::Future<Output = Result<echobot_core::models::LLMResponse>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let messages = build_messages_with_system(
                user_input,
                history,
                extra_system_messages,
                transient_system_messages,
            );
            self.provider
                .generate(
                    &messages,
                    None,
                    Some(&echobot_providers::ToolChoice::Auto),
                    temperature,
                    max_tokens,
                    None,
                )
                .await
                .map_err(|e| Error::Wiring(format!("LLM provider error: {e}")))
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
        _trace_callback: Option<crate::turns::TraceCallback>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::turns::AgentRunResult>> + Send + 'a>,
    > {
        Box::pin(async move {
            let history = history_from_optional(history);
            let messages = build_messages_with_system(
                user_input,
                Some(&history),
                extra_system_messages,
                transient_system_messages,
            );
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
                .map_err(|e| Error::Wiring(format!("LLM provider error: {e}")))?;
            Ok(crate::turns::AgentRunResult {
                response,
                new_messages: Vec::new(),
                history,
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
        _trace_callback: Option<crate::turns::TraceCallback>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::turns::AgentRunResult>> + Send + 'a>,
    > {
        Box::pin(async move {
            let history = history_from_optional(history);
            let messages = build_messages_with_system(
                user_input,
                Some(&history),
                extra_system_messages,
                transient_system_messages,
            );
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
                .map_err(|e| Error::Wiring(format!("LLM provider error: {e}")))?;
            Ok(crate::turns::AgentRunResult {
                response,
                new_messages: Vec::new(),
                history,
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
        _skill_registry: &'a dyn SkillRegistryLike,
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
        _trace_callback: Option<crate::turns::TraceCallback>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::turns::AgentRunResult>> + Send + 'a>,
    > {
        Box::pin(async move {
            let history = history_from_optional(history);
            let messages = build_messages_with_system(
                user_input,
                Some(&history),
                extra_system_messages,
                transient_system_messages,
            );
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
                .map_err(|e| Error::Wiring(format!("LLM provider error: {e}")))?;
            Ok(crate::turns::AgentRunResult {
                response,
                new_messages: Vec::new(),
                history,
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
// Options + context (kept structurally compatible with the original Python
// fields so the CLI assembly can read them as-is).
// ---------------------------------------------------------------------------

/// Options controlling runtime construction. Mirrors the Python dataclass.
#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    /// Path to the `.env` file to load before assembling the runtime.
    pub env_file: String,
    /// Workspace root. `None` means "use the current directory".
    pub workspace: Option<PathBuf>,
    /// Optional sampling-temperature override.
    pub temperature: Option<f32>,
    /// Optional max-tokens override.
    pub max_tokens: Option<u32>,
    /// Optional `delegated_ack_enabled` override.
    pub delegated_ack_enabled: Option<bool>,
    /// Disable built-in tools.
    pub no_tools: bool,
    /// Disable skills.
    pub no_skills: bool,
    /// Disable long-term memory.
    pub no_memory: bool,
    /// Disable the heartbeat service.
    pub no_heartbeat: bool,
    /// Optional heartbeat interval override (seconds).
    pub heartbeat_interval: Option<u64>,
    /// Session to open on startup.
    pub session: Option<String>,
    /// If set, create this new session on startup.
    pub new_session: Option<String>,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            env_file: ".env".to_string(),
            workspace: None,
            temperature: None,
            max_tokens: None,
            delegated_ack_enabled: None,
            no_tools: false,
            no_skills: false,
            no_memory: false,
            no_heartbeat: false,
            heartbeat_interval: None,
            session: None,
            new_session: None,
        }
    }
}

/// The assembled runtime. Each field is wrapped in `Arc<...>` or trait object
/// so the context is cheap to clone and can be moved between async tasks.
///
/// The orchestration-only fields (`coordinator`, `role_registry`,
/// `memory_support`, `tool_registry`, `skill_registry`,
/// `tool_registry_factory`) are populated by the CLI assembly layer (see
/// `echobot_cli::runtime_assembly`).
pub struct RuntimeContext {
    /// Workspace root.
    pub workspace: PathBuf,
    /// Attachment store (images + files).
    pub attachment_store: AttachmentStorePlaceholder,
    /// Whether the LLM accepts image inputs.
    pub supports_image_input: bool,
    /// The agent core (trait object — concrete type lives in the agent
    /// crate once it's ported).
    pub agent: Arc<dyn AgentCoreLike>,
    /// The LLM provider.
    pub provider: Arc<dyn echobot_providers::LLMProvider>,
    /// Visible / persona session store.
    pub session_store: SessionStore,
    /// Background agent session store.
    pub agent_session_store: SessionStore,
    /// Currently active session, or `None` if not loaded.
    pub session: Option<ChatSession>,
    /// Optional tool registry for the active session (populated by CLI).
    pub tool_registry: Option<Box<dyn ToolRegistryLike>>,
    /// Optional skill registry (populated by CLI).
    pub skill_registry: Option<Arc<dyn SkillRegistryLike>>,
    /// Cron scheduler.
    pub cron_service: Arc<CronService>,
    /// Optional heartbeat service.
    pub heartbeat_service: Option<HeartbeatService>,
    /// Session-bound agent runner.
    pub session_runner: Arc<SessionAgentRunner>,
    /// Conversation coordinator (populated by CLI).
    pub coordinator: Option<Arc<dyn CoordinatorPlaceholder>>,
    /// Role card registry (populated by CLI).
    pub role_registry: Option<Arc<dyn RoleRegistryPlaceholder>>,
    /// Long-term memory support (populated by CLI).
    pub memory_support: Option<Arc<dyn MemorySupportPlaceholder>>,
    /// Path of the heartbeat file.
    pub heartbeat_file_path: PathBuf,
    /// Heartbeat interval (seconds).
    pub heartbeat_interval_seconds: u64,
    /// Factory for per-session tool registries (populated by CLI).
    pub tool_registry_factory: Option<ToolRegistryFactoryPlaceholder>,
    /// Mutable runtime controls.
    pub runtime_controls: Arc<tokio::sync::Mutex<RuntimeControls>>,
    /// Effective default config snapshot.
    pub default_runtime_config: RuntimeConfigSnapshot,
    /// Optional settings manager (overrides + persistence).
    pub settings_manager: Option<Arc<RuntimeSettingsManager<RuntimeControlsCoordinator>>>,
    /// Trace store (used by the session runner for per-run JSONL logs).
    pub trace_store: Option<AgentTraceStore>,
}

// ---------------------------------------------------------------------------
// Placeholder trait aliases (kept as `Option<...>` so the runtime crate
// stays orchestration-free).
// ---------------------------------------------------------------------------

/// Placeholder trait the conversation coordinator must implement. The
/// orchestration crate's `ConversationCoordinator` satisfies this contract.
pub trait CoordinatorPlaceholder: Send + Sync {}
/// Placeholder trait the role card registry must implement. The
/// orchestration crate's `RoleCardRegistry` satisfies this contract.
pub trait RoleRegistryPlaceholder: Send + Sync {}
/// Placeholder trait the long-term-memory support must implement. A
/// concrete implementation lands with the memory crate.
pub trait MemorySupportPlaceholder: Send + Sync {}

/// Coordinator-shaped type that bridges the runtime controls to the
/// orchestration `RuntimeSettingsCoordinator` trait.
pub struct RuntimeControlsCoordinator {
    /// Shared mutable controls.
    pub controls: Arc<tokio::sync::Mutex<RuntimeControls>>,
    /// Whether to ack delegations.
    pub delegated_ack_enabled: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for RuntimeControlsCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeControlsCoordinator").finish()
    }
}

impl RuntimeControlsCoordinator {
    /// Creates a new coordinator from the current controls value.
    pub fn new(
        controls: Arc<tokio::sync::Mutex<RuntimeControls>>,
        delegated_ack_enabled: bool,
    ) -> Self {
        Self {
            controls,
            delegated_ack_enabled: std::sync::atomic::AtomicBool::new(delegated_ack_enabled),
        }
    }
}

impl crate::settings::RuntimeSettingsCoordinator for RuntimeControlsCoordinator {
    fn delegated_ack_enabled(&self) -> bool {
        self.delegated_ack_enabled
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn set_delegated_ack_enabled(&mut self, enabled: bool) {
        self.delegated_ack_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

// (no additional re-exports — the placeholder types above cover the public surface.)
