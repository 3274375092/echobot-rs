//! Delivery store stub.
//!
//! v1: the gateway `DeliveryStore` is an in-memory map of
//! `delivery_id -> payload`. Future versions will persist it to
//! `delivery.json` under the workspace.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct DeliveryStore {
    inner: Mutex<HashMap<String, serde_json::Value>>,
}

impl DeliveryStore {
    pub fn new(_path: impl Into<std::path::PathBuf>) -> Self {
        Self::default()
    }

    pub fn record(&self, delivery_id: String, payload: serde_json::Value) {
        let mut g = self.inner.lock().expect("delivery store poisoned");
        g.insert(delivery_id, payload);
    }

    pub fn get(&self, delivery_id: &str) -> Option<serde_json::Value> {
        let g = self.inner.lock().expect("delivery store poisoned");
        g.get(delivery_id).cloned()
    }

    pub fn len(&self) -> usize {
        let g = self.inner.lock().expect("delivery store poisoned");
        g.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
