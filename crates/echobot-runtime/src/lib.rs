//! `echobot-runtime` wires the agent runtime together: sessions, settings,
//! scheduling (cron + heartbeat), system prompt construction, the runtime
//! context bootstrap, the per-session agent runner, turn execution, and the
//! agent trace store.
//!
//! ## Layout
//!
//! * [`sessions`]        — `ChatSession`, `SessionInfo`, `SessionStore`.
//! * [`settings`]        — `RuntimeConfigSnapshot`, `RuntimeControls`,
//!   `RuntimeSettings`, `RuntimeSettingsStore`, `RuntimeSettingsManager`.
//! * [`system_prompt`]   — `build_default_system_prompt` + `SystemPromptOptions`.
//! * [`agent_traces`]    — `AgentTraceStore` (JSONL per-run trace logs).
//! * [`session_runner`]  — `SessionAgentRunner`, `SessionRunResult`.
//! * [`turns`]           — `run_agent_turn` + the `AgentCoreLike` /
//!   `ToolRegistryLike` / `SkillRegistryLike` trait aliases used by the
//!   runner. The real `AgentCore` / `ToolRegistry` / `SkillRegistry` types
//!   are not yet ported; this module ships the trait contracts they will
//!   implement.
//! * [`scheduled_tasks`] — `build_cron_job_executor`,
//!   `build_heartbeat_executor`, and the `CoordinatorLike` trait the
//!   orchestrator's `ConversationCoordinator` will implement.
//! * [`scheduling`]      — `cron` + `heartbeat` submodules.
//! * [`bootstrap`]       — `RuntimeOptions`, `RuntimeContext`, and a
//!   `build_runtime_context` STUB. The real wiring will land in the
//!   CLI+finalize phase.
//! * [`error`]           — top-level error type with sub-variants per module.

pub mod agent_traces;
pub mod bootstrap;
pub mod error;
pub mod scheduled_tasks;
pub mod session_runner;
pub mod sessions;
pub mod settings;
pub mod system_prompt;
pub mod turns;

pub mod scheduling;

pub use error::{Error, Result};
