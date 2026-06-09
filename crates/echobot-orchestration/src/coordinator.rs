//! `ConversationCoordinator`: the orchestrator that ties the decision
//! layer, roleplay engine, and full agent runner together.
//!
//! Mirrors `echobot/orchestration/coordinator.py`. The coordinator owns
//! the per-session lock map, the background job store, and the streamed
//! end-to-end turn flow:
//!
//! 1. Acquire the per-session lock.
//! 2. Resolve the role card and route mode for the turn.
//! 3. Ask the [`DecisionEngine`] how to route.
//! 4. For chat: stream a roleplay reply, save, return.
//! 5. For agent: emit a delegated acknowledgement, append the user
//!    prompt to the session, create a background job, and return
//!    immediately. The job runs the agent via the
//!    [`SessionAgentRunner`], then calls the optional
//!    [`CompletionCallback`].

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dashmap::DashMap;
use futures::stream::Stream;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::task::AbortHandle;
use tracing::warn;

use echobot_core::models::{
    build_message_content, is_message_content_empty, message_content_blocks,
    message_content_to_text, LLMMessage, MessageContent, MessageContentBlock, MessageRole,
    TEXT_CONTENT_BLOCK_TYPE,
};
use echobot_runtime::agent_traces::AgentTraceStore;
use echobot_runtime::scheduled_tasks::CoordinatorLike;
use echobot_runtime::session_runner::SessionAgentRunner;
use echobot_runtime::sessions::{ChatSession, SessionStore};
use echobot_runtime::turns::TraceCallback;

use crate::decision::DecisionEngine;
use crate::jobs::{
    job_can_retry, CompletionCallback, ConversationJob, ConversationJobStore,
    OrchestratedTurnResult, JOB_CANCELLED_TEXT,
};
use crate::roleplay::{RoleplayEngine, ScheduledCronJobInfo, StreamCallback};
use crate::route_modes::{route_mode_from_metadata, set_route_mode, RouteMode};
use crate::roles::{role_name_from_metadata, set_role_name, RoleCard, RoleCardRegistry};

/// Cap on the number of recent messages the agent sees as handoff context.
pub const AGENT_HANDOFF_MAX_MESSAGES: usize = 6;
/// Cap on the total characters in the agent handoff context.
pub const AGENT_HANDOFF_MAX_TOTAL_CHARS: usize = 6000;
/// Cap on the characters per message in the agent handoff context.
pub const AGENT_HANDOFF_MAX_MESSAGE_CHARS: usize = 1800;
/// Session-metadata key used to record a pending user-input request.
pub const PENDING_USER_INPUT_METADATA_KEY: &str = "pending_user_input";

/// Background job factory: a closure that produces a future running the
/// job. The returned future must be `'static`.
pub type BackgroundJobFactory = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
>;

/// The orchestrator.
pub struct ConversationCoordinator {
    session_store: SessionStore,
    agent_runner: Arc<SessionAgentRunner>,
    decision_engine: Arc<DecisionEngine>,
    roleplay_engine: Arc<RoleplayEngine>,
    role_registry: Arc<RoleCardRegistry>,
    delegated_ack_enabled: tokio::sync::RwLock<bool>,
    jobs: Arc<ConversationJobStore>,
    session_locks: DashMap<String, Arc<Mutex<()>>>,
    background_tasks: Mutex<HashMap<String, AbortHandle>>,
    deleted_sessions: tokio::sync::RwLock<std::collections::HashSet<String>>,
}

impl std::fmt::Debug for ConversationCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationCoordinator").finish()
    }
}

impl ConversationCoordinator {
    /// Creates a new coordinator.
    pub fn new(
        session_store: SessionStore,
        agent_runner: Arc<SessionAgentRunner>,
        decision_engine: Arc<DecisionEngine>,
        roleplay_engine: Arc<RoleplayEngine>,
        role_registry: Arc<RoleCardRegistry>,
        delegated_ack_enabled: bool,
        jobs: Option<Arc<ConversationJobStore>>,
    ) -> Self {
        Self {
            session_store,
            agent_runner,
            decision_engine,
            roleplay_engine,
            role_registry,
            delegated_ack_enabled: tokio::sync::RwLock::new(delegated_ack_enabled),
            jobs: jobs.unwrap_or_else(|| {
                Arc::new(ConversationJobStore::new(None::<std::path::PathBuf>))
            }),
            session_locks: DashMap::new(),
            background_tasks: Mutex::new(HashMap::new()),
            deleted_sessions: tokio::sync::RwLock::new(std::collections::HashSet::new()),
        }
    }

    /// Returns the current `delegated_ack_enabled` flag.
    pub async fn delegated_ack_enabled(&self) -> bool {
        *self.delegated_ack_enabled.read().await
    }

    /// Sets the `delegated_ack_enabled` flag.
    pub async fn set_delegated_ack_enabled(&self, enabled: bool) {
        *self.delegated_ack_enabled.write().await = enabled;
    }

    /// Handles a single user turn (non-streaming wrapper around
    /// [`handle_user_turn_stream`]).
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_user_turn(
        self: &Arc<Self>,
        session_name: &str,
        prompt: &str,
        image_urls: Option<&[Value]>,
        file_attachments: Option<&[Value]>,
        role_name: Option<&str>,
        route_mode: Option<RouteMode>,
        completion_callback: Option<CompletionCallback>,
        retry_of_job_id: Option<String>,
        attempt: u32,
    ) -> Result<OrchestratedTurnResult, anyhow::Error> {
        self.handle_user_turn_stream(
            session_name,
            prompt,
            image_urls,
            file_attachments,
            role_name,
            route_mode,
            completion_callback,
            None,
            retry_of_job_id,
            attempt,
        )
        .await
    }

    /// Handles a single user turn, optionally streaming the chat reply
    /// (or the delegated acknowledgement) to `on_chunk`.
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_user_turn_stream(
        self: &Arc<Self>,
        session_name: &str,
        prompt: &str,
        image_urls: Option<&[Value]>,
        file_attachments: Option<&[Value]>,
        role_name: Option<&str>,
        route_mode: Option<RouteMode>,
        completion_callback: Option<CompletionCallback>,
        on_chunk: Option<StreamCallback>,
        retry_of_job_id: Option<String>,
        attempt: u32,
    ) -> Result<OrchestratedTurnResult, anyhow::Error> {
        self.restore_session(session_name).await;
        let chunk_handler: StreamCallback = on_chunk.unwrap_or_else(discard_stream_chunk);

        let lock = self.session_lock(session_name).await;
        let _guard = lock.lock().await;

        let mut session = self.load_session_internal(session_name).await?;
        let role_card = self.resolve_turn_role(&mut session, role_name).await;
        let resolved_route_mode = self.resolve_turn_route_mode(&session, route_mode);
        let pending_user_input = pending_user_input_from_metadata(Some(&session.metadata));

        let decision = if pending_user_input.is_some() {
            forced_agent_decision_for_pending_input()
        } else {
            let recent_history = &session.history[session.history.len().saturating_sub(8)..];
            self.decision_engine
                .decide(prompt, Some(recent_history), resolved_route_mode)
                .await
        };

        if !decision.requires_agent() {
            let response_text = self
                .roleplay_engine
                .stream_chat_reply(
                    &session,
                    prompt,
                    image_urls,
                    file_attachments,
                    &role_card,
                    chunk_handler.clone(),
                )
                .await
                .unwrap_or_else(|e| {
                    warn!(error = %e, "chat reply failed; returning empty");
                    String::new()
                });
            session.history.push(LLMMessage {
                role: MessageRole::User,
                content: build_message_content(
                    prompt,
                    image_urls.map(|v| v.to_vec()).as_deref(),
                    file_attachments.map(|v| v.to_vec()).as_deref(),
                ),
                name: None,
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning_content: String::new(),
                reasoning_field: echobot_core::models::ReasoningField::default(),
            });
            session.history.push(LLMMessage {
                role: MessageRole::Assistant,
                content: MessageContent::Text(response_text.clone()),
                name: None,
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning_content: String::new(),
                reasoning_field: echobot_core::models::ReasoningField::default(),
            });
            self.save_session_internal(&session).await?;
            return Ok(OrchestratedTurnResult {
                session: session.clone(),
                response_text: response_text.clone(),
                delegated: false,
                completed: true,
                response_content: MessageContent::Text(response_text),
                job_id: None,
                status: "completed".to_string(),
                role_name: role_card.name.clone(),
                steps: 1,
                compressed_summary: session.compressed_summary.clone(),
            });
        }

        // Agent route.
        let mut immediate_response = String::new();
        let ack_enabled = *self.delegated_ack_enabled.read().await;
        if ack_enabled && pending_user_input.is_none() {
            immediate_response = self
                .roleplay_engine
                .delegated_ack(&session, prompt, image_urls, file_attachments, &role_card)
                .await
                .unwrap_or_else(|e| {
                    warn!(error = %e, "delegated ack failed; continuing with empty ack");
                    String::new()
                });
        }
        let handoff_text = build_agent_handoff_text(&session);
        let continuation_text =
            build_pending_user_input_continuation_text(pending_user_input.as_ref());

        session.history.push(LLMMessage {
            role: MessageRole::User,
            content: build_message_content(
                prompt,
                image_urls.map(|v| v.to_vec()).as_deref(),
                file_attachments.map(|v| v.to_vec()).as_deref(),
            ),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: String::new(),
            reasoning_field: echobot_core::models::ReasoningField::default(),
        });
        if !immediate_response.trim().is_empty() {
            session.history.push(LLMMessage {
                role: MessageRole::Assistant,
                content: MessageContent::Text(immediate_response.clone()),
                name: None,
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning_content: String::new(),
                reasoning_field: echobot_core::models::ReasoningField::default(),
            });
        }
        if pending_user_input.is_some() {
            session.metadata = clear_pending_user_input(&session.metadata);
        }
        self.save_session_internal(&session).await?;

        let trace_run_id = self.agent_runner.create_trace_run_id();
        let image_url_maps: Vec<HashMap<String, String>> = image_urls
            .map(|v| {
                v.iter()
                    .map(|val| match val {
                        Value::String(s) => {
                            let mut m = HashMap::new();
                            m.insert("url".to_string(), s.clone());
                            m
                        }
                        Value::Object(obj) => obj
                            .iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect(),
                        _ => HashMap::new(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let file_attachment_maps: Vec<HashMap<String, Value>> = file_attachments
            .map(|v| {
                v.iter()
                    .filter_map(|val| val.as_object().map(|m| {
                        m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                    }))
                    .collect()
            })
            .unwrap_or_default();

        let job = self
            .jobs
            .create(
                session.name.clone(),
                prompt.to_string(),
                immediate_response.clone(),
                role_card.name.clone(),
                resolved_route_mode.as_str(),
                image_url_maps,
                file_attachment_maps,
                trace_run_id.clone(),
                attempt.max(1),
                retry_of_job_id,
            )
            .await;

        let job_id = job.job_id.clone();
        let handoff_text_for_job = handoff_text.clone();
        let continuation_text_for_job = continuation_text.clone();
        let me_for_factory = self.clone();
        let job_id_for_task = job_id.clone();
        let completion_callback_for_task = completion_callback;
        let prompt_for_task = prompt.to_string();
        let image_urls_for_task: Option<Vec<Value>> = image_urls.map(|v| v.to_vec());
        let file_attachments_for_task: Option<Vec<Value>> =
            file_attachments.map(|v| v.to_vec());
        let trace_run_id_for_task = trace_run_id;
        let session_name_for_task = session.name.clone();

        let factory: BackgroundJobFactory = Arc::new(move || {
            let me = me_for_factory.clone();
            let job_id = job_id_for_task.clone();
            let session_name = session_name_for_task.clone();
            let prompt = prompt_for_task.clone();
            let image_urls = image_urls_for_task.clone();
            let file_attachments = file_attachments_for_task.clone();
            let handoff_text = handoff_text_for_job.clone();
            let continuation_text = continuation_text_for_job.clone();
            let trace_run_id = trace_run_id_for_task.clone();
            let completion_callback = completion_callback_for_task.clone();
            Box::pin(async move {
                me.run_agent_job(
                    job_id,
                    session_name,
                    prompt,
                    image_urls,
                    file_attachments,
                    handoff_text,
                    continuation_text,
                    trace_run_id,
                    completion_callback,
                )
                .await;
            })
        });
        self.start_background_job(job_id.clone(), factory);

        if !immediate_response.trim().is_empty() {
            (chunk_handler)(immediate_response.clone()).await;
        }
        Ok(OrchestratedTurnResult {
            session: session.clone(),
            response_text: immediate_response.clone(),
            delegated: true,
            completed: false,
            response_content: MessageContent::Text(immediate_response),
            job_id: Some(job_id),
            status: job.status.clone(),
            role_name: role_card.name.clone(),
            steps: 0,
            compressed_summary: session.compressed_summary.clone(),
        })
    }

    /// Loads the session.
    pub async fn load_session(&self, session_name: &str) -> Result<ChatSession, anyhow::Error> {
        let lock = self.session_lock(session_name).await;
        let _guard = lock.lock().await;
        let session = self.load_session_internal(session_name).await?;
        Ok(session)
    }

    /// Sets the role for `session_name`.
    pub async fn set_session_role(
        &self,
        session_name: &str,
        role_name: &str,
    ) -> Result<ChatSession, anyhow::Error> {
        let role_card = self
            .role_registry
            .try_require(Some(role_name))
            .await
            .map_err(anyhow::Error::msg)?;
        let lock = self.session_lock(session_name).await;
        let _guard = lock.lock().await;
        let mut session = self.load_session_internal(session_name).await?;
        session.metadata = set_role_name(&session.metadata, &role_card.name);
        self.save_session_internal(&session).await?;
        Ok(session)
    }

    /// Returns the active role name for the session.
    pub async fn current_role_name(&self, session_name: &str) -> Result<String, anyhow::Error> {
        let lock = self.session_lock(session_name).await;
        let _guard = lock.lock().await;
        let mut session = self.load_session_internal(session_name).await?;
        let mut role_name = role_name_from_metadata(Some(&session.metadata));
        if self.role_registry.get(Some(&role_name)).await.is_none() {
            if let Some(card) = self.role_registry.get(None).await {
                role_name = card.name.clone();
                session.metadata = set_role_name(&session.metadata, &card.name);
                self.save_session_internal(&session).await?;
            }
        }
        Ok(role_name)
    }

    /// Sets the route mode for `session_name`.
    pub async fn set_session_route_mode(
        &self,
        session_name: &str,
        route_mode: RouteMode,
    ) -> Result<ChatSession, anyhow::Error> {
        let lock = self.session_lock(session_name).await;
        let _guard = lock.lock().await;
        let mut session = self.load_session_internal(session_name).await?;
        session.metadata = set_route_mode(&session.metadata, route_mode);
        self.save_session_internal(&session).await?;
        Ok(session)
    }

    /// Returns the active route mode for the session.
    pub async fn current_route_mode(
        &self,
        session_name: &str,
    ) -> Result<RouteMode, anyhow::Error> {
        let lock = self.session_lock(session_name).await;
        let _guard = lock.lock().await;
        let mut session = self.load_session_internal(session_name).await?;
        let mode = route_mode_from_metadata(Some(&session.metadata));
        session.metadata = set_route_mode(&session.metadata, mode);
        self.save_session_internal(&session).await?;
        Ok(mode)
    }

    /// Lists registered role names.
    pub async fn available_roles(&self) -> Vec<String> {
        self.role_registry.names().await
    }

    /// Returns the job with id `job_id`, or `None`.
    pub async fn get_job(&self, job_id: &str) -> Option<ConversationJob> {
        self.jobs.get(job_id).await
    }

    /// Lists jobs, optionally filtered by session name + status.
    pub async fn list_jobs(
        &self,
        session_name: Option<&str>,
        status: Option<&str>,
        limit: usize,
    ) -> Vec<ConversationJob> {
        self.jobs.list_jobs(session_name, status, limit).await
    }

    /// Re-runs a failed or cancelled job.
    pub async fn retry_job(
        self: &Arc<Self>,
        job_id: &str,
        completion_callback: Option<CompletionCallback>,
    ) -> Result<OrchestratedTurnResult, anyhow::Error> {
        let job = self
            .jobs
            .get(job_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("任务不存在：{job_id}"))?;
        if !job_can_retry(&job) {
            return Err(anyhow::anyhow!("只有失败或已取消的任务可以重试"));
        }
        let route_mode = match job.route_mode.as_str() {
            "auto" => Some(RouteMode::Auto),
            "chat_only" => Some(RouteMode::ChatOnly),
            "force_agent" => Some(RouteMode::ForceAgent),
            _ => None,
        };
        let image_values: Vec<Value> = job
            .image_urls
            .iter()
            .map(|m| {
                let mut obj = serde_json::Map::new();
                for (k, v) in m {
                    obj.insert(k.clone(), Value::String(v.clone()));
                }
                Value::Object(obj)
            })
            .collect();
        let file_values: Vec<Value> = job
            .file_attachments
            .iter()
            .map(|m| {
                let mut obj = serde_json::Map::new();
                for (k, v) in m {
                    obj.insert(k.clone(), v.clone());
                }
                Value::Object(obj)
            })
            .collect();
        self.handle_user_turn(
            &job.session_name,
            &job.prompt,
            Some(&image_values),
            Some(&file_values),
            Some(job.role_name.as_str()),
            route_mode,
            completion_callback,
            Some(job.job_id.clone()),
            job.attempt + 1,
        )
        .await
    }

    /// Returns the trace events recorded for a job.
    pub async fn get_job_trace(
        &self,
        job_id: &str,
    ) -> Result<(Option<ConversationJob>, Vec<serde_json::Map<String, Value>>), anyhow::Error>
    {
        let job = self.jobs.get(job_id).await;
        let Some(job) = job else {
            return Ok((None, Vec::new()));
        };
        let Some(trace_run_id) = &job.trace_run_id else {
            return Ok((Some(job), Vec::new()));
        };
        let events = self
            .agent_runner
            .load_trace_events(&job.session_name, trace_run_id)
            .await
            .unwrap_or_default();
        Ok((Some(job), events))
    }

    /// Cancels a running job.
    pub async fn cancel_job(&self, job_id: &str) -> Option<ConversationJob> {
        let job = self.jobs.get(job_id).await?;
        if job.status != "running" {
            return Some(job);
        }
        let handle = {
            let tasks = self.background_tasks.lock().await;
            tasks.get(job_id).cloned()
        };
        if let Some(handle) = handle {
            handle.abort();
        }
        let current = self.jobs.get(job_id).await?;
        if current.status != "running" {
            return Some(current);
        }
        let final_text = self.append_cancelled_message(&current.session_name).await;
        self.jobs
            .set_cancelled(
                job_id,
                &final_text,
                MessageContent::Text(final_text.clone()),
                0,
            )
            .await
    }

    /// Cancels every running job for `session_name`.
    pub async fn cancel_jobs_for_session(
        &self,
        session_name: &str,
    ) -> Vec<ConversationJob> {
        let running = self
            .jobs
            .list_for_session(session_name, Some("running"))
            .await;
        if running.is_empty() {
            return Vec::new();
        }
        let mut handles: Vec<AbortHandle> = Vec::new();
        for job in &running {
            let handle = {
                let map = self.background_tasks.lock().await;
                map.get(&job.job_id).cloned()
            };
            if let Some(handle) = handle {
                handle.abort();
                handles.push(handle);
            }
        }
        let mut cancelled: Vec<ConversationJob> = Vec::new();
        for job in running {
            if let Some(current) = self.jobs.get(&job.job_id).await {
                if current.status != "running" {
                    cancelled.push(current);
                    continue;
                }
                if let Some(updated) = self
                    .jobs
                    .set_cancelled(
                        &job.job_id,
                        "",
                        MessageContent::Text(String::new()),
                        0,
                    )
                    .await
                {
                    cancelled.push(updated);
                }
            }
        }
        cancelled
    }

    /// Marks a session as deleted.
    pub async fn mark_session_deleted(&self, session_name: &str) {
        self.deleted_sessions
            .write()
            .await
            .insert(session_name.to_string());
        self.agent_runner.mark_session_deleted(session_name).await;
    }

    /// Unmarks a session as deleted.
    pub async fn restore_session(&self, session_name: &str) {
        self.deleted_sessions
            .write()
            .await
            .remove(session_name);
        self.agent_runner.restore_session(session_name).await;
    }

    /// Returns job counts grouped by status.
    pub async fn job_counts(&self) -> HashMap<String, usize> {
        self.jobs.counts().await
    }

    /// Cancels every background job and waits for them to exit.
    pub async fn close(&self) {
        let drained: Vec<(String, AbortHandle)> = {
            let mut tasks = self.background_tasks.lock().await;
            tasks.drain().collect()
        };
        for (_, handle) in &drained {
            handle.abort();
        }
        // Give the tasks a chance to actually exit, then mark each
        // remaining running job as cancelled.
        for (job_id, _) in &drained {
            self.mark_job_cancelled(job_id).await;
        }
    }

    /// Presents a scheduled notification that is firing right now.
    pub async fn present_scheduled_notification(
        &self,
        session_name: &str,
        raw_content: &str,
    ) -> Result<String, anyhow::Error> {
        let content = raw_content.trim().to_string();
        if content.is_empty() {
            return Ok(String::new());
        }
        let lock = self.session_lock(session_name).await;
        let _guard = lock.lock().await;
        if self.is_session_deleted(session_name).await {
            return Ok(String::new());
        }
        let mut session = self.load_session_internal(session_name).await?;
        let role_card = self.resolve_session_role(&mut session).await;
        let final_text = self
            .roleplay_engine
            .present_scheduled_notification(&session, &content, &role_card)
            .await
            .unwrap_or_else(|e| {
                warn!(error = %e, "present_scheduled_notification failed; returning raw");
                content.clone()
            });
        if !final_text.trim().is_empty() {
            session.history.push(LLMMessage {
                role: MessageRole::Assistant,
                content: MessageContent::Text(final_text.clone()),
                name: None,
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning_content: String::new(),
                reasoning_field: echobot_core::models::ReasoningField::default(),
            });
            self.save_session_internal(&session).await?;
        }
        Ok(final_text)
    }

    // -------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_job(
        self: Arc<Self>,
        job_id: String,
        session_name: String,
        prompt: String,
        image_urls: Option<Vec<Value>>,
        file_attachments: Option<Vec<Value>>,
        handoff_text: Option<String>,
        continuation_text: Option<String>,
        trace_run_id: Option<String>,
        completion_callback: Option<CompletionCallback>,
    ) {
        let mut transient: Vec<String> = Vec::new();
        if let Some(t) = handoff_text {
            if !t.trim().is_empty() {
                transient.push(t);
            }
        }
        if let Some(t) = continuation_text {
            if !t.trim().is_empty() {
                transient.push(t);
            }
        }
        let transient_option: Option<Vec<String>> = if transient.is_empty() {
            None
        } else {
            Some(transient)
        };

        let image_urls_slice: Option<&[Value]> = image_urls.as_deref();
        let file_attachments_slice: Option<&[Value]> = file_attachments.as_deref();
        let result = self
            .agent_runner
            .run_prompt(
                &session_name,
                &prompt,
                image_urls_slice,
                file_attachments_slice,
                false,
                None,
                transient_option.as_deref(),
                None,
                None,
                trace_run_id.as_deref(),
            )
            .await;
        match result {
            Ok(execution) => {
                let raw_content =
                    message_content_to_text(&execution.agent_result.response.message.content)
                        .trim()
                        .to_string();
                let outbound_content_blocks = execution.agent_result.outbound_content_blocks.clone();
                let awaiting_user_input = execution.agent_result.status == "waiting_for_input";
                let scheduled_job = extract_scheduled_cron_job(&execution.agent_result.new_messages);
                let (final_text, final_content, _visible_role_name) = self
                    .finalize_visible_result(
                        &session_name,
                        &prompt,
                        image_urls.as_ref(),
                        file_attachments.as_ref(),
                        &raw_content,
                        false,
                        scheduled_job,
                        Some(outbound_content_blocks),
                        awaiting_user_input,
                        if awaiting_user_input {
                            Some(execution.agent_result.response.message.content.clone())
                        } else {
                            None
                        },
                        execution.agent_result.pending_user_input.clone(),
                    )
                    .await;
                if awaiting_user_input {
                    self.jobs
                        .set_waiting_for_input(
                            &job_id,
                            &final_text,
                            final_content,
                            execution.agent_result.steps,
                            execution
                                .agent_result
                                .pending_user_input
                                .as_ref()
                                .and_then(|v| v.as_object().cloned())
                                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                        )
                        .await;
                } else {
                    self.jobs
                        .set_completed(
                            &job_id,
                            &final_text,
                            final_content,
                            execution.agent_result.steps,
                        )
                        .await;
                }
            }
            Err(e) => {
                let error_text = e.to_string();
                let (final_text, final_content, _role_name) = self
                    .finalize_visible_result(
                        &session_name,
                        &prompt,
                        image_urls.as_ref(),
                        file_attachments.as_ref(),
                        &error_text,
                        true,
                        None,
                        None,
                        false,
                        None,
                        None,
                    )
                    .await;
                self.jobs
                    .set_failed(&job_id, &final_text, final_content, &error_text, 0)
                    .await;
            }
        };

        let Some(updated) = self.jobs.get(&job_id).await else {
            return;
        };
        if updated.final_response.trim().is_empty() {
            return;
        }
        if let Some(callback) = completion_callback {
            callback(updated).await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize_visible_result(
        &self,
        session_name: &str,
        prompt: &str,
        image_urls: Option<&Vec<Value>>,
        file_attachments: Option<&Vec<Value>>,
        raw_content: &str,
        is_error: bool,
        scheduled_job: Option<ScheduledCronJobInfo>,
        outbound_content_blocks: Option<Vec<Value>>,
        bypass_roleplay: bool,
        direct_response_content: Option<MessageContent>,
        pending_user_input: Option<Value>,
    ) -> (String, MessageContent, String) {
        let lock = self.session_lock(session_name).await;
        let _guard = lock.lock().await;
        if self.is_session_deleted(session_name).await {
            return (String::new(), MessageContent::Text(String::new()), String::new());
        }
        let mut session = match self.load_session_internal(session_name).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, session = %session_name, "finalize: load_session failed");
                return (String::new(), MessageContent::Text(String::new()), String::new());
            }
        };
        let role_card = self.resolve_session_role(&mut session).await;
        let mut metadata_changed = false;
        let normalized_pending = normalize_pending_user_input(pending_user_input.as_ref());
        if let Some(ref pending) = normalized_pending {
            session.metadata = set_pending_user_input(&session.metadata, pending);
            metadata_changed = true;
        }

        let final_content: MessageContent = if bypass_roleplay {
            let mut lead_in = String::new();
            if let Some(ref pending) = normalized_pending {
                lead_in = self
                    .roleplay_engine
                    .present_user_input_request(
                        &session,
                        &pending.prompt,
                        &pending.choices,
                        &pending.why_needed,
                        &role_card,
                    )
                    .await
                    .unwrap_or_default();
            }
            let direct = direct_response_content
                .clone()
                .unwrap_or_else(|| MessageContent::Text(String::new()));
            let outbound = outbound_content_blocks.clone().unwrap_or_default();
            build_visible_response_content(&lead_in, Some(&direct), Some(&outbound))
        } else if !raw_content.trim().is_empty() {
            let final_text = if is_error {
                self.roleplay_engine
                    .present_agent_failure(
                        &session,
                        prompt,
                        raw_content,
                        image_urls.map(|v| v.as_slice()),
                        file_attachments.map(|v| v.as_slice()),
                        &role_card,
                    )
                    .await
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "present_agent_failure failed; returning raw error");
                        format!("The task failed: {raw_content}")
                    })
            } else if let Some(scheduled) = scheduled_job {
                self.roleplay_engine
                    .present_scheduled_setup_result(
                        &session,
                        prompt,
                        raw_content,
                        image_urls.map(|v| v.as_slice()),
                        file_attachments.map(|v| v.as_slice()),
                        &scheduled,
                        &role_card,
                    )
                    .await
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "present_scheduled_setup_result failed; returning raw");
                        raw_content.trim().to_string()
                    })
            } else {
                self.roleplay_engine
                    .present_agent_result(
                        &session,
                        prompt,
                        raw_content,
                        image_urls.map(|v| v.as_slice()),
                        file_attachments.map(|v| v.as_slice()),
                        &role_card,
                    )
                    .await
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "present_agent_result failed; returning raw");
                        raw_content.trim().to_string()
                    })
            };
            let outbound = outbound_content_blocks.clone().unwrap_or_default();
            build_visible_response_content(&final_text, None, Some(&outbound))
        } else {
            let outbound = outbound_content_blocks.clone().unwrap_or_default();
            build_visible_response_content("", None, Some(&outbound))
        };

        let mut appended_message = false;
        if !is_message_content_empty(&final_content) {
            session.history.push(LLMMessage {
                role: MessageRole::Assistant,
                content: final_content.clone(),
                name: None,
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning_content: String::new(),
                reasoning_field: echobot_core::models::ReasoningField::default(),
            });
            appended_message = true;
        }
        if appended_message || metadata_changed {
            if let Err(e) = self.save_session_internal(&session).await {
                warn!(error = %e, session = %session_name, "finalize: save_session failed");
            }
        }
        (
            message_content_to_text(&final_content).trim().to_string(),
            final_content,
            role_card.name,
        )
    }

    async fn append_cancelled_message(&self, session_name: &str) -> String {
        let final_text = JOB_CANCELLED_TEXT.to_string();
        let lock = self.session_lock(session_name).await;
        let _guard = lock.lock().await;
        if self.is_session_deleted(session_name).await {
            return String::new();
        }
        let mut session = match self.load_session_internal(session_name).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, session = %session_name, "append_cancelled_message: load failed");
                return String::new();
            }
        };
        session.history.push(LLMMessage {
            role: MessageRole::Assistant,
            content: MessageContent::Text(final_text.clone()),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: String::new(),
            reasoning_field: echobot_core::models::ReasoningField::default(),
        });
        let _ = self.save_session_internal(&session).await;
        final_text
    }

    async fn resolve_turn_role(
        &self,
        session: &mut ChatSession,
        role_name: Option<&str>,
    ) -> RoleCard {
        if let Some(name) = role_name {
            if let Some(card) = self.role_registry.get(Some(name)).await {
                session.metadata = set_role_name(&session.metadata, &card.name);
                return card;
            }
        }
        self.resolve_session_role(session).await
    }

    async fn resolve_session_role(&self, session: &mut ChatSession) -> RoleCard {
        let role_name = role_name_from_metadata(Some(&session.metadata));
        if let Some(card) = self.role_registry.get(Some(&role_name)).await {
            session.metadata = set_role_name(&session.metadata, &card.name);
            return card;
        }
        let default = self
            .role_registry
            .get(None)
            .await
            .unwrap_or_else(|| RoleCard {
                name: "default".to_string(),
                prompt: String::new(),
                source_path: None,
            });
        session.metadata = set_role_name(&session.metadata, &default.name);
        default
    }

    fn resolve_turn_route_mode(
        &self,
        session: &ChatSession,
        route_mode: Option<RouteMode>,
    ) -> RouteMode {
        route_mode.unwrap_or_else(|| route_mode_from_metadata(Some(&session.metadata)))
    }

    async fn mark_job_cancelled(&self, job_id: &str) {
        let Some(job) = self.jobs.get(job_id).await else {
            return;
        };
        if job.status != "running" {
            return;
        }
        self.jobs
            .set_cancelled(
                job_id,
                JOB_CANCELLED_TEXT,
                MessageContent::Text(JOB_CANCELLED_TEXT.to_string()),
                0,
            )
            .await;
    }

    async fn session_lock(&self, session_name: &str) -> Arc<Mutex<()>> {
        if let Some(lock) = self.session_locks.get(session_name) {
            return lock.clone();
        }
        let lock = Arc::new(Mutex::new(()));
        self.session_locks
            .insert(session_name.to_string(), lock.clone());
        lock
    }

    async fn is_session_deleted(&self, session_name: &str) -> bool {
        self.deleted_sessions.read().await.contains(session_name)
    }

    fn start_background_job(self: &Arc<Self>, job_id: String, factory: BackgroundJobFactory) {
        let handle = tokio::spawn(async move {
            (factory)().await;
        });
        let abort = handle.abort_handle();
        let me = self.clone();
        let job_id = job_id.clone();
        tokio::spawn(async move {
            let mut tasks = me.background_tasks.lock().await;
            tasks.insert(job_id, abort);
        });
    }

    async fn load_session_internal(
        &self,
        session_name: &str,
    ) -> Result<ChatSession, anyhow::Error> {
        let store = self.session_store.clone();
        let name = session_name.to_string();
        let session = store.load_or_create_session(&name).await?;
        Ok(session)
    }

    async fn save_session_internal(&self, session: &ChatSession) -> Result<(), anyhow::Error> {
        let store = self.session_store.clone();
        let snapshot = session.clone();
        store.save_session(&snapshot).await?;
        Ok(())
    }
}

fn discard_stream_chunk() -> StreamCallback {
    Arc::new(|_chunk: String| Box::pin(async {}))
}

#[async_trait::async_trait]
impl CoordinatorLike for ConversationCoordinator {
    fn present_scheduled_notification<'a>(
        &'a self,
        session_name: &'a str,
        content: &'a str,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<String, echobot_runtime::error::Error>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.present_scheduled_notification(session_name, content)
                .await
                .map_err(|e| echobot_runtime::error::Error::Wiring(e.to_string()))
        })
    }
}

// ---------------------------------------------------------------------------
// Pending-user-input helpers (verbatim ports of the Python helpers).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NormalizedPendingUserInput {
    pub prompt: String,
    pub choices: Vec<String>,
    pub why_needed: String,
}

pub fn normalize_pending_user_input(value: Option<&Value>) -> Option<NormalizedPendingUserInput> {
    let obj = value?.as_object()?;
    let prompt = obj
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if prompt.is_empty() {
        return None;
    }
    let choices = obj
        .get("choices")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::trim).map(str::to_string))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let why_needed = obj
        .get("why_needed")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    Some(NormalizedPendingUserInput {
        prompt: prompt.to_string(),
        choices,
        why_needed,
    })
}

pub fn pending_user_input_from_metadata(
    metadata: Option<&HashMap<String, Value>>,
) -> Option<NormalizedPendingUserInput> {
    let metadata = metadata?;
    let value = metadata.get(PENDING_USER_INPUT_METADATA_KEY)?;
    normalize_pending_user_input(Some(value))
}

pub fn set_pending_user_input(
    metadata: &HashMap<String, Value>,
    pending: &NormalizedPendingUserInput,
) -> HashMap<String, Value> {
    let mut next = metadata.clone();
    let mut map = serde_json::Map::new();
    map.insert("prompt".to_string(), Value::String(pending.prompt.clone()));
    if !pending.choices.is_empty() {
        map.insert(
            "choices".to_string(),
            Value::Array(pending.choices.iter().map(|c| Value::String(c.clone())).collect()),
        );
    }
    if !pending.why_needed.is_empty() {
        map.insert(
            "why_needed".to_string(),
            Value::String(pending.why_needed.clone()),
        );
    }
    next.insert(
        PENDING_USER_INPUT_METADATA_KEY.to_string(),
        Value::Object(map),
    );
    next
}

pub fn clear_pending_user_input(metadata: &HashMap<String, Value>) -> HashMap<String, Value> {
    let mut next = metadata.clone();
    next.remove(PENDING_USER_INPUT_METADATA_KEY);
    next
}

pub fn build_pending_user_input_continuation_text(
    pending: Option<&NormalizedPendingUserInput>,
) -> Option<String> {
    let pending = pending?;
    let mut lines = vec![
        "The previous agent run paused to request missing information.".to_string(),
        "Treat the current user message as the latest reply to that request or as a new instruction that supersedes it.".to_string(),
        "Continue the task with the hidden agent history from the previous run.".to_string(),
        String::new(),
        "Pending user input request:".to_string(),
        pending.prompt.clone(),
    ];
    if !pending.choices.is_empty() {
        lines.push(String::new());
        lines.push("Suggested choices:".to_string());
        for choice in &pending.choices {
            lines.push(format!("- {choice}"));
        }
    }
    if !pending.why_needed.is_empty() {
        lines.push(String::new());
        lines.push("Why this was needed:".to_string());
        lines.push(pending.why_needed.clone());
    }
    Some(lines.join("\n"))
}

fn forced_agent_decision_for_pending_input() -> crate::decision::RouteDecision {
    crate::decision::RouteDecision::forced_agent("Continue paused task after request_user_input")
}

fn build_agent_handoff_text(session: &ChatSession) -> Option<String> {
    let entries = collect_handoff_entries(&session.history);
    if entries.is_empty() {
        return None;
    }
    let mut lines = vec![
        "Visible conversation handoff from the lightweight roleplay layer.".to_string(),
        String::new(),
        "The current user request follows immediately after this handoff.".to_string(),
        "Use the visible context below to resolve references such as 'that script', 'the previous result', or 'the list above'.".to_string(),
        "Treat these messages as user-visible conversation context. If they mention files, memory, schedules, or tool results, verify them with tools before relying on them.".to_string(),
        format!("Session name: {}", session.name),
        String::new(),
        "Recent visible messages:".to_string(),
    ];
    for (idx, entry) in entries.iter().enumerate() {
        lines.push(format!(
            "<visible_message index=\"{}\" role=\"{}\">",
            idx + 1,
            entry.role
        ));
        lines.push(entry.content.clone());
        lines.push("</visible_message>".to_string());
        lines.push(String::new());
    }
    Some(lines.join("\n").trim().to_string())
}

#[derive(Debug, Clone)]
struct HandoffEntry {
    role: String,
    content: String,
}

fn collect_handoff_entries(history: &[LLMMessage]) -> Vec<HandoffEntry> {
    let mut selected: Vec<HandoffEntry> = Vec::new();
    let mut remaining = AGENT_HANDOFF_MAX_TOTAL_CHARS;
    for message in history.iter().rev() {
        if !matches!(message.role, MessageRole::User | MessageRole::Assistant) {
            continue;
        }
        let content = message.content_text();
        let content = content.trim();
        if content.is_empty() {
            continue;
        }
        if remaining == 0 {
            break;
        }
        let max = std::cmp::min(AGENT_HANDOFF_MAX_MESSAGE_CHARS, remaining);
        let trimmed = trim_handoff_content(content, max);
        if trimmed.is_empty() {
            continue;
        }
        selected.push(HandoffEntry {
            role: message.role.as_str().to_string(),
            content: trimmed.clone(),
        });
        remaining = remaining.saturating_sub(trimmed.len());
        if selected.len() >= AGENT_HANDOFF_MAX_MESSAGES {
            break;
        }
    }
    selected.reverse();
    selected
}

fn trim_handoff_content(content: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let stripped = content.trim();
    if stripped.len() <= max_chars {
        return stripped.to_string();
    }
    if max_chars <= 16 {
        return stripped.chars().take(max_chars).collect();
    }
    let cut = max_chars.saturating_sub(16);
    let mut out: String = stripped.chars().take(cut).collect();
    out = out.trim_end().to_string();
    out.push_str("\n...[truncated]");
    out
}

pub fn build_visible_response_content(
    text: &str,
    base_content: Option<&MessageContent>,
    outbound_content_blocks: Option<&[Value]>,
) -> MessageContent {
    let mut blocks: Vec<Value> = Vec::new();
    let cleaned = text.trim();
    if !cleaned.is_empty() {
        blocks.push(serde_json::json!({
            "type": TEXT_CONTENT_BLOCK_TYPE,
            "text": cleaned,
        }));
    }
    let base = base_content
        .cloned()
        .unwrap_or_else(|| MessageContent::Text(String::new()));
    for block in message_content_blocks(&base) {
        blocks.push(block.to_value());
    }
    for block in outbound_content_blocks.unwrap_or(&[]) {
        if let Some(parsed) = MessageContentBlock::from_value(block.clone()) {
            blocks.push(parsed.to_value());
        } else {
            blocks.push(block.clone());
        }
    }
    if blocks.is_empty() {
        return MessageContent::Text(String::new());
    }
    if blocks.len() == 1 {
        if let Some(obj) = blocks[0].as_object() {
            if obj.get("type").and_then(Value::as_str) == Some(TEXT_CONTENT_BLOCK_TYPE) {
                if let Some(text) = obj.get("text").and_then(Value::as_str) {
                    return MessageContent::Text(text.to_string());
                }
            }
        }
    }
    MessageContent::Blocks(
        blocks
            .into_iter()
            .filter_map(|v| MessageContentBlock::from_value(v))
            .collect(),
    )
}

fn extract_scheduled_cron_job(messages: &[LLMMessage]) -> Option<ScheduledCronJobInfo> {
    let mut cron_add_calls: HashMap<String, String> = HashMap::new();
    for message in messages {
        if message.role != MessageRole::Assistant {
            continue;
        }
        for tool_call in &message.tool_calls {
            if tool_call.name != "cron" {
                continue;
            }
            let parsed: Option<Value> = serde_json::from_str(&tool_call.arguments).ok();
            let Some(obj) = parsed.and_then(|v| v.as_object().cloned()) else {
                continue;
            };
            let action = obj
                .get("action")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_lowercase)
                .unwrap_or_default();
            if action != "add" {
                continue;
            }
            let content = obj
                .get("content")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            cron_add_calls.insert(tool_call.id.clone(), content);
        }
    }
    if cron_add_calls.is_empty() {
        return None;
    }
    for message in messages {
        if message.role != MessageRole::Tool {
            continue;
        }
        let Some(call_id) = &message.tool_call_id else {
            continue;
        };
        if !cron_add_calls.contains_key(call_id) {
            continue;
        }
        let parsed: Option<Value> = serde_json::from_str(&message.content_text()).ok();
        let Some(obj) = parsed.and_then(|v| v.as_object().cloned()) else {
            continue;
        };
        if !obj.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let Some(result) = obj.get("result").and_then(|v| v.as_object().cloned()) else {
            continue;
        };
        if !result.get("created").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let Some(job) = result.get("job").and_then(|v| v.as_object().cloned()) else {
            continue;
        };
        let name = job.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let schedule = job.get("schedule").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let next_run_at = job
            .get("next_run_at")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let payload_kind = job
            .get("payload_kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let payload_content = cron_add_calls.get(call_id).cloned().unwrap_or_default();
        return Some(ScheduledCronJobInfo {
            name,
            schedule,
            next_run_at,
            payload_kind,
            payload_content,
        });
    }
    None
}

// silence unused-import warnings
#[allow(dead_code)]
fn _silence_unused(_v: &dyn std::fmt::Debug, _cb: Option<TraceCallback>, _s: &AgentTraceStore) {}

// silence unused Stream import
#[allow(dead_code)]
fn _silence_stream(_s: &dyn Stream<Item = String>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_agent_handoff_text_returns_none_for_empty_history() {
        let session = ChatSession::new("test");
        assert!(build_agent_handoff_text(&session).is_none());
    }

    #[test]
    fn collect_handoff_entries_respects_limits() {
        let mut history = Vec::new();
        for i in 0..10 {
            history.push(LLMMessage {
                role: MessageRole::User,
                content: MessageContent::Text(format!(
                    "message {i} with some padding to take space"
                )),
                name: None,
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning_content: String::new(),
                reasoning_field: echobot_core::models::ReasoningField::default(),
            });
        }
        let entries = collect_handoff_entries(&history);
        assert!(entries.len() <= AGENT_HANDOFF_MAX_MESSAGES);
        let total: usize = entries.iter().map(|e| e.content.len()).sum();
        assert!(total <= AGENT_HANDOFF_MAX_TOTAL_CHARS);
    }

    #[test]
    fn trim_handoff_content_truncates() {
        let trimmed = trim_handoff_content("a".repeat(200).as_str(), 32);
        assert!(trimmed.ends_with("...[truncated]"));
        assert!(trimmed.len() <= 32);
    }

    #[test]
    fn normalize_pending_user_input_handles_invalid() {
        assert!(normalize_pending_user_input(None).is_none());
        assert!(normalize_pending_user_input(Some(&Value::Null)).is_none());
        assert!(normalize_pending_user_input(Some(&Value::String("x".into()))).is_none());
    }

    #[test]
    fn build_visible_response_content_text_shortcut() {
        let content = build_visible_response_content("hi", None, None);
        match content {
            MessageContent::Text(t) => assert_eq!(t, "hi"),
            _ => panic!("expected text"),
        }
    }
}
