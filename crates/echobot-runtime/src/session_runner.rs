//! Per-session agent runner.
//!
//! Mirrors `echobot/runtime/session_runner.py`. Wraps the agent core with
//! per-session locks, trace callbacks, and history persistence.

use std::collections::HashMap;
use std::sync::Arc;

use echobot_core::models::{FileInput, ImageInput, LLMMessage, MessageRole};
use tokio::sync::Mutex;

use crate::agent_traces::AgentTraceStore;
use crate::error::{Error, Result};
use crate::sessions::{ChatSession, SessionStore};
use crate::turns::{run_agent_turn, AgentCoreLike, AgentRunResult, SkillRegistryLike, ToolRegistryLike, TraceCallback};

/// Factory that produces a per-session tool registry. Mirrors the Python
/// `ToolRegistryFactory`.
pub type ToolRegistryFactory = Box<
    dyn Fn(&str, bool) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Box<dyn ToolRegistryLike>>> + Send>>
        + Send
        + Sync,
>;

/// The result of running a single prompt through the session runner.
#[derive(Debug, Clone)]
pub struct SessionRunResult {
    /// The session after the turn (history + metadata updated).
    pub session: ChatSession,
    /// The agent's result.
    pub agent_result: AgentRunResult,
    /// The trace run id, if tracing is enabled.
    pub trace_run_id: Option<String>,
}

/// Per-session agent runner.
pub struct SessionAgentRunner {
    agent: Arc<dyn AgentCoreLike>,
    session_store: SessionStore,
    skill_registry: Option<Arc<dyn SkillRegistryLike>>,
    tool_registry_factory: Option<ToolRegistryFactory>,
    default_temperature: Option<f32>,
    default_max_tokens: Option<u32>,
    default_max_steps: usize,
    trace_store: Option<AgentTraceStore>,
    session_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    deleted_sessions: Mutex<HashMap<String, ()>>,
}

impl std::fmt::Debug for SessionAgentRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionAgentRunner")
            .field("default_max_steps", &self.default_max_steps)
            .finish()
    }
}

impl SessionAgentRunner {
    /// Creates a new runner.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent: Arc<dyn AgentCoreLike>,
        session_store: SessionStore,
        skill_registry: Option<Arc<dyn SkillRegistryLike>>,
        tool_registry_factory: Option<ToolRegistryFactory>,
        default_temperature: Option<f32>,
        default_max_tokens: Option<u32>,
        default_max_steps: usize,
        trace_store: Option<AgentTraceStore>,
    ) -> Self {
        Self {
            agent,
            session_store,
            skill_registry,
            tool_registry_factory,
            default_temperature,
            default_max_tokens,
            default_max_steps: default_max_steps.max(1),
            trace_store,
            session_locks: Mutex::new(HashMap::new()),
            deleted_sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Loads (or creates) a session.
    pub async fn load_session(&self, session_name: &str) -> Result<ChatSession> {
        let lock = self.lock_for(session_name).await;
        let _guard = lock.lock().await;
        self.session_store.load_or_create_session(session_name).await
    }

    /// Marks a session as deleted (subsequent `run_prompt` calls fail).
    pub async fn mark_session_deleted(&self, session_name: &str) {
        self.deleted_sessions
            .lock()
            .await
            .insert(session_name.to_string(), ());
    }

    /// Unmarks a session as deleted.
    pub async fn restore_session(&self, session_name: &str) {
        self.deleted_sessions.lock().await.remove(session_name);
    }

    /// Runs `prompt` against the session.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_prompt(
        &self,
        session_name: &str,
        prompt: &str,
        image_urls: Option<&[ImageInput]>,
        file_attachments: Option<&[FileInput]>,
        scheduled_context: bool,
        extra_system_messages: Option<&[String]>,
        transient_system_messages: Option<&[String]>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        trace_run_id: Option<&str>,
    ) -> Result<SessionRunResult> {
        let lock = self.lock_for(session_name).await;
        let _guard = lock.lock().await;
        if self.is_deleted(session_name).await {
            return Err(Error::SessionDeleted(session_name.to_string()));
        }

        let session = self
            .session_store
            .load_or_create_session(session_name)
            .await?;
        let tool_registry: Option<Box<dyn ToolRegistryLike>> =
            if let Some(factory) = &self.tool_registry_factory {
                factory(&session.name, scheduled_context).await
            } else {
                None
            };

        let (trace_callback, active_run_id) =
            self.build_trace_callback(&session.name, trace_run_id).await;

        if let Some(cb) = &trace_callback {
            cb(
                "turn_started".to_string(),
                serde_json::json!({
                    "prompt": prompt,
                    "image_count": image_urls.map(|v| v.len()).unwrap_or(0),
                    "file_count": file_attachments.map(|v| v.len()).unwrap_or(0),
                    "scheduled_context": scheduled_context,
                    "history_length": session.history.len(),
                    "tool_names": tool_registry
                        .as_ref()
                        .map(|r| r.names())
                        .unwrap_or_default(),
                    "extra_system_messages_count": extra_system_messages.map(|v| v.len()).unwrap_or(0),
                    "transient_system_messages_count": transient_system_messages
                        .map(|v| v.len())
                        .unwrap_or(0),
                }),
            )
            .await;
        }

        let result = {
            let result = run_agent_turn(
                self.agent.as_ref(),
                prompt,
                session.history.clone(),
                image_urls,
                file_attachments,
                &session.compressed_summary,
                self.skill_registry.as_ref().map(|s| s.as_ref()),
                tool_registry.as_ref().map(|b| b.as_ref()),
                extra_system_messages,
                transient_system_messages,
                self.default_temperature.or(temperature),
                self.default_max_tokens.or(max_tokens),
                self.default_max_steps,
                trace_callback.clone(),
            )
            .await;

            match result {
                Ok(r) => r,
                Err(e) => {
                    if let Some(cb) = &trace_callback {
                        cb(
                            "turn_failed".to_string(),
                            serde_json::json!({
                                "error": e.to_string(),
                                "error_type": "Error",
                            }),
                        )
                        .await;
                    }
                    return Err(e);
                }
            }
        };

        let mut session = session;
        session.history = result.history.clone();
        session.compressed_summary = result.compressed_summary.clone();
        if !self.is_deleted(&session.name).await {
            self.session_store.save_session(&session).await?;
        }

        if let Some(cb) = &trace_callback {
            cb(
                "turn_completed".to_string(),
                serde_json::json!({
                    "steps": result.steps,
                    "status": result.status,
                    "history_length": session.history.len(),
                    "final_message": message_to_trace_dict(&result.response.message),
                    "usage": result.response.usage.to_value(),
                    "compressed_summary": session.compressed_summary,
                    "pending_user_input": result.pending_user_input,
                }),
            )
            .await;
        }

        Ok(SessionRunResult {
            session,
            agent_result: result,
            trace_run_id: active_run_id,
        })
    }

    /// Appends a single assistant message to a session's history and saves it.
    pub async fn append_assistant_message(
        &self,
        session_name: &str,
        content: &str,
    ) -> Result<ChatSession> {
        let lock = self.lock_for(session_name).await;
        let _guard = lock.lock().await;
        if self.is_deleted(session_name).await {
            return Ok(ChatSession {
                name: session_name.to_string(),
                history: Vec::new(),
                updated_at: String::new(),
                compressed_summary: String::new(),
                metadata: HashMap::new(),
            });
        }
        let mut session = self
            .session_store
            .load_or_create_session(session_name)
            .await?;
        session
            .history
            .push(LLMMessage::text(MessageRole::Assistant, content));
        if !self.is_deleted(&session.name).await {
            self.session_store.save_session(&session).await?;
        }
        Ok(session)
    }

    /// Returns the recorded events for a run, or `None` when tracing is
    /// disabled / no run id was provided.
    pub async fn load_trace_events(
        &self,
        session_name: &str,
        run_id: &str,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let Some(store) = &self.trace_store else {
            return Ok(Vec::new());
        };
        if run_id.trim().is_empty() {
            return Ok(Vec::new());
        }
        let store = store.clone();
        let session = session_name.to_string();
        let run = run_id.to_string();
        let events = tokio::task::spawn_blocking(move || store.read_events(&session, &run))
            .await
            .map_err(|e| Error::Wiring(format!("trace read task failed: {e}")))??;
        Ok(events)
    }

    /// Generates a new run id, or `None` if tracing is disabled.
    pub fn create_trace_run_id(&self) -> Option<String> {
        self.trace_store.as_ref().map(|s| s.create_run_id())
    }

    async fn lock_for(&self, session_name: &str) -> Arc<Mutex<()>> {
        let mut locks = self.session_locks.lock().await;
        if let Some(existing) = locks.get(session_name).cloned() {
            return existing;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(session_name.to_string(), lock.clone());
        lock
    }

    async fn is_deleted(&self, session_name: &str) -> bool {
        self.deleted_sessions
            .lock()
            .await
            .contains_key(session_name)
    }

    async fn build_trace_callback(
        &self,
        session_name: &str,
        trace_run_id: Option<&str>,
    ) -> (Option<TraceCallback>, Option<String>) {
        let Some(store) = self.trace_store.clone() else {
            return (None, None);
        };
        let run_id = trace_run_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| store.create_run_id());
        let session = session_name.to_string();
        let run = run_id.clone();
        let cb: TraceCallback = Arc::new(move |event, data| {
            let store = store.clone();
            let session = session.clone();
            let run = run.clone();
            let data_value = data;
            let mut data_map = serde_json::Map::new();
            if let serde_json::Value::Object(map) = data_value {
                data_map = map;
            }
            Box::pin(async move {
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    store.append_event(&session, &run, &event, Some(data_map))
                })
                .await
                .map_err(|e| Error::Wiring(format!("trace write task failed: {e}")))
                .and_then(|inner| inner)
                {
                    tracing::warn!(error = %e, "failed to append trace event");
                }
            })
        });
        (Some(cb), Some(run_id))
    }
}

fn message_to_trace_dict(message: &LLMMessage) -> serde_json::Value {
    serde_json::json!({
        "role": message.role.as_str(),
        "content": serde_json::to_value(&message.content).unwrap_or(serde_json::Value::Null),
        "content_text": message.content.to_text(),
        "name": message.name,
        "tool_call_id": message.tool_call_id,
        "tool_calls": message.tool_calls.iter().map(|t| serde_json::json!({
            "id": t.id,
            "name": t.name,
            "arguments": t.arguments,
        })).collect::<Vec<_>>(),
    })
}
