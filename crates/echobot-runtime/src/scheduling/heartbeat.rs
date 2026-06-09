//! Heartbeat service: periodically reads `.echobot/HEARTBEAT.md` and runs the
//! `on_execute` closure when active tasks are detected.
//!
//! Mirrors `echobot/scheduling/heartbeat/*`. The decision step
//! (`LLMProvider.generate(heartbeat_decision)`) is out of scope for this
//! crate; the Rust version relies on the caller to plug in an [`LLMProvider`]
//! (e.g. via the orchestrator) and uses the decision-tool contract by hand.
//!
//! For a v1, this module ships the file-reading / scheduling surface and a
//! stub for the LLM-decision step. The stub returns `skip` so the service
//! stays inert until the orchestrator wires the provider in.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use echobot_core::models::{LLMMessage, LLMTool, MessageRole};
use echobot_providers::LLMProvider;

use crate::error::{Error, Result};

/// Default contents of a freshly-created `HEARTBEAT.md`.
pub const DEFAULT_HEARTBEAT_TEMPLATE: &str = "\
# HEARTBEAT.md

<!--
Add periodic tasks here.
Keep only active tasks in this file.
-->
";

/// Signature of the executor the heartbeat service invokes. The string is
/// the natural-language task list extracted from `HEARTBEAT.md`. Returns the
/// visible response (or `None` if nothing to surface).
pub type HeartbeatExecutor = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<String>>> + Send>>
        + Send
        + Sync,
>;

/// Notifier called with the executor's response (if any).
pub type HeartbeatNotifier = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Tool definition used when asking the LLM to decide whether to act.
pub fn heartbeat_decision_tool() -> LLMTool {
    LLMTool::new(
        "heartbeat_decision",
        "Decide whether HEARTBEAT.md contains active tasks. \
         Use action=skip when there is nothing actionable.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["skip", "run"],
                },
                "tasks": {
                    "type": "string",
                    "description": "Short task summary for the next full agent run.",
                },
            },
            "required": ["action"],
            "additionalProperties": false,
        }),
    )
}

/// Heartbeat service. Periodically polls the heartbeat file and dispatches
/// the executor when the decision LLM returns `action=run`.
pub struct HeartbeatService {
    /// Path to the heartbeat markdown file.
    pub heartbeat_file: PathBuf,
    /// LLM provider used for the decision step.
    pub provider: Arc<dyn LLMProvider>,
    /// Closure invoked when active tasks are detected.
    pub on_execute: Option<HeartbeatExecutor>,
    /// Closure invoked with the executor's response.
    pub on_notify: Option<HeartbeatNotifier>,
    /// Interval between checks (seconds).
    pub interval_seconds: u64,
    /// Whether the service is enabled.
    pub enabled: bool,
    state: Arc<Mutex<HeartbeatState>>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

struct HeartbeatState {
    running: bool,
}

impl std::fmt::Debug for HeartbeatService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeartbeatService")
            .field("heartbeat_file", &self.heartbeat_file)
            .field("interval_seconds", &self.interval_seconds)
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl HeartbeatService {
    /// Creates a new heartbeat service.
    pub fn new(
        heartbeat_file: impl Into<PathBuf>,
        provider: Arc<dyn LLMProvider>,
        on_execute: Option<HeartbeatExecutor>,
        on_notify: Option<HeartbeatNotifier>,
        interval_seconds: u64,
        enabled: bool,
    ) -> Self {
        Self {
            heartbeat_file: heartbeat_file.into(),
            provider,
            on_execute,
            on_notify,
            interval_seconds: interval_seconds.max(1),
            enabled,
            state: Arc::new(Mutex::new(HeartbeatState { running: false })),
            task: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts the heartbeat loop. Creates the heartbeat file with the default
    /// template if missing.
    pub async fn start(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        {
            let state = self.state.lock().await;
            if state.running {
                return Ok(());
            }
        }
        ensure_heartbeat_file(&self.heartbeat_file).await?;
        {
            let mut state = self.state.lock().await;
            state.running = true;
        }
        let provider = self.provider.clone();
        let file = self.heartbeat_file.clone();
        let on_execute = self.on_execute.clone();
        let on_notify = self.on_notify.clone();
        let interval = self.interval_seconds;
        let state = self.state.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(interval)).await;
                let running = {
                    let state = state.lock().await;
                    state.running
                };
                if !running {
                    break;
                }
                if let Err(e) = tick(
                    &file,
                    &provider,
                    on_execute.as_ref(),
                    on_notify.as_ref(),
                )
                .await
                {
                    tracing::warn!(error = %e, "heartbeat tick failed");
                }
            }
        });
        *self.task.lock().await = Some(handle);
        Ok(())
    }

    /// Stops the heartbeat loop.
    pub async fn stop(&self) {
        {
            let mut state = self.state.lock().await;
            state.running = false;
        }
        let task = { self.task.lock().await.take() };
        if let Some(handle) = task {
            handle.abort();
            let _ = handle.await;
        }
    }

    /// Triggers a single heartbeat cycle immediately.
    pub async fn trigger_now(&self) -> Result<Option<String>> {
        tick(
            &self.heartbeat_file,
            &self.provider,
            self.on_execute.as_ref(),
            self.on_notify.as_ref(),
        )
        .await
    }
}

async fn tick(
    heartbeat_file: &Path,
    provider: &Arc<dyn LLMProvider>,
    on_execute: Option<&HeartbeatExecutor>,
    on_notify: Option<&HeartbeatNotifier>,
) -> Result<Option<String>> {
    let content = read_heartbeat_file(heartbeat_file).await?;
    if !has_meaningful_heartbeat_content(&content) {
        return Ok(None);
    }
    let (action, tasks) = decide(provider, &content).await?;
    if action != "run" || tasks.is_empty() {
        return Ok(None);
    }
    let Some(executor) = on_execute else {
        return Ok(None);
    };
    let response = executor(tasks).await?;
    if let Some(text) = &response {
        if let Some(notifier) = on_notify {
            notifier(text.clone()).await?;
        }
    }
    Ok(response)
}

async fn decide(provider: &Arc<dyn LLMProvider>, content: &str) -> Result<(String, String)> {
    let tool = heartbeat_decision_tool();
    let tool_choice = echobot_providers::ToolChoice::Structured(serde_json::json!({
        "type": "function",
        "function": {"name": "heartbeat_decision"},
    }));
    let messages = vec![
        LLMMessage::text(
            MessageRole::System,
            "You are a heartbeat checker. \
             Call heartbeat_decision with action=skip or action=run.",
        ),
        LLMMessage::text(
            MessageRole::User,
            format!(
                "Review this HEARTBEAT.md content. \
                 If there are active periodic tasks, return action=run with a short \
                 execution prompt. Otherwise return skip.\n\n{content}"
            ),
        ),
    ];
    let response = provider
        .generate(
            &messages,
            Some(&[tool]),
            Some(&tool_choice),
            None,
            None,
            None,
        )
        .await
        .map_err(|e| Error::HeartbeatFile(e.to_string()))?;
    let tool_calls = if !response.tool_calls.is_empty() {
        response.tool_calls.clone()
    } else {
        response.message.tool_calls.clone()
    };
    let Some(call) = tool_calls.first() else {
        return Ok(("skip".to_string(), String::new()));
    };
    let parsed: serde_json::Value = serde_json::from_str(&call.arguments)
        .unwrap_or_else(|_| serde_json::json!({}));
    let action = parsed
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("skip")
        .trim()
        .to_lowercase();
    let tasks = parsed
        .get("tasks")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let action = if action == "run" { "run" } else { "skip" };
    Ok((action.to_string(), tasks))
}

/// Returns `true` if `content` has any line that's not a comment or heading.
pub fn has_meaningful_heartbeat_content(content: &str) -> bool {
    let comment_re = Regex::new(r"<!--.*?-->").unwrap();
    let without_comments = comment_re.replace_all(content, "");
    for raw_line in without_comments.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        return true;
    }
    false
}

/// Reads `heartbeat_file`, creating the default template if it doesn't exist.
pub async fn read_or_create_heartbeat_file(heartbeat_file: &Path) -> Result<String> {
    if !heartbeat_file.exists() {
        write_heartbeat_file(heartbeat_file, DEFAULT_HEARTBEAT_TEMPLATE).await?;
        return Ok(DEFAULT_HEARTBEAT_TEMPLATE.to_string());
    }
    Ok(tokio::fs::read_to_string(heartbeat_file)
        .await
        .map_err(|e| Error::HeartbeatFile(e.to_string()))?)
}

/// Writes `content` to `heartbeat_file` (creating parents as needed).
pub async fn write_heartbeat_file(heartbeat_file: &Path, content: &str) -> Result<()> {
    if let Some(parent) = heartbeat_file.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::HeartbeatFile(e.to_string()))?;
    }
    tokio::fs::write(heartbeat_file, content)
        .await
        .map_err(|e| Error::HeartbeatFile(e.to_string()))?;
    Ok(())
}

async fn read_heartbeat_file(heartbeat_file: &Path) -> Result<String> {
    if !heartbeat_file.exists() {
        return Ok(String::new());
    }
    tokio::fs::read_to_string(heartbeat_file)
        .await
        .map_err(|e| Error::HeartbeatFile(e.to_string()))
}

async fn ensure_heartbeat_file(heartbeat_file: &Path) -> Result<()> {
    if heartbeat_file.exists() {
        return Ok(());
    }
    write_heartbeat_file(heartbeat_file, DEFAULT_HEARTBEAT_TEMPLATE).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meaningful_content_skips_comments_and_headings() {
        let content = "# heading\n<!-- comment -->\n  \n- [ ] task\n";
        assert!(has_meaningful_heartbeat_content(content));
    }

    #[test]
    fn empty_or_heading_only_is_not_meaningful() {
        let content = "# HEARTBEAT.md\n<!-- only comments -->\n";
        assert!(!has_meaningful_heartbeat_content(content));
    }
}
