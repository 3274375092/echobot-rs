//! Shared application state.

use std::sync::Arc;

use axum::extract::FromRef;

use crate::runtime::AppRuntime;

/// State held by every axum handler via `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    runtime: Arc<AppRuntime>,
}

impl AppState {
    /// Wraps an `AppRuntime` in shared state.
    pub fn new(runtime: Arc<AppRuntime>) -> Self {
        Self { runtime }
    }

    /// Returns a handle to the runtime.
    pub fn runtime(&self) -> Arc<AppRuntime> {
        self.runtime.clone()
    }
}

impl FromRef<AppState> for Arc<AppRuntime> {
    fn from_ref(state: &AppState) -> Self {
        state.runtime.clone()
    }
}
