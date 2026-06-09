//! `echobot-orchestration` implements the Decision - Roleplay - Agent
//! coordinator: it routes turns to chat or agent, generates persona
//! replies, tracks background jobs, and exposes the per-session
//! `RouteMode` settings.
//!
//! ## Layout
//!
//! * [`decision`]   — `DecisionEngine`, `RouteDecision`, regex rules, the
//!   decider system prompt. Verbatim port of `decision.py`.
//! * [`roleplay`]   — `RoleplayEngine`, the roleplay system prompt, the
//!   presentation instructions. Verbatim port of `roleplay.py`.
//! * [`coordinator`] — `ConversationCoordinator`, the orchestrator.
//!   Verbatim port of `coordinator.py` (modulo async / lock changes).
//! * [`jobs`]       — `ConversationJob`, `ConversationJobStore`,
//!   `OrchestratedTurnResult`, `CompletionCallback`.
//! * [`roles`]      — `RoleCard`, `RoleCardRegistry`. Discovers role
//!   cards from `echobot/roles/`, `roles/`, and `.echobot/roles/`
//!   (in that order) and auto-creates the default role card on
//!   first run.
//! * [`route_modes`] — `RouteMode` enum, `normalize_route_mode`,
//!   `route_mode_from_metadata`, `set_route_mode`. Verbatim port of
//!   `route_modes.py`.

pub mod coordinator;
pub mod decision;
pub mod jobs;
pub mod roleplay;
pub mod roles;
pub mod route_modes;

pub use coordinator::{
    ConversationCoordinator, BackgroundJobFactory,
    AGENT_HANDOFF_MAX_MESSAGES, AGENT_HANDOFF_MAX_MESSAGE_CHARS, AGENT_HANDOFF_MAX_TOTAL_CHARS,
    PENDING_USER_INPUT_METADATA_KEY,
};
pub use decision::{
    DeciderAgent, DeciderAgentResponse, DecisionEngine, RouteDecision, AGENT_PATTERNS,
    DECISION_SYSTEM_PROMPT, DEFAULT_DECISION_MAX_TOKENS, ENGLISH_REQUEST_PREFIX,
    CHINESE_REQUEST_PREFIX,
};
pub use jobs::{
    CompletionCallback, ConversationJob, ConversationJobStore, JobId,
    ACTIVE_JOB_STATUSES, JOB_CANCELLED_TEXT, JOB_INTERRUPTED_TEXT, RETRYABLE_JOB_STATUSES, job_can_retry,
};
pub use roleplay::{
    ProviderRoleplayLlm, RoleplayEngine, RoleplayLlm, ScheduledCronJobInfo, StreamCallback,
    AGENT_FAILURE_PRESENTATION_INSTRUCTION, AGENT_RESULT_PRESENTATION_INSTRUCTION,
    DEFAULT_ROLEPLAY_MAX_TOKENS, DELEGATED_ACK_INSTRUCTION, DIRECT_CHAT_INSTRUCTION,
    ROLEPLAY_SYSTEM_PROMPT, SCHEDULED_NOTIFICATION_PRESENTATION_INSTRUCTION,
    SCHEDULED_SETUP_PRESENTATION_INSTRUCTION, USER_INPUT_REQUEST_PRESENTATION_INSTRUCTION,
};
pub use roles::{
    default_role_roots, ensure_default_role_card, normalize_role_name, role_name_from_metadata,
    set_role_name, RoleCard, RoleCardRegistry, DEFAULT_ROLE_NAME, DEFAULT_ROLE_PROMPT,
};
pub use route_modes::{
    normalize_route_mode, route_mode_from_metadata, set_route_mode, RouteMode, DEFAULT_ROUTE_MODE,
    ROUTE_MODE_VALUES,
};
