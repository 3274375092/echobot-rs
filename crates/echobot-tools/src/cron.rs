//! `CronTool` — thin wrapper over a runtime `CronService`.
//!
//! The runtime crate owns the actual cron engine and the persisted
//! job store; this module just exposes the JSON tool surface that the
//! LLM can call. The dependency is passed as a trait object so the
//! runtime crate can plug in its concrete implementation without
//! forcing `echobot-tools` to depend on it.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use echobot_core::Error;

use crate::base::{optional_string, require_string, BaseTool, ToolExecutionOutput};

// ---------------------------------------------------------------------------
// CronService trait
// ---------------------------------------------------------------------------

/// Public summary of a single cron job — the same shape the Python
/// `summarize_job` helper returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobSummary {
    /// Job id.
    pub id: String,
    /// User-supplied job name.
    pub name: String,
    /// Schedule kind: `at`, `every`, `cron`.
    pub schedule_kind: String,
    /// Schedule value (`at` = ISO timestamp, `every` = seconds, `cron` = expression).
    pub schedule_value: String,
    /// Optional IANA timezone (cron expressions only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Task kind: `agent` or `text`.
    pub task_kind: String,
    /// Job content / instructions.
    pub content: String,
    /// Session name used when the job runs.
    pub session_name: String,
    /// Whether the job is enabled.
    pub enabled: bool,
    /// Whether the job should be deleted after its first run.
    #[serde(default)]
    pub delete_after_run: bool,
}

/// A cron job owned by the runtime's `CronService`.
#[derive(Debug, Clone)]
pub struct CronJob {
    pub summary: CronJobSummary,
}

/// The runtime's `CronService` API, narrowed to the surface that the
/// cron tool needs. The runtime crate provides a concrete
/// implementation in a follow-up phase.
#[async_trait]
pub trait CronService: Send + Sync {
    /// Adds a new job.
    async fn add_job(
        &self,
        name: &str,
        schedule: &CronSchedule,
        payload: &CronPayload,
        delete_after_run: bool,
    ) -> Result<CronJob, Error>;

    /// Lists jobs, optionally including disabled ones.
    async fn list_jobs(&self, include_disabled: bool) -> Result<Vec<CronJob>, Error>;

    /// Removes a job by id; returns true if something was removed.
    async fn remove_job(&self, job_id: &str) -> Result<bool, Error>;

    /// Runs a job immediately.
    async fn run_job(&self, job_id: &str, force: bool) -> Result<bool, Error>;

    /// Enables / disables a job.
    async fn set_enabled(&self, job_id: &str, enabled: bool) -> Result<Option<CronJob>, Error>;
}

/// Schedule descriptor.
#[derive(Debug, Clone)]
pub struct CronSchedule {
    /// `"at"` / `"every"` / `"cron"`.
    pub kind: String,
    /// `at`: ISO timestamp; `every`: interval seconds; `cron`: expression.
    pub value: String,
    /// Optional IANA timezone (cron only).
    pub timezone: Option<String>,
}

impl CronSchedule {
    /// Builds a one-shot schedule.
    pub fn at(iso: impl Into<String>) -> Self {
        Self {
            kind: "at".to_string(),
            value: iso.into(),
            timezone: None,
        }
    }

    /// Builds an interval schedule.
    pub fn every(seconds: u64) -> Self {
        Self {
            kind: "every".to_string(),
            value: seconds.to_string(),
            timezone: None,
        }
    }

    /// Builds a cron expression schedule.
    pub fn cron(expr: impl Into<String>, timezone: Option<String>) -> Self {
        Self {
            kind: "cron".to_string(),
            value: expr.into(),
            timezone,
        }
    }
}

/// Payload carried by a job.
#[derive(Debug, Clone)]
pub struct CronPayload {
    /// `agent` / `text`.
    pub kind: String,
    /// Job content / instructions.
    pub content: String,
    /// Session name used when the job runs.
    pub session_name: String,
}

impl CronPayload {
    /// Builds a payload.
    pub fn new(kind: impl Into<String>, content: impl Into<String>, session_name: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            content: content.into(),
            session_name: session_name.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// CronTool
// ---------------------------------------------------------------------------

/// The cron management tool exposed to the LLM.
pub struct CronTool {
    service: Arc<dyn CronService>,
    session_name: String,
    allow_mutations: bool,
}

impl CronTool {
    /// Creates a new tool.
    pub fn new(
        service: Arc<dyn CronService>,
        session_name: impl Into<String>,
        allow_mutations: bool,
    ) -> Self {
        Self {
            service,
            session_name: session_name.into(),
            allow_mutations,
        }
    }
}

#[async_trait]
impl BaseTool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Manage scheduled jobs. Use it for exact or one-time reminders. Use delay_seconds for reminders like 'in 20 seconds'. Use every_seconds only for repeating jobs. For loose periodic checklists, edit HEARTBEAT.md instead."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "remove", "run", "enable", "disable"]
                },
                "name": { "type": "string", "description": "Job name. Optional for add." },
                "content": { "type": "string", "description": "Task text for the job." },
                "task_type": {
                    "type": "string",
                    "enum": ["agent", "text"],
                    "description": "agent = run through the model, text = send fixed text."
                },
                "every_seconds": { "type": "integer", "minimum": 1, "description": "Repeat interval in seconds for recurring jobs only." },
                "delay_seconds": { "type": "integer", "minimum": 1, "description": "One-time delay in seconds. Prefer this for reminders like 'in 20 seconds'." },
                "cron_expr": { "type": "string", "description": "Five-field cron expression like '0 9 * * 1-5'." },
                "timezone": { "type": "string", "description": "IANA timezone used with cron_expr." },
                "at": { "type": "string", "description": "One-time ISO datetime. If no timezone is given, local time is used." },
                "job_id": { "type": "string", "description": "Job id for remove/run/enable/disable." },
                "session_name": { "type": "string", "description": "Session name used by the scheduled run." },
                "include_disabled": { "type": "boolean", "description": "Include disabled jobs when listing." }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let action = optional_string(&arguments, "action", "").to_string();
        match action.as_str() {
            "add" => self.add_job(arguments).await,
            "list" => self.list_jobs(arguments).await,
            "remove" => self.remove_job(arguments).await,
            "run" => self.run_job(arguments).await,
            "enable" => self.set_enabled(arguments, true).await,
            "disable" => self.set_enabled(arguments, false).await,
            other => Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "action".to_string(),
                message: format!("Unsupported cron action: {other}"),
            })),
        }
    }
}

impl CronTool {
    fn require_mutations(&self) -> Result<(), Error> {
        if self.allow_mutations {
            Ok(())
        } else {
            Err(Error::Tool(crate::base::ToolError::Blocked(
                "cron mutations are disabled while a scheduled task is running".to_string(),
            )))
        }
    }

    async fn add_job(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        self.require_mutations()?;
        let content = require_string(&arguments, "content").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let schedule = self.build_schedule(&arguments)?;
        let task_type = optional_string(&arguments, "task_type", "agent").to_string();
        let name = {
            let raw = optional_string(&arguments, "name", "").to_string();
            if raw.is_empty() {
                default_job_name(content)
            } else {
                raw
            }
        };
        let session_name = {
            let raw = optional_string(&arguments, "session_name", "").to_string();
            if raw.is_empty() {
                self.session_name.clone()
            } else {
                raw
            }
        };
        let delete_after_run = schedule.kind == "at";

        let job = self
            .service
            .add_job(
                &name,
                &schedule,
                &CronPayload::new(task_type, content.to_string(), session_name),
                delete_after_run,
            )
            .await?;
        Ok(ToolExecutionOutput::from_payload(json!({
            "created": true,
            "job": job.summary,
        })))
    }

    async fn list_jobs(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let include_disabled = arguments
            .get("include_disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let jobs = self.service.list_jobs(include_disabled).await?;
        let summaries: Vec<CronJobSummary> = jobs.into_iter().map(|j| j.summary).collect();
        Ok(ToolExecutionOutput::from_payload(json!({ "jobs": summaries })))
    }

    async fn remove_job(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        self.require_mutations()?;
        let job_id = require_string(&arguments, "job_id").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let removed = self.service.remove_job(job_id).await?;
        Ok(ToolExecutionOutput::from_payload(json!({
            "removed": removed,
            "job_id": job_id,
        })))
    }

    async fn run_job(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        self.require_mutations()?;
        let job_id = require_string(&arguments, "job_id").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let started = self.service.run_job(job_id, true).await?;
        Ok(ToolExecutionOutput::from_payload(json!({
            "started": started,
            "job_id": job_id,
        })))
    }

    async fn set_enabled(&self, arguments: Value, enabled: bool) -> Result<ToolExecutionOutput, Error> {
        self.require_mutations()?;
        let job_id = require_string(&arguments, "job_id").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let job = self.service.set_enabled(job_id, enabled).await?;
        Ok(ToolExecutionOutput::from_payload(json!({
            "updated": job.is_some(),
            "job": job.map(|j| j.summary),
        })))
    }

    fn build_schedule(&self, arguments: &Value) -> Result<CronSchedule, Error> {
        let every_seconds = arguments.get("every_seconds").and_then(Value::as_i64);
        let delay_seconds = arguments.get("delay_seconds").and_then(Value::as_i64);
        let cron_expr = optional_string(arguments, "cron_expr", "").to_string();
        let at = optional_string(arguments, "at", "").to_string();
        let timezone = {
            let raw = optional_string(arguments, "timezone", "").to_string();
            if raw.is_empty() { None } else { Some(raw) }
        };

        let chosen = (delay_seconds.is_some() as i32)
            + (every_seconds.is_some() as i32)
            + (!cron_expr.is_empty() as i32)
            + (!at.is_empty() as i32);
        if chosen != 1 {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "schedule".to_string(),
                message: "Exactly one of delay_seconds, every_seconds, cron_expr, or at is required"
                    .to_string(),
            }));
        }
        if let Some(d) = delay_seconds {
            if d <= 0 {
                return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "delay_seconds".to_string(),
                    message: "delay_seconds must be greater than 0".to_string(),
                }));
            }
            return Ok(CronSchedule::at(delay_to_iso(d)));
        }
        if let Some(e) = every_seconds {
            if e <= 0 {
                return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "every_seconds".to_string(),
                    message: "every_seconds must be greater than 0".to_string(),
                }));
            }
            return Ok(CronSchedule::every(e as u64));
        }
        if !cron_expr.is_empty() {
            return Ok(CronSchedule::cron(cron_expr, timezone));
        }
        Ok(CronSchedule::at(normalize_iso_datetime(&at)))
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

fn default_job_name(content: &str) -> String {
    let trimmed = content.trim();
    let head: String = trimmed.chars().take(40).collect();
    let head = head.trim();
    if head.is_empty() {
        "scheduled-job".to_string()
    } else {
        head.to_string()
    }
}

fn normalize_iso_datetime(value: &str) -> String {
    let parsed = DateTime::parse_from_rfc3339(value)
        .or_else(|_| {
            // Try common alternate form (Python `datetime.fromisoformat`).
            DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f%:z")
                .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%:z"))
                .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f"))
                .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        })
        .unwrap_or_else(|_| Local::now().into());
    parsed.with_timezone(&Local).to_rfc3339()
}

fn delay_to_iso(delay_seconds: i64) -> String {
    let target = Local::now() + Duration::seconds(delay_seconds);
    target.to_rfc3339()
}
