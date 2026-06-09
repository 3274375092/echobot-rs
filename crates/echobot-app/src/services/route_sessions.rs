//! Route-session store stub.
//!
//! v1: in-memory map of `session_key -> session_name`. Future versions
//! will persist to `route_sessions.json` under the workspace.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct RouteSessionStore {
    inner: Mutex<HashMap<String, String>>,
}

impl RouteSessionStore {
    pub fn new(_path: impl Into<std::path::PathBuf>) -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let g = self.inner.lock().expect("route session store poisoned");
        g.get(key).cloned()
    }

    pub fn set(&self, key: String, session_name: String) {
        let mut g = self.inner.lock().expect("route session store poisoned");
        g.insert(key, session_name);
    }

    pub fn forget(&self, key: &str) {
        let mut g = self.inner.lock().expect("route session store poisoned");
        g.remove(key);
    }
}
