//! Session service — thin wrapper over `SessionStore` that exposes the
//! gateway-shaped API used by the Python app: list, load current, switch,
//! create, rename, delete.
//!
//! v1 has no delivery / route-session persistence; that is layered on
//! by the gateway's `GatewaySessionService` in a follow-up.

use std::sync::Arc;

use echobot_runtime::error::Error as RuntimeError;
use echobot_runtime::sessions::{ChatSession, SessionInfo, SessionStore};

use crate::error::AppError;

#[derive(Clone)]
pub struct SessionService {
    store: Arc<SessionStore>,
}

impl SessionService {
    pub fn new(store: Arc<SessionStore>) -> Self {
        Self { store }
    }

    /// Returns a clone of the underlying session-store arc. Exposed so
    /// the HTTP layer can call `set_current_session` / `save_session`
    /// without going through the higher-level session service.
    pub fn store(&self) -> Arc<SessionStore> {
        self.store.clone()
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, AppError> {
        self.store
            .list_sessions()
            .await
            .map_err(runtime_to_app_error)
    }

    pub async fn load_current_session(&self) -> Result<ChatSession, AppError> {
        self.store
            .load_current_session()
            .await
            .map_err(runtime_to_app_error)
    }

    pub async fn load_session(&self, name: &str) -> Result<ChatSession, AppError> {
        self.store
            .load_session(name)
            .await
            .map_err(|e| match e {
                RuntimeError::SessionNotFound { .. } => AppError::NotFound(e.to_string()),
                other => AppError::Internal(other.to_string()),
            })
    }

    pub async fn create_session(
        &self,
        name: Option<&str>,
    ) -> Result<ChatSession, AppError> {
        self.store
            .create_session(name)
            .await
            .map_err(|e| AppError::BadRequest(e.to_string()))
    }

    pub async fn switch_session(&self, name: &str) -> Result<ChatSession, AppError> {
        self.store
            .load_or_create_session(name)
            .await
            .map_err(|e| AppError::NotFound(e.to_string()))
    }

    pub async fn rename_session(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<ChatSession, AppError> {
        self.store
            .rename_session(old_name, new_name)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.to_lowercase().contains("not found") {
                    AppError::NotFound(msg)
                } else {
                    AppError::BadRequest(msg)
                }
            })
    }

    pub async fn delete_session(&self, name: &str) -> Result<bool, AppError> {
        match self.store.delete_session(name).await {
            Ok(()) => Ok(true),
            Err(RuntimeError::SessionNotFound { .. }) => Ok(false),
            Err(e) => Err(AppError::Internal(e.to_string())),
        }
    }
}

fn runtime_to_app_error(e: RuntimeError) -> AppError {
    match e {
        RuntimeError::SessionNotFound { .. } => AppError::NotFound(e.to_string()),
        other => AppError::Internal(other.to_string()),
    }
}
