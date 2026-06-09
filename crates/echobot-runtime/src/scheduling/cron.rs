//! Cron scheduler: stores, schedules, and dispatches jobs.
//!
//! Mirrors `echobot/scheduling/cron/*`.
//!
//! The scheduler is a single in-process task: a loop that wakes up on the
//! next due job (or every [`CronService::poll_interval_seconds`], whichever
//! is sooner), runs any due jobs through the registered `on_job` closure,
//! then reschedules them.
//!
//! ## File layout
//!
//! Jobs are persisted to `<store_path>` (defaults to
//! `<workspace>/.echobot/cron/jobs.json`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::error::{Error, Result};

/// Schedule kind discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CronScheduleKind {
    /// One-shot at a specific timestamp.
    At,
    /// Periodic every N seconds.
    Every,
    /// Cron expression.
    Cron,
}

impl CronScheduleKind {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "at" => Ok(Self::At),
            "every" => Ok(Self::Every),
            "cron" => Ok(Self::Cron),
            other => Err(Error::InvalidCronSchedule(format!("unknown kind: {other}"))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::At => "at",
            Self::Every => "every",
            Self::Cron => "cron",
        }
    }
}

/// Cron schedule definition. The Rust struct uses an enum for `kind` instead
/// of a string literal — the on-disk format still uses the lowercase string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronSchedule {
    /// Schedule kind.
    pub kind: CronScheduleKind,
    /// ISO 8601 timestamp for `kind = "at"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Period in seconds for `kind = "every"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_seconds: Option<u64>,
    /// Cron expression for `kind = "cron"` (5 fields).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    /// Optional IANA timezone for `kind = "cron"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl Default for CronSchedule {
    fn default() -> Self {
        Self {
            kind: CronScheduleKind::Every,
            at: None,
            every_seconds: None,
            expr: None,
            timezone: None,
        }
    }
}

impl CronSchedule {
    /// Parses a schedule from a JSON value (e.g. when reading from disk).
    pub fn from_value(data: &Map<String, Value>) -> Result<Self> {
        let kind_str = data
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("every");
        let kind = CronScheduleKind::parse(kind_str)?;
        Ok(Self {
            kind,
            at: optional_text(data.get("at")),
            every_seconds: optional_int(data.get("every_seconds")),
            expr: optional_text(data.get("expr")),
            timezone: optional_text(data.get("timezone")),
        })
    }

    /// Serializes the schedule back to a JSON object.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("kind".into(), Value::String(self.kind.as_str().to_string()));
        if let Some(at) = &self.at {
            map.insert("at".into(), Value::String(at.clone()));
        }
        if let Some(secs) = self.every_seconds {
            map.insert("every_seconds".into(), Value::from(secs));
        }
        if let Some(expr) = &self.expr {
            map.insert("expr".into(), Value::String(expr.clone()));
        }
        if let Some(tz) = &self.timezone {
            map.insert("timezone".into(), Value::String(tz.clone()));
        }
        Value::Object(map)
    }
}

/// Payload type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CronPayloadKind {
    /// Delegate to the agent.
    Agent,
    /// Send a fixed text message.
    Text,
}

impl CronPayloadKind {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "agent" => Ok(Self::Agent),
            "text" => Ok(Self::Text),
            other => Err(Error::InvalidCronSchedule(format!(
                "unknown payload kind: {other}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Text => "text",
        }
    }
}

/// What the cron job should do when it fires.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronPayload {
    /// Payload kind.
    pub kind: CronPayloadKind,
    /// Prompt / message content.
    #[serde(default)]
    pub content: String,
    /// Target session name.
    #[serde(default = "default_session_name")]
    pub session_name: String,
}

fn default_session_name() -> String {
    "default".to_string()
}

impl Default for CronPayload {
    fn default() -> Self {
        Self {
            kind: CronPayloadKind::Agent,
            content: String::new(),
            session_name: default_session_name(),
        }
    }
}

impl CronPayload {
    /// Parses from a JSON object.
    pub fn from_value(data: &Map<String, Value>) -> Result<Self> {
        let kind_str = data.get("kind").and_then(Value::as_str).unwrap_or("agent");
        let kind = CronPayloadKind::parse(kind_str)?;
        Ok(Self {
            kind,
            content: data
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            session_name: data
                .get("session_name")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_string(),
        })
    }

    /// Serializes to a JSON object.
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "kind": self.kind.as_str(),
            "content": self.content,
            "session_name": self.session_name,
        })
    }
}

/// Runtime state of a cron job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronJobState {
    /// Next scheduled run (ISO 8601 with seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    /// Last run timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    /// Last run status: `"ok" | "error" | "running" | "skipped"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    /// Last error message, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Default for CronJobState {
    fn default() -> Self {
        Self {
            next_run_at: None,
            last_run_at: None,
            last_status: None,
            last_error: None,
        }
    }
}

impl CronJobState {
    fn from_value(data: &Map<String, Value>) -> Self {
        Self {
            next_run_at: optional_text(data.get("next_run_at")),
            last_run_at: optional_text(data.get("last_run_at")),
            last_status: optional_text(data.get("last_status")),
            last_error: optional_text(data.get("last_error")),
        }
    }

    fn to_value(&self) -> Value {
        serde_json::json!({
            "next_run_at": self.next_run_at,
            "last_run_at": self.last_run_at,
            "last_status": self.last_status,
            "last_error": self.last_error,
        })
    }
}

/// A single cron job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronJob {
    /// 8-character job id.
    pub id: String,
    /// Human-readable job name.
    pub name: String,
    /// Whether the job is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Schedule definition.
    #[serde(default)]
    pub schedule: CronSchedule,
    /// What to do when the job fires.
    #[serde(default)]
    pub payload: CronPayload,
    /// Runtime state (next/last run timestamps etc).
    #[serde(default)]
    pub state: CronJobState,
    /// Creation timestamp.
    #[serde(default)]
    pub created_at: String,
    /// Last-modified timestamp.
    #[serde(default)]
    pub updated_at: String,
    /// For `kind = "at"` jobs: delete the job after it has fired.
    #[serde(default)]
    pub delete_after_run: bool,
}

fn default_true() -> bool {
    true
}

impl CronJob {
    /// Parses from a JSON object.
    pub fn from_value(data: &Map<String, Value>) -> Result<Self> {
        let schedule = match data.get("schedule") {
            Some(Value::Object(obj)) => CronSchedule::from_value(obj)?,
            _ => CronSchedule::default(),
        };
        let payload = match data.get("payload") {
            Some(Value::Object(obj)) => CronPayload::from_value(obj)?,
            _ => CronPayload::default(),
        };
        let state = match data.get("state") {
            Some(Value::Object(obj)) => CronJobState::from_value(obj),
            _ => CronJobState::default(),
        };
        Ok(Self {
            id: data.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
            name: data.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            enabled: data.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            schedule,
            payload,
            state,
            created_at: data.get("created_at").and_then(Value::as_str).unwrap_or("").to_string(),
            updated_at: data.get("updated_at").and_then(Value::as_str).unwrap_or("").to_string(),
            delete_after_run: data
                .get("delete_after_run")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Serializes to a JSON object.
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "enabled": self.enabled,
            "schedule": self.schedule.to_value(),
            "payload": self.payload.to_value(),
            "state": self.state.to_value(),
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "delete_after_run": self.delete_after_run,
        })
    }
}

/// Container persisted to disk as `<store_path>`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CronStore {
    /// Schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// All known jobs.
    #[serde(default)]
    pub jobs: Vec<CronJob>,
}

fn default_version() -> u32 {
    1
}

impl CronStore {
    /// Parses from a JSON value.
    pub fn from_value(data: &Value) -> Result<Self> {
        let obj = data
            .as_object()
            .ok_or_else(|| Error::InvalidCronStore("root must be an object".into()))?;
        let mut jobs: Vec<CronJob> = Vec::new();
        if let Some(Value::Array(items)) = obj.get("jobs") {
            for item in items {
                if let Value::Object(map) = item {
                    if let Ok(job) = CronJob::from_value(map) {
                        jobs.push(job);
                    }
                }
            }
        }
        Ok(Self {
            version: obj
                .get("version")
                .and_then(Value::as_u64)
                .map(|n| n as u32)
                .unwrap_or(1),
            jobs,
        })
    }

    /// Serializes to a JSON value.
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "version": self.version,
            "jobs": self.jobs.iter().map(|j| j.to_value()).collect::<Vec<_>>(),
        })
    }
}

/// Job executor signature. Receives a deep-copied job and returns the
/// response text (or `None` if nothing to show).
pub type JobExecutor = Arc<dyn Fn(CronJob) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<String>>> + Send>> + Send + Sync>;

/// In-process cron scheduler.
pub struct CronService {
    /// Path of the on-disk store.
    pub store_path: PathBuf,
    /// Closure invoked when a job fires.
    pub on_job: Option<JobExecutor>,
    /// Lower bound on the sleep between schedule scans (seconds).
    pub poll_interval_seconds: f64,
    state: Arc<Mutex<CronServiceState>>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

struct CronServiceState {
    store: CronStore,
    running: bool,
}

impl std::fmt::Debug for CronService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CronService")
            .field("store_path", &self.store_path)
            .field(
                "running",
                &self.state.try_lock().map(|g| g.running).unwrap_or(false),
            )
            .finish()
    }
}

impl CronService {
    /// Creates a new cron service. The on-disk store is loaded on
    /// [`start`](Self::start).
    pub fn new(
        store_path: impl Into<PathBuf>,
        on_job: Option<JobExecutor>,
        poll_interval_seconds: Option<f64>,
    ) -> Self {
        Self {
            store_path: store_path.into(),
            on_job,
            poll_interval_seconds: poll_interval_seconds.unwrap_or(1.0),
            state: Arc::new(Mutex::new(CronServiceState {
                store: CronStore::default(),
                running: false,
            })),
            task: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts the scheduler. Loads the on-disk store and spawns the loop.
    pub async fn start(&self) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            if state.running {
                return Ok(());
            }
            state.store = load_store(&self.store_path).unwrap_or_default();
            recompute_next_runs(&mut state.store);
            save_store(&self.store_path, &state.store)?;
            state.running = true;
        }
        let on_job = self.on_job.clone();
        let state = self.state.clone();
        let poll_interval = self.poll_interval_seconds;
        let store_path = self.store_path.clone();
        let handle = tokio::spawn(async move {
            run_loop(state, on_job, poll_interval, store_path).await;
        });
        *self.task.lock().await = Some(handle);
        Ok(())
    }

    /// Stops the scheduler, cancelling the loop task.
    pub async fn stop(&self) {
        let mut state_guard = self.state.lock().await;
        state_guard.running = false;
        drop(state_guard);
        let task = { self.task.lock().await.take() };
        if let Some(handle) = task {
            handle.abort();
            let _ = handle.await;
        }
    }

    /// Returns a deep-copied snapshot of every enabled job.
    pub async fn list_jobs(&self, include_disabled: bool) -> Vec<CronJob> {
        let state = self.state.lock().await;
        let mut jobs: Vec<CronJob> = state
            .store
            .jobs
            .iter()
            .filter(|j| include_disabled || j.enabled)
            .cloned()
            .collect();
        jobs.sort_by(|a, b| a.state.next_run_at.cmp(&b.state.next_run_at));
        jobs
    }

    /// Returns a deep-copied snapshot of a single job, or `None` if not
    /// found.
    pub async fn get_job(&self, job_id: &str) -> Option<CronJob> {
        let state = self.state.lock().await;
        state
            .store
            .jobs
            .iter()
            .find(|j| j.id == job_id)
            .cloned()
    }

    /// Adds a new job to the store. Returns the persisted job.
    pub async fn add_job(
        &self,
        name: &str,
        schedule: CronSchedule,
        payload: CronPayload,
        delete_after_run: bool,
    ) -> Result<CronJob> {
        let normalized = normalize_schedule(&schedule)?;
        let now = now_text();
        let next_run = compute_next_run(&normalized, None);
        let id = generate_job_id();
        let job = CronJob {
            id: id.clone(),
            name: name.to_string(),
            enabled: true,
            schedule: normalized,
            payload,
            state: CronJobState {
                next_run_at: next_run.as_ref().map(format_datetime_ref),
                ..CronJobState::default()
            },
            created_at: now.clone(),
            updated_at: now,
            delete_after_run,
        };
        let mut state = self.state.lock().await;
        state.store.jobs.push(job.clone());
        save_store(&self.store_path, &state.store)?;
        Ok(job)
    }

    /// Removes a job by id. Returns `true` if a job was removed.
    pub async fn remove_job(&self, job_id: &str) -> Result<bool> {
        let mut state = self.state.lock().await;
        let before = state.store.jobs.len();
        state.store.jobs.retain(|j| j.id != job_id);
        let removed = state.store.jobs.len() != before;
        if removed {
            save_store(&self.store_path, &state.store)?;
        }
        Ok(removed)
    }

    /// Enables or disables a job. Returns the updated job.
    pub async fn set_enabled(&self, job_id: &str, enabled: bool) -> Result<Option<CronJob>> {
        let mut state = self.state.lock().await;
        let job_snapshot: Option<CronJob> = {
            let job = match state.store.jobs.iter_mut().find(|j| j.id == job_id) {
                Some(j) => j,
                None => return Ok(None),
            };
            job.enabled = enabled;
            job.updated_at = now_text();
            if enabled {
                job.state.next_run_at =
                    compute_next_run(&job.schedule, None).map(format_datetime_owned);
            } else {
                job.state.next_run_at = None;
            }
            Some(job.clone())
        };
        save_store(&self.store_path, &state.store)?;
        Ok(job_snapshot)
    }

    /// Forces a job to run immediately. Returns `Ok(true)` if the job exists.
    pub async fn run_job(&self, job_id: &str, force: bool) -> Result<bool> {
        let job = {
            let state = self.state.lock().await;
            state.store.jobs.iter().find(|j| j.id == job_id).cloned()
        };
        let Some(job) = job else {
            return Ok(false);
        };
        if !force && !job.enabled {
            return Ok(false);
        }
        self.execute_job(job_id).await;
        Ok(true)
    }

    /// Returns a JSON-shaped status snapshot.
    pub async fn status(&self) -> StatusReport {
        let state = self.state.lock().await;
        StatusReport {
            enabled: state.running,
            jobs: state.store.jobs.len(),
            next_run_at: next_run_at(&state.store),
        }
    }

    async fn execute_job(&self, job_id: &str) {
        let job_copy = {
            let mut state = self.state.lock().await;
            let Some(job) = state.store.jobs.iter_mut().find(|j| j.id == job_id) else {
                return;
            };
            job.state.last_status = Some("running".to_string());
            job.state.last_error = None;
            job.updated_at = now_text();
            let copy = job.clone();
            if let Err(e) = save_store(&self.store_path, &state.store) {
                tracing::warn!(error = %e, "failed to save cron store");
            }
            copy
        };
        let mut error_text: Option<String> = None;
        if let Some(executor) = &self.on_job {
            match executor(job_copy.clone()).await {
                Ok(_) => {}
                Err(e) => error_text = Some(e.to_string()),
            }
        }
        let mut state = self.state.lock().await;
        let Some(job) = state.store.jobs.iter_mut().find(|j| j.id == job_id) else {
            return;
        };
        let now = Utc::now();
        job.state.last_run_at = Some(format_datetime_ref(&now));
        job.updated_at = now_text();
        if let Some(err) = &error_text {
            job.state.last_status = Some("error".to_string());
            job.state.last_error = Some(err.clone());
        } else {
            job.state.last_status = Some("ok".to_string());
            job.state.last_error = None;
        }
        if job.schedule.kind == CronScheduleKind::At {
            if job.delete_after_run {
                state.store.jobs.retain(|j| j.id != job_id);
            } else {
                job.enabled = false;
                job.state.next_run_at = None;
            }
        } else {
            job.state.next_run_at =
                compute_next_run(&job.schedule, Some(now)).map(format_datetime_owned);
        }
        if let Err(e) = save_store(&self.store_path, &state.store) {
            tracing::warn!(error = %e, "failed to save cron store");
        }
    }
}

/// JSON-friendly status snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    /// Whether the loop is running.
    pub enabled: bool,
    /// Number of jobs in the store.
    pub jobs: usize,
    /// Next scheduled run across all enabled jobs.
    pub next_run_at: Option<String>,
}

impl StatusReport {
    /// Returns the status as a JSON value.
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "enabled": self.enabled,
            "jobs": self.jobs,
            "next_run_at": self.next_run_at,
        })
    }
}

// ---------------------------------------------------------------------------
// Schedule parsing & next-run computation
// ---------------------------------------------------------------------------

/// Normalizes a schedule (validates fields, applies defaults).
pub fn normalize_schedule(schedule: &CronSchedule) -> Result<CronSchedule> {
    match schedule.kind {
        CronScheduleKind::At => {
            let at = schedule
                .at
                .as_ref()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| Error::InvalidCronSchedule("at schedule requires 'at'".into()))?;
            parse_datetime_string(at)?;
            Ok(schedule.clone())
        }
        CronScheduleKind::Every => {
            let secs = schedule.every_seconds.ok_or_else(|| {
                Error::InvalidCronSchedule("every schedule requires every_seconds".into())
            })?;
            if secs == 0 {
                return Err(Error::InvalidCronSchedule(
                    "every schedule requires every_seconds > 0".into(),
                ));
            }
            Ok(schedule.clone())
        }
        CronScheduleKind::Cron => {
            let expr = schedule
                .expr
                .as_ref()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| Error::InvalidCronSchedule("cron schedule requires expr".into()))?;
            if expr.split_whitespace().count() != 5 {
                return Err(Error::InvalidCronSchedule(
                    "cron expression must have 5 fields".into(),
                ));
            }
            parse_cron_expression(expr)?;
            Ok(schedule.clone())
        }
    }
}

/// Computes the next scheduled run. Returns `None` for past `at` schedules.
pub fn compute_next_run(
    schedule: &CronSchedule,
    now: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    let now = ensure_aware(now.unwrap_or_else(Utc::now));
    let normalized = match normalize_schedule(schedule) {
        Ok(s) => s,
        Err(_) => return None,
    };
    match normalized.kind {
        CronScheduleKind::At => {
            let at = normalized.at.as_deref().unwrap_or("");
            match parse_datetime_string(at) {
                Ok(dt) if dt > now => Some(dt),
                _ => None,
            }
        }
        CronScheduleKind::Every => Some(
            now + chrono::Duration::seconds(normalized.every_seconds.unwrap_or(0) as i64),
        ),
        CronScheduleKind::Cron => {
            let expr = normalized.expr.as_deref().unwrap_or("");
            let cron = match parse_cron_expression(expr) {
                Ok(c) => c,
                Err(_) => return None,
            };
            // Scan up to a year of minutes.
            let mut candidate = (now + chrono::Duration::minutes(1))
                .with_second(0)
                .and_then(|d| d.with_nanosecond(0))
                .unwrap_or(now);
            for _ in 0..(366 * 24 * 60) {
                if cron.matches(&candidate) {
                    return Some(candidate);
                }
                candidate += chrono::Duration::minutes(1);
            }
            None
        }
    }
}

/// Returns a human-readable description of `schedule`.
pub fn describe_schedule(schedule: &CronSchedule) -> String {
    match schedule.kind {
        CronScheduleKind::At => format!("at {}", schedule.at.as_deref().unwrap_or("")),
        CronScheduleKind::Every => format!(
            "every {}s",
            schedule.every_seconds.unwrap_or(0)
        ),
        CronScheduleKind::Cron => {
            let tz = schedule
                .timezone
                .as_deref()
                .map(|t| format!(" ({t})"))
                .unwrap_or_default();
            format!(
                "cron {}{}",
                schedule.expr.as_deref().unwrap_or(""),
                tz
            )
        }
    }
}

/// Returns a JSON summary of a job, suitable for tool output.
pub fn summarize_job(job: &CronJob) -> Value {
    serde_json::json!({
        "id": job.id,
        "name": job.name,
        "enabled": job.enabled,
        "schedule": describe_schedule(&job.schedule),
        "payload_kind": job.payload.kind.as_str(),
        "session_name": job.payload.session_name,
        "next_run_at": job.state.next_run_at,
        "last_run_at": job.state.last_run_at,
        "last_status": job.state.last_status,
        "last_error": job.state.last_error,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn optional_text(value: Option<&Value>) -> Option<String> {
    let s = value?.as_str()?.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn optional_int(value: Option<&Value>) -> Option<u64> {
    let v = value?;
    if v.is_null() {
        return None;
    }
    v.as_u64()
}

fn ensure_aware(value: DateTime<Utc>) -> DateTime<Utc> {
    value
}

fn parse_datetime_string(value: &str) -> Result<DateTime<Utc>> {
    let trimmed = value.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S") {
        return Ok(Utc.from_utc_datetime(&naive));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return Ok(Utc.from_utc_datetime(&naive));
    }
    Err(Error::InvalidCronSchedule(format!(
        "could not parse datetime: {value}"
    )))
}

fn now_text() -> String {
    format_datetime_ref(&Utc::now())
}

fn format_datetime_ref(value: &DateTime<Utc>) -> String {
    format_datetime_owned(*value)
}

fn format_datetime_owned(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn generate_job_id() -> String {
    use std::fmt::Write;
    let bytes = uuid::Uuid::new_v4();
    let mut out = String::with_capacity(8);
    for byte in bytes.as_bytes().iter().take(4) {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn load_store(path: &Path) -> Option<CronStore> {
    if !path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    CronStore::from_value(&value).ok()
}

fn save_store(path: &Path, store: &CronStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let value = store.to_value();
    let text = serde_json::to_string_pretty(&value)?;
    std::fs::write(path, text)?;
    Ok(())
}

fn next_run_at(store: &CronStore) -> Option<String> {
    store
        .jobs
        .iter()
        .filter(|j| j.enabled)
        .filter_map(|j| j.state.next_run_at.clone())
        .min()
}

fn recompute_next_runs(store: &mut CronStore) {
    let now = Utc::now();
    let mut retained: Vec<CronJob> = Vec::new();
    for mut job in store.jobs.drain(..) {
        if !job.enabled {
            job.state.next_run_at = None;
            retained.push(job);
            continue;
        }
        if normalize_schedule(&job.schedule).is_err() {
            retained.push(job);
            continue;
        }
        let next = compute_next_run(&job.schedule, Some(now));
        if job.schedule.kind == CronScheduleKind::At && next.is_none() {
            job.state.next_run_at = None;
            if job.delete_after_run {
                continue;
            }
            job.enabled = false;
            job.updated_at = now_text();
            retained.push(job);
            continue;
        }
        job.state.next_run_at = next.map(format_datetime_owned);
        retained.push(job);
    }
    store.jobs = retained;
}

async fn run_loop(
    state: Arc<Mutex<CronServiceState>>,
    on_job: Option<JobExecutor>,
    poll_interval: f64,
    store_path: PathBuf,
) {
    loop {
        let due_job_ids: Vec<String> = {
            let state_guard = state.lock().await;
            if !state_guard.running {
                break;
            }
            let now = Utc::now();
            state_guard
                .store
                .jobs
                .iter()
                .filter(|j| j.enabled)
                .filter_map(|j| {
                    j.state
                        .next_run_at
                        .as_ref()
                        .and_then(|s| parse_datetime_string(s).ok())
                })
                .zip(state_guard.store.jobs.iter())
                .filter(|(dt, _)| *dt <= now)
                .map(|(_, j)| j.id.clone())
                .collect()
        };
        for job_id in due_job_ids {
            if let Some(executor) = &on_job {
                let job_copy = {
                    let state_guard = state.lock().await;
                    state_guard
                        .store
                        .jobs
                        .iter()
                        .find(|j| j.id == job_id)
                        .cloned()
                };
                let Some(job) = job_copy else {
                    continue;
                };
                let result = executor(job.clone()).await;
                let mut state_guard = state.lock().await;
                let Some(job) = state_guard.store.jobs.iter_mut().find(|j| j.id == job_id) else {
                    continue;
                };
                let now = Utc::now();
                job.state.last_run_at = Some(format_datetime_ref(&now));
                job.updated_at = now_text();
                match result {
                    Ok(_) => {
                        job.state.last_status = Some("ok".to_string());
                        job.state.last_error = None;
                    }
                    Err(e) => {
                        job.state.last_status = Some("error".to_string());
                        job.state.last_error = Some(e.to_string());
                    }
                }
                if job.schedule.kind == CronScheduleKind::At {
                    if job.delete_after_run {
                        state_guard.store.jobs.retain(|j| j.id != job_id);
                    } else {
                        job.enabled = false;
                        job.state.next_run_at = None;
                    }
                } else {
                    job.state.next_run_at =
                        compute_next_run(&job.schedule, Some(now)).map(format_datetime_owned);
                }
                if let Err(e) = save_store(&store_path, &state_guard.store) {
                    tracing::warn!(error = %e, "failed to save cron store");
                }
            }
        }
        let sleep_secs = sleep_seconds(&state, poll_interval).await;
        tokio::time::sleep(Duration::from_secs_f64(sleep_secs)).await;
    }
}

async fn sleep_seconds(
    state: &Arc<Mutex<CronServiceState>>,
    poll_interval: f64,
) -> f64 {
    let state_guard = state.lock().await;
    let next_run_at = next_run_at(&state_guard.store);
    let base = poll_interval.max(1.0);
    let Some(next) = next_run_at.and_then(|s| parse_datetime_string(&s).ok()) else {
        return base;
    };
    let now = Utc::now();
    let remaining = (next - now).num_milliseconds() as f64 / 1000.0;
    if remaining <= 0.0 {
        0.1
    } else {
        remaining.clamp(0.1, base)
    }
}

// ---------------------------------------------------------------------------
// Cron expression
// ---------------------------------------------------------------------------

struct CronExpression {
    minute: BTreeSet<u32>,
    hour: BTreeSet<u32>,
    day_of_month_any: bool,
    day_of_month: BTreeSet<u32>,
    month: BTreeSet<u32>,
    day_of_week_any: bool,
    day_of_week: BTreeSet<u32>,
}

impl CronExpression {
    fn parse(expr: &str) -> Result<Self> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(Error::InvalidCronExpression(
                "cron expression must have 5 fields".into(),
            ));
        }
        Ok(Self {
            minute: parse_field(parts[0], 0, 60)?,
            hour: parse_field(parts[1], 0, 23)?,
            day_of_month_any: parts[2] == "*",
            day_of_month: parse_field(parts[2], 1, 31)?,
            month: parse_field(parts[3], 1, 12)?,
            day_of_week_any: parts[4] == "*",
            day_of_week: parse_field(parts[4], 0, 7)?,
        })
    }

    fn matches(&self, candidate: &DateTime<Utc>) -> bool {
        let weekday = (candidate.format("%u").to_string().parse::<u32>().unwrap_or(0)) % 7;
        let dom = candidate.format("%d").to_string().parse::<u32>().unwrap_or(0);
        let dow_in_set = self.day_of_week.contains(&weekday);
        let dom_in_set = self.day_of_month.contains(&dom);
        let day_match = if self.day_of_month_any && self.day_of_week_any {
            true
        } else if self.day_of_month_any {
            dow_in_set
        } else if self.day_of_week_any {
            dom_in_set
        } else {
            dom_in_set || dow_in_set
        };
        let minute = candidate.format("%M").to_string().parse::<u32>().unwrap_or(0);
        let hour = candidate.format("%H").to_string().parse::<u32>().unwrap_or(0);
        let month = candidate.format("%m").to_string().parse::<u32>().unwrap_or(0);
        self.minute.contains(&minute)
            && self.hour.contains(&hour)
            && self.month.contains(&month)
            && day_match
    }
}

fn parse_cron_expression(expr: &str) -> Result<CronExpression> {
    CronExpression::parse(expr)
}

fn parse_field(raw: &str, minimum: u32, maximum: u32) -> Result<BTreeSet<u32>> {
    let mut out = BTreeSet::new();
    for chunk in raw.split(',') {
        out.extend(parse_chunk(chunk.trim(), minimum, maximum)?);
    }
    if out.is_empty() {
        return Err(Error::InvalidCronExpression(format!(
            "invalid cron field: {raw}"
        )));
    }
    Ok(out)
}

fn parse_chunk(chunk: &str, minimum: u32, maximum: u32) -> Result<BTreeSet<u32>> {
    if chunk == "*" {
        return Ok((minimum..=maximum).collect());
    }
    let (base, step) = match chunk.split_once('/') {
        Some((b, s)) => (
            b,
            s.parse::<u32>().map_err(|_| {
                Error::InvalidCronExpression(format!("invalid cron step: {s}"))
            })?,
        ),
        None => (chunk, 1),
    };
    if step == 0 {
        return Err(Error::InvalidCronExpression(
            "cron step must be positive".into(),
        ));
    }
    let (start, end) = if base == "*" {
        (minimum, maximum)
    } else if let Some((a, b)) = base.split_once('-') {
        let start = a.parse::<u32>().map_err(|_| {
            Error::InvalidCronExpression(format!("invalid cron range start: {a}"))
        })?;
        let end = b.parse::<u32>().map_err(|_| {
            Error::InvalidCronExpression(format!("invalid cron range end: {b}"))
        })?;
        (start, end)
    } else {
        let v = base.parse::<u32>().map_err(|_| {
            Error::InvalidCronExpression(format!("invalid cron value: {base}"))
        })?;
        if v < minimum || v > maximum {
            return Err(Error::InvalidCronExpression(format!(
                "value {v} outside range {minimum}-{maximum}"
            )));
        }
        return Ok([v].into_iter().collect());
    };
    if start > end {
        return Err(Error::InvalidCronExpression(format!(
            "invalid cron range: {chunk}"
        )));
    }
    let mut out = BTreeSet::new();
    let mut v = start;
    while v <= end {
        if v < minimum || v > maximum {
            return Err(Error::InvalidCronExpression(format!(
                "value {v} outside range {minimum}-{maximum}"
            )));
        }
        out.insert(v);
        v = match v.checked_add(step) {
            Some(next) if next <= end => next,
            _ => break,
        };
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let unique = format!("echobot-cron-test-{}-{}", std::process::id(), name);
        let dir = std::env::temp_dir().join(unique);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_every_schedule() {
        let s = CronSchedule {
            kind: CronScheduleKind::Every,
            every_seconds: Some(60),
            ..CronSchedule::default()
        };
        assert!(normalize_schedule(&s).is_ok());
    }

    #[test]
    fn parse_cron_expression_5_fields_required() {
        let s = CronSchedule {
            kind: CronScheduleKind::Cron,
            expr: Some("0 0 * * *".into()),
            ..CronSchedule::default()
        };
        assert!(normalize_schedule(&s).is_ok());
        let bad = CronSchedule {
            kind: CronScheduleKind::Cron,
            expr: Some("0 0 * *".into()),
            ..CronSchedule::default()
        };
        assert!(normalize_schedule(&bad).is_err());
    }

    #[tokio::test]
    async fn add_and_remove_job_persists() {
        let dir = tmp_dir("add-remove");
        let store_path = dir.join("jobs.json");
        let service = CronService::new(&store_path, None, None);
        let schedule = CronSchedule {
            kind: CronScheduleKind::Every,
            every_seconds: Some(30),
            ..CronSchedule::default()
        };
        let payload = CronPayload::default();
        let job = service
            .add_job("reminder", schedule, payload, false)
            .await
            .unwrap();
        let loaded = service.get_job(&job.id).await.unwrap();
        assert_eq!(loaded.name, "reminder");
        let removed = service.remove_job(&job.id).await.unwrap();
        assert!(removed);
        assert!(service.get_job(&job.id).await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cron_matches_simple_minute() {
        let expr = parse_cron_expression("5 * * * *").unwrap();
        let mut dt = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        dt = dt.with_second(0).unwrap().with_nanosecond(0).unwrap();
        assert!(!expr.matches(&dt));
        dt = dt.with_minute(5).unwrap();
        assert!(expr.matches(&dt));
    }

    #[test]
    fn cron_handles_comma_and_step() {
        let expr = parse_cron_expression("0,15,30,45 * * * *").unwrap();
        let dt = Utc.with_ymd_and_hms(2026, 1, 1, 0, 15, 0).unwrap();
        assert!(expr.matches(&dt));
        let expr = parse_cron_expression("*/15 * * * *").unwrap();
        let dt = Utc.with_ymd_and_hms(2026, 1, 1, 0, 30, 0).unwrap();
        assert!(expr.matches(&dt));
    }

    #[test]
    fn store_round_trip() {
        let dir = tmp_dir("store-round-trip");
        let store_path = dir.join("jobs.json");
        let mut store = CronStore::default();
        store.jobs.push(CronJob {
            id: "abc12345".into(),
            name: "test".into(),
            enabled: true,
            schedule: CronSchedule {
                kind: CronScheduleKind::Every,
                every_seconds: Some(60),
                ..CronSchedule::default()
            },
            payload: CronPayload::default(),
            state: CronJobState {
                next_run_at: Some("2026-01-01T00:00:00Z".into()),
                last_status: Some("ok".into()),
                ..CronJobState::default()
            },
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            delete_after_run: false,
        });
        save_store(&store_path, &store).unwrap();
        let loaded = load_store(&store_path).unwrap();
        assert_eq!(loaded.jobs.len(), 1);
        assert_eq!(loaded.jobs[0].id, "abc12345");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
