//! `AppRuntime` — the long-lived runtime that backs the HTTP server.
//!
//! This is the Rust analogue of the Python `echobot.app.runtime.AppRuntime`.
//! It holds the shared `FullRuntimeContext` (sessions, scheduling, settings,
//! coordinator), plus the TTS / ASR services. For v1 the channel manager
//! is a stub: it always reports "no channels configured".
//!
//! `AppRuntime::start()` is idempotent; `AppRuntime::stop()` is the
//! inverse. The struct is `Send + Sync` (all fields are already
//! thread-safe) and is wrapped in `Arc` by [`crate::AppState`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use echobot_asr::service::AsrService;
use echobot_orchestration::ConversationCoordinator;
use echobot_orchestration::RoleCardRegistry;
use echobot_runtime::bootstrap::RuntimeContext;
use echobot_runtime::scheduling::cron::CronService;
use echobot_runtime::scheduling::heartbeat::HeartbeatService;
use echobot_runtime::sessions::SessionStore;
use echobot_tts::service::TtsService;

use crate::services::web_console::WebConsoleService;

/// Bundle of the runtime pieces the HTTP layer needs.
///
/// Channels, gateway delivery, and route-session persistence from the
/// Python app are not yet wired in v1 — see the `services::` modules
/// for thin stubs that expose the same shape.
pub struct AppRuntime {
    /// The full runtime context (sessions, scheduling, settings, runner).
    pub context: RuntimeContext,
    /// Conversation coordinator (always present after `start`).
    pub coordinator: Option<Arc<ConversationCoordinator>>,
    /// Role card registry (always present after `start`).
    pub role_registry: Option<Arc<RoleCardRegistry>>,
    /// TTS service facade. `None` only if construction failed.
    pub tts_service: Option<Arc<TtsService>>,
    /// ASR service facade. `None` only if construction failed.
    pub asr_service: Option<Arc<AsrService>>,
    /// Web console service (Live2D + frontend config aggregator).
    pub web_console_service: Option<Arc<WebConsoleService>>,
    /// Workspace root, captured for convenience.
    pub workspace: PathBuf,
    /// Whether the runtime has been started.
    started: bool,
}

impl std::fmt::Debug for AppRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppRuntime")
            .field("workspace", &self.workspace)
            .field("started", &self.started)
            .finish_non_exhaustive()
    }
}

impl AppRuntime {
    /// Creates a new runtime with the given pieces.
    pub fn new(
        context: RuntimeContext,
        coordinator: Option<Arc<ConversationCoordinator>>,
        role_registry: Option<Arc<RoleCardRegistry>>,
        tts_service: Option<Arc<TtsService>>,
        asr_service: Option<Arc<AsrService>>,
    ) -> Self {
        let workspace = context.workspace.clone();
        let web_console_service = if tts_service.is_some() || asr_service.is_some() {
            Some(Arc::new(WebConsoleService::new(
                workspace.clone(),
                tts_service.clone(),
                asr_service.clone(),
            )))
        } else {
            None
        };
        Self {
            context,
            coordinator,
            role_registry,
            tts_service,
            asr_service,
            web_console_service,
            workspace,
            started: false,
        }
    }

    /// Idempotent start hook. Currently a no-op aside from the flag.
    pub async fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
    }

    /// Idempotent stop hook. Currently a no-op aside from the flag.
    pub async fn stop(&mut self) {
        if !self.started {
            return;
        }
        self.started = false;
    }

    /// Returns the workspace root.
    pub fn workspace_path(&self) -> &Path {
        &self.workspace
    }

    /// Returns the session store.
    pub fn session_store(&self) -> &SessionStore {
        &self.context.session_store
    }

    /// Returns the cron service.
    pub fn cron_service(&self) -> &Arc<CronService> {
        &self.context.cron_service
    }

    /// Returns the optional heartbeat service.
    pub fn heartbeat_service(&self) -> Option<&HeartbeatService> {
        self.context.heartbeat_service.as_ref()
    }

    /// Returns the heartbeat file path.
    pub fn heartbeat_file_path(&self) -> &Path {
        &self.context.heartbeat_file_path
    }

    /// Returns the heartbeat interval (seconds).
    pub fn heartbeat_interval_seconds(&self) -> u64 {
        self.context.heartbeat_interval_seconds
    }

    /// Returns the current channel status map.
    ///
    /// v1 stub — always reports an empty map because the channel manager
    /// is not yet wired into `AppRuntime`.
    pub fn channel_status(&self) -> HashMap<String, HashMap<String, bool>> {
        HashMap::new()
    }

    /// Snapshot the runtime's health for the `/api/health` endpoint.
    pub async fn health_snapshot(&self) -> serde_json::Value {
        let workspace = self.workspace.display().to_string();
        let channels = self.channel_status();
        let jobs = match &self.coordinator {
            Some(c) => c.job_counts().await,
            None => HashMap::new(),
        };
        serde_json::json!({
            "status": "ok",
            "workspace": workspace,
            "current_session": self
                .session_store()
                .get_current_session_name()
                .ok()
                .flatten()
                .unwrap_or_else(|| "default".to_string()),
            "current_role": "default",
            "channels": channels,
            "bus": {
                "inbound_size": 0,
                "outbound_size": 0,
            },
            "jobs": jobs,
        })
    }
}
