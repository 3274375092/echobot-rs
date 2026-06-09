//! `echobot-tools` hosts the tool framework used by the background agent:
//! a `BaseTool` trait, a `ToolRegistry` for discovery, and the built-in
//! tools (filesystem, shell, git, web, cron, media, memory search,
//! planning).
//!
//! ## Layout
//!
//! * [`base`]      — `BaseTool` trait, `ToolResult`, `ToolExecutionOutput`,
//!   `ToolTraceEvent`, `ToolLoopControl`, and the `ToolRegistry`.
//! * [`builtin`]   — `CurrentTimeTool` and `create_basic_tool_registry`.
//! * [`filesystem`] — `ReadTextFileTool`, `WriteTextFileTool`,
//!   `EditTextFileTool`, `ListDirectoryTool`, `SearchFilesTool`,
//!   `SearchTextInFilesTool`.
//! * [`shell`]     — `CommandExecutionTool` + `ShellCommandPolicy`.
//! * [`git`]       — `GitStatusTool`, `GitDiffTool`.
//! * [`web`]       — `WebRequestTool`.
//! * [`cron`]      — `CronTool` (thin wrapper over a runtime
//!   `CronService`).
//! * [`media`]     — `ViewImageTool`, `SendImageToUserTool`,
//!   `SendFileToUserTool`.
//! * [`memory`]    — `MemorySearchTool` (stub; wires to long-term memory
//!   subsystem in a later phase).
//! * [`planning`]  — `UpdatePlanTool`, `RequestUserInputTool`.

pub mod base;
pub mod builtin;
pub mod cron;
pub mod filesystem;
pub mod git;
pub mod media;
pub mod memory;
pub mod planning;
pub mod shell;
pub mod web;

pub use base::{
    truncate_text, BaseTool, ToolExecutionOutput, ToolLoopControl, ToolPayload, ToolRegistry,
    ToolResult, ToolTraceEvent,
};

pub use builtin::{create_basic_tool_registry, BasicToolDeps, CurrentTimeTool};

pub use filesystem::{
    EditTextFileTool, ListDirectoryTool, ReadTextFileTool, SearchFilesTool,
    SearchTextInFilesTool, WritableWorkspaceTool, WorkspaceTool, WriteTextFileTool,
};

pub use shell::{CommandExecutionTool, ShellSafetyMode};

pub use git::{GitDiffTool, GitStatusTool};
pub use web::WebRequestTool;
pub use cron::CronTool;
pub use media::{SendFileToUserTool, SendImageToUserTool, ViewImageTool};
pub use memory::MemorySearchTool;
pub use planning::{RequestUserInputTool, UpdatePlanTool};

// Re-export the `ToolError` enum from `echobot_core` so downstream
// callers can match on tool failures without depending on
// `echobot_core` directly.
pub use echobot_core::ToolError;
