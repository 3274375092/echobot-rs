//! Per-run agent trace storage.
//!
//! Mirrors `echobot/runtime/agent_traces.py`. Each agent run writes a stream
//! of JSONL records into
//! `<base_dir>/<session_name>/<run_id>.jsonl`. The trace callback used by
//! `SessionAgentRunner` calls [`AgentTraceStore::append_event`] under the
//! hood.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::error::Result;
use crate::sessions::normalize_session_name;

/// Per-run agent trace storage.
#[derive(Debug, Clone)]
pub struct AgentTraceStore {
    /// Root directory containing `<session_name>/<run_id>.jsonl` files.
    pub base_dir: PathBuf,
}

impl AgentTraceStore {
    /// Creates a new trace store rooted at `base_dir`.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Generates a unique run id: `<YYYYMMDD-HHMMSS>-<8-hex>`.
    pub fn create_run_id(&self) -> String {
        let timestamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let suffix: String = {
            let bytes = uuid::Uuid::new_v4();
            let mut out = String::with_capacity(8);
            for byte in bytes.as_bytes().iter().take(4) {
                use std::fmt::Write;
                let _ = write!(&mut out, "{byte:02x}");
            }
            out
        };
        format!("{timestamp}-{suffix}")
    }

    /// Returns the on-disk path of a trace file.
    pub fn trace_path(&self, session_name: &str, run_id: &str) -> PathBuf {
        let normalized = normalize_session_name(session_name)
            .unwrap_or_else(|_| session_name.to_string());
        self.base_dir.join(normalized).join(format!("{run_id}.jsonl"))
    }

    /// Appends an event record to a trace file. The `data` map is merged into
    /// the base record under keys like `event`, `session_name`, `run_id`,
    /// `created_at`.
    pub fn append_event(
        &self,
        session_name: &str,
        run_id: &str,
        event: &str,
        data: Option<Map<String, Value>>,
    ) -> Result<PathBuf> {
        let path = self.trace_path(session_name, run_id);
        let session_normalized = normalize_session_name(session_name)
            .unwrap_or_else(|_| session_name.to_string());
        let mut record = Map::new();
        record.insert("event".into(), Value::String(event.to_string()));
        record.insert(
            "session_name".into(),
            Value::String(session_normalized),
        );
        record.insert("run_id".into(), Value::String(run_id.to_string()));
        record.insert("created_at".into(), Value::String(now_text()));
        if let Some(data) = data {
            for (k, v) in data {
                record.insert(k, v);
            }
        }
        write_record_blocking(&path, &record)
    }

    /// Reads every record from a trace file, skipping blank / invalid lines.
    pub fn read_events(
        &self,
        session_name: &str,
        run_id: &str,
    ) -> Result<Vec<Map<String, Value>>> {
        let path = self.trace_path(session_name, run_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&path)?;
        let mut events: Vec<Map<String, Value>> = Vec::new();
        for line in text.split('\n') {
            let cleaned = line.trim();
            if cleaned.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(cleaned) {
                Ok(Value::Object(obj)) => events.push(obj),
                _ => continue,
            }
        }
        Ok(events)
    }
}

fn write_record_blocking(path: &Path, record: &Map<String, Value>) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(record)?;
    use std::io::Write;
    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    handle.write_all(line.as_bytes())?;
    handle.write_all(b"\n")?;
    Ok(path.to_path_buf())
}

fn now_text() -> String {
    let now: DateTime<Utc> = Utc::now();
    now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> PathBuf {
        let unique = format!(
            "echobot-trace-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let dir = std::env::temp_dir().join(unique);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn append_and_read_events_round_trip() {
        let dir = tmp_dir();
        let store = AgentTraceStore::new(&dir);
        let run_id = store.create_run_id();
        let mut data = Map::new();
        data.insert("step".into(), Value::from(1u64));
        store
            .append_event("alpha", &run_id, "turn_started", Some(data.clone()))
            .unwrap();
        store
            .append_event("alpha", &run_id, "turn_completed", None)
            .unwrap();
        let events = store.read_events("alpha", &run_id).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].get("event").unwrap().as_str(), Some("turn_started"));
        assert_eq!(events[1].get("event").unwrap().as_str(), Some("turn_completed"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
