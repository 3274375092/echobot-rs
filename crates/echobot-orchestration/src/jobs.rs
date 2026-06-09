//! Background-job store, completion callback, and the
//! `OrchestratedTurnResult` returned to callers.
//!
//! Mirrors `echobot/orchestration/jobs.py`. In v1 the store is purely
//! in-memory (no on-disk persistence); the constructor accepts a
//! `path: Option<...>` to keep the API surface forward-compatible.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use echobot_core::models::{message_content_to_text, MessageContent, MessageRole};
use echobot_runtime::sessions::ChatSession;

/// Text the coordinator appends when a job is cancelled.
pub const JOB_CANCELLED_TEXT: &str = "后台任务已停止。";
/// Text used when a previously-running job is recovered after a restart.
pub const JOB_INTERRUPTED_TEXT: &str = "任务因 EchoBot 重启而中断。";
/// Statuses for which a job can be retried.
pub const RETRYABLE_JOB_STATUSES: &[&str] = &["failed", "cancelled"];
/// Statuses that count as "still running" for shutdown / recovery.
pub const ACTIVE_JOB_STATUSES: &[&str] = &["running"];

/// A unique job identifier.
pub type JobId = String;

/// The result of a single orchestrated turn.
#[derive(Debug, Clone)]
pub struct OrchestratedTurnResult {
    /// The session after the turn.
    pub session: ChatSession,
    /// The visible text reply (empty for a delegated turn).
    pub response_text: String,
    /// True if the work was delegated to a background agent job.
    pub delegated: bool,
    /// True if the work completed synchronously (chat path).
    pub completed: bool,
    /// The rich content of the reply (defaults to `response_text`).
    pub response_content: MessageContent,
    /// The background-job id (only set for delegated turns).
    pub job_id: Option<JobId>,
    /// The current job status (`"running"`, `"completed"`, ...).
    pub status: String,
    /// The role name used for the turn.
    pub role_name: String,
    /// Number of agent steps taken (0 for delegated / chat turns).
    pub steps: usize,
    /// Compressed-history summary from the session, if any.
    pub compressed_summary: String,
}

/// A background job record.
#[derive(Debug, Clone)]
pub struct ConversationJob {
    /// Unique id (uuid4 hex).
    pub job_id: JobId,
    /// Owning session name.
    pub session_name: String,
    /// Original user prompt.
    pub prompt: String,
    /// Synchronous "I'm on it" reply given before the agent started.
    pub immediate_response: String,
    /// Role name used for the turn.
    pub role_name: String,
    /// `"running"`, `"completed"`, `"failed"`, `"cancelled"`, or
    /// `"waiting_for_input"`.
    pub status: String,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
    /// ISO 8601 updated timestamp.
    pub updated_at: String,
    /// ISO 8601 start timestamp.
    pub started_at: String,
    /// ISO 8601 finish timestamp (empty while running).
    pub finished_at: String,
    /// Optional trace run id.
    pub trace_run_id: Option<String>,
    /// The route mode under which the job was started.
    pub route_mode: String,
    /// 1-based attempt number.
    pub attempt: u32,
    /// If this is a retry, the id of the job it retries.
    pub retry_of_job_id: Option<JobId>,
    /// Image URL inputs captured when the job was created.
    pub image_urls: Vec<HashMap<String, String>>,
    /// File attachment inputs captured when the job was created.
    pub file_attachments: Vec<HashMap<String, Value>>,
    /// Final visible response (set when the job completes / fails /
    /// is cancelled / waits for input).
    pub final_response: String,
    /// Rich final content.
    pub final_response_content: MessageContent,
    /// Error text (set on failure).
    pub error: String,
    /// Number of agent steps taken.
    pub steps: usize,
    /// Pending user-input payload, if the agent paused for input.
    pub pending_user_input: Option<HashMap<String, Value>>,
}

impl ConversationJob {
    /// Returns a deep-copy of `self` suitable for handing out to a
    /// completion callback.
    pub fn snapshot(&self) -> ConversationJob {
        self.clone()
    }
}

/// A boxed async callback that receives a finished job.
pub type CompletionCallback =
    Arc<dyn Fn(ConversationJob) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// True if `job` is in a state that can be retried.
pub fn job_can_retry(job: &ConversationJob) -> bool {
    RETRYABLE_JOB_STATUSES.contains(&job.status.as_str())
}

/// In-memory store for [`ConversationJob`]s.
#[derive(Debug)]
pub struct ConversationJobStore {
    path: Option<std::path::PathBuf>,
    jobs: Mutex<HashMap<JobId, ConversationJob>>,
}

impl ConversationJobStore {
    /// Creates a new in-memory store. If `path` is supplied, the store
    /// will load / persist jobs at that path.
    pub fn new(path: Option<impl Into<std::path::PathBuf>>) -> Self {
        Self {
            path: path.map(Into::into),
            jobs: Mutex::new(HashMap::new()),
        }
    }

    /// Creates a new job.
    ///
    /// The argument list mirrors the Python
    /// `ConversationJobStore.create` helper 1:1; the `too_many_arguments`
    /// lint is suppressed intentionally.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        session_name: impl Into<String>,
        prompt: impl Into<String>,
        immediate_response: impl Into<String>,
        role_name: impl Into<String>,
        route_mode: &str,
        image_urls: Vec<HashMap<String, String>>,
        file_attachments: Vec<HashMap<String, Value>>,
        trace_run_id: Option<String>,
        attempt: u32,
        retry_of_job_id: Option<JobId>,
    ) -> ConversationJob {
        let now = now_text();
        let job = ConversationJob {
            job_id: uuid::Uuid::new_v4().simple().to_string(),
            session_name: session_name.into(),
            prompt: prompt.into(),
            immediate_response: immediate_response.into(),
            role_name: role_name.into(),
            status: "running".to_string(),
            created_at: now.clone(),
            updated_at: now.clone(),
            started_at: now,
            finished_at: String::new(),
            trace_run_id,
            route_mode: route_mode.to_string(),
            attempt: attempt.max(1),
            retry_of_job_id,
            image_urls: copy_string_mapping_list(&image_urls),
            file_attachments: copy_object_mapping_list(&file_attachments),
            final_response: String::new(),
            final_response_content: MessageContent::Text(String::new()),
            error: String::new(),
            steps: 0,
            pending_user_input: None,
        };
        let snapshot = job.snapshot();
        {
            let mut guard = self.jobs.lock().await;
            guard.insert(snapshot.job_id.clone(), snapshot.clone());
        }
        self.persist_locked().await;
        snapshot
    }

    /// Returns a snapshot of the job with id `job_id`, or `None`.
    pub async fn get(&self, job_id: &str) -> Option<ConversationJob> {
        let guard = self.jobs.lock().await;
        guard.get(job_id).map(|j| j.snapshot())
    }

    /// Marks the job as `completed` with the supplied final response.
    pub async fn set_completed(
        &self,
        job_id: &str,
        final_response: &str,
        final_response_content: MessageContent,
        steps: usize,
    ) -> Option<ConversationJob> {
        self.transition(job_id, |job| {
            job.status = "completed".to_string();
            job.final_response = final_response.to_string();
            job.final_response_content = normalize_message_content(&final_response_content);
            job.steps = steps;
            job.error = String::new();
            job.pending_user_input = None;
        })
        .await
    }

    /// Marks the job as `failed`.
    pub async fn set_failed(
        &self,
        job_id: &str,
        final_response: &str,
        final_response_content: MessageContent,
        error: &str,
        steps: usize,
    ) -> Option<ConversationJob> {
        self.transition(job_id, |job| {
            job.status = "failed".to_string();
            job.final_response = final_response.to_string();
            job.final_response_content = normalize_message_content(&final_response_content);
            job.error = error.to_string();
            job.steps = steps;
            job.pending_user_input = None;
        })
        .await
    }

    /// Marks the job as `cancelled`.
    pub async fn set_cancelled(
        &self,
        job_id: &str,
        final_response: &str,
        final_response_content: MessageContent,
        steps: usize,
    ) -> Option<ConversationJob> {
        self.transition(job_id, |job| {
            job.status = "cancelled".to_string();
            job.final_response = final_response.to_string();
            job.final_response_content = normalize_message_content(&final_response_content);
            job.error = String::new();
            job.steps = steps;
            job.pending_user_input = None;
        })
        .await
    }

    /// Marks the job as `waiting_for_input`.
    pub async fn set_waiting_for_input(
        &self,
        job_id: &str,
        final_response: &str,
        final_response_content: MessageContent,
        steps: usize,
        pending_user_input: Option<HashMap<String, Value>>,
    ) -> Option<ConversationJob> {
        self.transition(job_id, |job| {
            job.status = "waiting_for_input".to_string();
            job.final_response = final_response.to_string();
            job.final_response_content = normalize_message_content(&final_response_content);
            job.error = String::new();
            job.steps = steps;
            job.pending_user_input = pending_user_input
                .as_ref()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
        })
        .await
    }

    /// Returns counts of jobs by status.
    pub async fn counts(&self) -> HashMap<String, usize> {
        let guard = self.jobs.lock().await;
        let mut out: HashMap<String, usize> = HashMap::new();
        for job in guard.values() {
            *out.entry(job.status.clone()).or_insert(0) += 1;
        }
        out
    }

    /// Returns the jobs for a given session, optionally filtered by
    /// status, sorted by `created_at` ascending.
    pub async fn list_for_session(
        &self,
        session_name: &str,
        status: Option<&str>,
    ) -> Vec<ConversationJob> {
        let guard = self.jobs.lock().await;
        let mut out: Vec<ConversationJob> = guard
            .values()
            .filter(|j| j.session_name == session_name)
            .filter(|j| status.map(|s| s == j.status).unwrap_or(true))
            .map(|j| j.snapshot())
            .collect();
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        out
    }

    /// Returns jobs, optionally filtered by `session_name` and `status`,
    /// sorted by `(updated_at, created_at, job_id)` descending, capped at
    /// `limit`.
    pub async fn list_jobs(
        &self,
        session_name: Option<&str>,
        status: Option<&str>,
        limit: usize,
    ) -> Vec<ConversationJob> {
        let limit = limit.max(1);
        let guard = self.jobs.lock().await;
        let mut out: Vec<ConversationJob> = guard
            .values()
            .filter(|j| session_name.map(|s| s == j.session_name).unwrap_or(true))
            .filter(|j| status.map(|s| s == j.status).unwrap_or(true))
            .map(|j| j.snapshot())
            .collect();
        out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then(b.created_at.cmp(&a.created_at))
                .then(b.job_id.cmp(&a.job_id))
        });
        out.truncate(limit);
        out
    }

    async fn transition<F>(&self, job_id: &str, mutate: F) -> Option<ConversationJob>
    where
        F: FnOnce(&mut ConversationJob),
    {
        let snapshot = {
            let mut guard = self.jobs.lock().await;
            let job = guard.get_mut(job_id)?;
            let now = now_text();
            job.updated_at = now.clone();
            job.finished_at = now;
            mutate(job);
            job.snapshot()
        };
        drop(snapshot);
        self.persist_locked().await;
        let guard = self.jobs.lock().await;
        guard.get(job_id).map(|j| j.snapshot())
    }

    async fn persist_locked(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let guard = self.jobs.lock().await;
        let mut ordered: Vec<&ConversationJob> = guard.values().collect();
        ordered.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.job_id.cmp(&b.job_id)));
        let jobs_value = Value::Array(
            ordered
                .iter()
                .map(|j| job_to_value(j))
                .collect::<Vec<_>>(),
        );
        let payload = serde_json::json!({ "jobs": jobs_value });
        let path = path.clone();
        let _ = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let text = serde_json::to_string_pretty(&payload)
                .unwrap_or_else(|_| "{}".to_string());
            std::fs::write(&path, format!("{text}\n"))?;
            Ok(())
        })
        .await;
    }
}

/// Helper: build a one-shot completion callback that just logs the job.
pub fn logging_completion_callback() -> CompletionCallback {
    Arc::new(|job: ConversationJob| {
        Box::pin(async move {
            tracing::info!(
                job_id = %job.job_id,
                session = %job.session_name,
                status = %job.status,
                steps = job.steps,
                "background job finished"
            );
        })
    })
}

fn now_text() -> String {
    let now: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
    now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn copy_string_mapping_list(values: &[HashMap<String, String>]) -> Vec<HashMap<String, String>> {
    values
        .iter()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .collect()
}

fn copy_object_mapping_list(values: &[HashMap<String, Value>]) -> Vec<HashMap<String, Value>> {
    values
        .iter()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .collect()
}

fn normalize_message_content(content: &MessageContent) -> MessageContent {
    match content {
        MessageContent::Text(s) => MessageContent::Text(s.trim().to_string()),
        MessageContent::Blocks(blocks) => {
            let mut out: Vec<echobot_core::models::MessageContentBlock> = Vec::new();
            for block in blocks {
                if let Some(parsed) =
                    echobot_core::models::MessageContentBlock::from_value(block.to_value())
                {
                    out.push(parsed);
                }
            }
            if out.is_empty() {
                MessageContent::Text(String::new())
            } else {
                MessageContent::Blocks(out)
            }
        }
    }
}

fn job_to_value(job: &ConversationJob) -> Value {
    serde_json::json!({
        "job_id": job.job_id,
        "session_name": job.session_name,
        "prompt": job.prompt,
        "immediate_response": job.immediate_response,
        "role_name": job.role_name,
        "status": job.status,
        "created_at": job.created_at,
        "updated_at": job.updated_at,
        "started_at": job.started_at,
        "finished_at": job.finished_at,
        "trace_run_id": job.trace_run_id,
        "route_mode": job.route_mode,
        "attempt": job.attempt,
        "retry_of_job_id": job.retry_of_job_id,
        "image_urls": job.image_urls,
        "file_attachments": job.file_attachments,
        "final_response": job.final_response,
        "final_response_content": job.final_response_content,
        "error": job.error,
        "steps": job.steps,
        "pending_user_input": job.pending_user_input,
    })
}

/// Ensures `output` exists under `parent` as a directory. Returns the
/// `parent/output` path. Used by the bootstrap / CLI when wiring an
/// optional on-disk job store.
pub fn ensure_job_store_path(parent: &Path, output: &str) -> std::path::PathBuf {
    let path = parent.join(output);
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    path
}

// silence unused-import warnings for traits used elsewhere
#[allow(dead_code)]
fn _silence_unused_message_content() {
    let _ = message_content_to_text(&MessageContent::Text(String::new()));
    let _ = MessageRole::System;
}

#[cfg(test)]
mod tests {
    use super::*;
    use echobot_core::models::MessageRole;

    #[tokio::test]
    async fn create_and_complete_job() {
        let store = ConversationJobStore::new(None::<String>);
        let job = store
            .create(
                "session-1",
                "hello",
                "on it",
                "default",
                "auto",
                vec![],
                vec![],
                None,
                1,
                None,
            )
            .await;
        assert_eq!(job.status, "running");
        assert!(store.set_completed(&job.job_id, "done", MessageContent::Text("done".into()), 3).await.is_some());
        let snapshot = store.get(&job.job_id).await.unwrap();
        assert_eq!(snapshot.status, "completed");
        assert_eq!(snapshot.steps, 3);
        assert!(!job_can_retry(&job));
    }

    #[tokio::test]
    async fn retryable_status_check() {
        let mut job = ConversationJob {
            job_id: "j".into(),
            session_name: "s".into(),
            prompt: "p".into(),
            immediate_response: "".into(),
            role_name: "default".into(),
            status: "failed".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: "2024-01-01T00:00:00Z".into(),
            started_at: "2024-01-01T00:00:00Z".into(),
            finished_at: "2024-01-01T00:00:00Z".into(),
            trace_run_id: None,
            route_mode: "auto".into(),
            attempt: 1,
            retry_of_job_id: None,
            image_urls: vec![],
            file_attachments: vec![],
            final_response: "".into(),
            final_response_content: MessageContent::Text(String::new()),
            error: "boom".into(),
            steps: 0,
            pending_user_input: None,
        };
        assert!(job_can_retry(&job));
        job.status = "running".into();
        assert!(!job_can_retry(&job));
        let _ = MessageRole::System;
    }

    #[tokio::test]
    async fn list_jobs_respects_limit_and_filters() {
        let store = ConversationJobStore::new(None::<String>);
        for i in 0..3 {
            store
                .create(
                    "session",
                    format!("p{i}"),
                    "",
                    "default",
                    "auto",
                    vec![],
                    vec![],
                    None,
                    1,
                    None,
                )
                .await;
        }
        let all = store.list_jobs(None, None, 10).await;
        assert_eq!(all.len(), 3);
        let two = store.list_jobs(None, None, 2).await;
        assert_eq!(two.len(), 2);
        let none = store.list_jobs(Some("other"), None, 10).await;
        assert!(none.is_empty());
    }
}
