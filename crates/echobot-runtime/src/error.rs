//! Error type for the `echobot-runtime` crate.
//!
//! Aggregates failure modes from the runtime submodules. The variants are kept
//! flat (no deep sub-enums) so the runtime is easy to consume from the CLI
//! and HTTP layers — each variant is paired with a clear `Display` string.

use thiserror::Error;

/// Result alias for runtime operations.
pub type Result<T> = std::result::Result<T, Error>;

/// All errors the runtime crate may surface.
#[derive(Debug, Error)]
pub enum Error {
    /// An I/O error (file or directory operation).
    #[error("runtime I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON (de)serialization error.
    #[error("runtime JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Session name was empty or contained illegal characters.
    #[error("invalid session name: {0}")]
    InvalidSessionName(String),

    /// The session file was empty or unreadable.
    #[error("session not found: {0}")]
    SessionNotFound(String),

    /// The session file's metadata block was malformed.
    #[error("invalid session metadata: {0}")]
    InvalidSessionMetadata(String),

    /// A cron schedule could not be parsed.
    #[error("invalid cron schedule: {0}")]
    InvalidCronSchedule(String),

    /// A cron expression could not be parsed.
    #[error("invalid cron expression: {0}")]
    InvalidCronExpression(String),

    /// The cron job store could not be loaded.
    #[error("invalid cron job store: {0}")]
    InvalidCronStore(String),

    /// A runtime setting name was unknown.
    #[error("unknown runtime setting: {0}")]
    UnknownRuntimeSetting(String),

    /// A runtime setting carried an invalid value.
    #[error("invalid runtime setting value for {name}: {message}")]
    InvalidRuntimeSettingValue { name: String, message: String },

    /// A scheduled job was not found.
    #[error("cron job not found: {0}")]
    CronJobNotFound(String),

    /// A heartbeat file could not be read or written.
    #[error("heartbeat file error: {0}")]
    HeartbeatFile(String),

    /// The runtime is missing a required dependency (provider, agent, ...).
    #[error("runtime wiring error: {0}")]
    Wiring(String),

    /// A request was made against a deleted session.
    #[error("session is deleted: {0}")]
    SessionDeleted(String),

    /// A function or feature is not yet implemented.
    #[error("not implemented: {0}")]
    Unimplemented(&'static str),
}
