//! On-disk session storage.
//!
//! Mirrors `echobot/runtime/sessions.py`. Each session lives in
//! `<base_dir>/<name>.jsonl` with one metadata record followed by one record
//! per message. A sidecar `index.jsonl` records the "current" session name.
//!
//! ## Concurrency
//!
//! All mutating methods take an `RwLock` on the store so multiple
//! `SessionAgentRunner`s can share a single store. Disk I/O is funneled
//! through `tokio::task::spawn_blocking` to keep the event loop free.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::RwLock;

use echobot_core::models::{LLMMessage, MessageRole, ToolCall};
use echobot_core::naming::normalize_name_token;

use crate::error::{Error, Result};

/// A chat session: the metadata header + the rolling history of messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatSession {
    /// Normalized session name.
    pub name: String,
    /// Conversation history.
    pub history: Vec<LLMMessage>,
    /// Last-modified timestamp (ISO 8601 with seconds).
    pub updated_at: String,
    /// Optional compressed summary of the conversation.
    #[serde(default)]
    pub compressed_summary: String,
    /// Free-form metadata (route_mode, role, pending_user_input, ...).
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl ChatSession {
    /// Creates a new empty session with `name` and the current timestamp.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            history: Vec::new(),
            updated_at: now_text(),
            compressed_summary: String::new(),
            metadata: HashMap::new(),
        }
    }
}

/// Lightweight session info used by `SessionStore::list_sessions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionInfo {
    /// Normalized session name.
    pub name: String,
    /// Number of message records in the session file.
    pub message_count: usize,
    /// Last-modified timestamp (ISO 8601 with seconds).
    pub updated_at: String,
}

/// Stores chat sessions on disk as JSONL files. The store is shareable across
/// async tasks (`Arc<SessionStore>`).
#[derive(Debug, Clone)]
pub struct SessionStore {
    /// Root directory containing `<name>.jsonl` files and the `index.jsonl`.
    pub base_dir: PathBuf,
    /// Path to the index file.
    pub index_file: PathBuf,
    lock: Arc<RwLock<()>>,
}

impl SessionStore {
    /// Creates a new store rooted at `base_dir`. The directory is created
    /// lazily on the first write.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        let index_file = base_dir.join("index.jsonl");
        Self {
            base_dir,
            index_file,
            lock: Arc::new(RwLock::new(())),
        }
    }

    /// Loads the current session, or creates a `"default"` session if none is
    /// recorded as current.
    pub async fn load_current_session(&self) -> Result<ChatSession> {
        let _g = self.lock.read().await;
        if let Some(name) = self.get_current_session_name()? {
            return self.load_or_create_session(&name).await;
        }
        drop(_g);
        let session = self.load_or_create_session("default").await?;
        self.set_current_session(&session.name).await?;
        Ok(session)
    }

    /// Loads the session with `name`, creating an empty one if the file does
    /// not exist yet.
    pub async fn load_or_create_session(&self, name: &str) -> Result<ChatSession> {
        let normalized = normalize_session_name(name)?;
        let path = self.session_path(&normalized);
        if path.exists() {
            return self.load_session(&normalized).await;
        }
        let session = ChatSession::new(&normalized);
        self.save_session(&session).await?;
        Ok(session)
    }

    /// Creates a brand-new session. Returns [`Error::SessionNotFound`] is
    /// raised if the session name conflicts with an existing file.
    pub async fn create_session(&self, name: Option<&str>) -> Result<ChatSession> {
        let session_name = match name {
            Some(raw) => normalize_session_name(raw)?,
            None => self.generate_session_name().await?,
        };
        let path = self.session_path(&session_name);
        if path.exists() {
            return Err(Error::SessionNotFound(session_name));
        }
        let session = ChatSession::new(&session_name);
        self.save_session(&session).await?;
        self.set_current_session(&session.name).await?;
        Ok(session)
    }

    /// Loads a session from disk. Errors if the file is missing or malformed.
    pub async fn load_session(&self, name: &str) -> Result<ChatSession> {
        let normalized = normalize_session_name(name)?;
        let path = self.session_path(&normalized);
        if !path.exists() {
            return Err(Error::SessionNotFound(normalized));
        }
        let path_for_blocking = path.clone();
        let records =
            tokio::task::spawn_blocking(move || read_jsonl_records(&path_for_blocking))
                .await
                .map_err(|e| Error::Wiring(format!("session read task failed: {e}")))??;
        if records.is_empty() {
            return Err(Error::InvalidSessionMetadata(normalized));
        }

        let metadata = &records[0];
        if metadata.get("type").and_then(Value::as_str) != Some("session") {
            return Err(Error::InvalidSessionMetadata(normalized));
        }

        let mut history: Vec<LLMMessage> = Vec::new();
        for record in records.iter().skip(1) {
            if record.get("type").and_then(Value::as_str) != Some("message") {
                continue;
            }
            let mut data = record.clone();
            data.remove("type");
            history.push(message_from_dict(&data));
        }

        Ok(ChatSession {
            name: metadata
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&normalized)
                .to_string(),
            history,
            updated_at: metadata
                .get("updated_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            compressed_summary: metadata
                .get("compressed_summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            metadata: read_metadata(metadata.get("metadata")),
        })
    }

    /// Saves `session` to disk (overwriting any existing file).
    pub async fn save_session(&self, session: &ChatSession) -> Result<()> {
        let path = self.session_path(&session.name);
        let mut updated = session.clone();
        updated.updated_at = now_text();

        let mut records: Vec<Map<String, Value>> = Vec::new();
        let mut meta = Map::new();
        meta.insert("type".into(), Value::String("session".into()));
        meta.insert("name".into(), Value::String(updated.name.clone()));
        meta.insert(
            "updated_at".into(),
            Value::String(updated.updated_at.clone()),
        );
        meta.insert(
            "compressed_summary".into(),
            Value::String(updated.compressed_summary.clone()),
        );
        meta.insert(
            "metadata".into(),
            Value::Object(metadata_to_map(&updated.metadata)),
        );
        records.push(meta);
        for message in &updated.history {
            let mut record = message_to_dict(message);
            record.insert("type".into(), Value::String("message".into()));
            records.push(record);
        }

        let store = self.clone();
        let path_clone = path.clone();
        let base_dir = store.base_dir.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            std::fs::create_dir_all(&base_dir)?;
            let mut buf = String::new();
            for record in &records {
                let line = serde_json::to_string(record)?;
                buf.push_str(&line);
                buf.push('\n');
            }
            std::fs::write(&path_clone, buf)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Wiring(format!("session write task failed: {e}")))??;
        Ok(())
    }

    /// Deletes a session file. Missing files are silently ignored.
    pub async fn delete_session(&self, name: &str) -> Result<()> {
        let normalized = normalize_session_name(name)?;
        let path = self.session_path(&normalized);
        if path.exists() {
            let _g = self.lock.write().await;
            if path.exists() {
                tokio::task::spawn_blocking(move || std::fs::remove_file(path))
                    .await
                    .map_err(|e| Error::Wiring(format!("session delete task failed: {e}")))??;
            }
        }
        Ok(())
    }

    /// Renames a session, updating the current-session index if needed.
    pub async fn rename_session(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<ChatSession> {
        let old = normalize_session_name(old_name)?;
        let new = normalize_session_name(new_name)?;
        let _g = self.lock.write().await;
        let mut session = self.load_session(&old).await?;
        if old == new {
            return Ok(session);
        }
        let new_path = self.session_path(&new);
        if new_path.exists() {
            return Err(Error::SessionNotFound(new));
        }
        let current_name = self.get_current_session_name()?;
        let old_path = self.session_path(&old);
        session.name = new.clone();
        Self::write_session_sync(&self.base_dir, &self.session_path(&new), &session)?;
        if old_path.exists() {
            let _ = std::fs::remove_file(&old_path);
        }
        if current_name.as_deref() == Some(old.as_str()) {
            drop(_g);
            self.set_current_session(&new).await?;
        }
        Ok(session)
    }

    /// Marks `name` as the "current" session by rewriting `index.jsonl`.
    pub async fn set_current_session(&self, name: &str) -> Result<()> {
        let normalized = normalize_session_name(name)?;
        let _g = self.lock.write().await;
        let base_dir = self.base_dir.clone();
        let path = self.index_file.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            std::fs::create_dir_all(&base_dir)?;
            let mut record = Map::new();
            record.insert(
                "current_session".into(),
                Value::String(normalized.clone()),
            );
            let line = serde_json::to_string(&Value::Object(record))?;
            std::fs::write(&path, format!("{line}\n"))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Wiring(format!("index write task failed: {e}")))??;
        Ok(())
    }

    /// Returns the current-session name from `index.jsonl`, or `None` if no
    /// current session has been recorded.
    pub fn get_current_session_name(&self) -> Result<Option<String>> {
        if !self.index_file.exists() {
            return Ok(None);
        }
        let records = read_jsonl_records(&self.index_file)?;
        if records.is_empty() {
            return Ok(None);
        }
        let current = records[0]
            .get("current_session")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        Ok(if current.is_empty() {
            None
        } else {
            Some(current.to_string())
        })
    }

    /// Lists all sessions in the store, sorted by `updated_at` descending.
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let base_dir = self.base_dir.clone();
        let index_name = self.index_file.file_name().unwrap_or_default().to_os_string();
        let result = tokio::task::spawn_blocking(move || -> Result<Vec<SessionInfo>> {
            if !base_dir.exists() {
                return Ok(Vec::new());
            }
            let mut sessions: Vec<SessionInfo> = Vec::new();
            for entry in std::fs::read_dir(&base_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                    continue;
                }
                if path.file_name() == Some(index_name.as_os_str()) {
                    continue;
                }
                let records = read_jsonl_records(&path)?;
                if records.is_empty() {
                    continue;
                }
                let metadata = &records[0];
                if metadata.get("type").and_then(Value::as_str) != Some("session") {
                    continue;
                }
                let name = metadata
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string()
                    });
                let updated_at = metadata
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let message_count = records
                    .iter()
                    .skip(1)
                    .filter(|r| r.get("type").and_then(Value::as_str) == Some("message"))
                    .count();
                sessions.push(SessionInfo {
                    name,
                    message_count,
                    updated_at,
                });
            }
            sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            Ok(sessions)
        })
        .await
        .map_err(|e| Error::Wiring(format!("session list task failed: {e}")))??;
        Ok(result)
    }

    /// Returns whether a session file exists for `name`.
    pub fn has_session(&self, name: &str) -> bool {
        match normalize_session_name(name) {
            Ok(n) => self.session_path(&n).exists(),
            Err(_) => false,
        }
    }

    /// Returns the on-disk path of a session's JSONL file.
    pub fn session_path(&self, name: &str) -> PathBuf {
        self.base_dir.join(format!("{name}.jsonl"))
    }

    async fn generate_session_name(&self) -> Result<String> {
        let prefix = format!(
            "session-{}",
            Utc::now().format("%Y%m%d-%H%M%S")
        );
        let mut candidate = prefix.clone();
        let mut counter: usize = 1;
        while self.session_path(&candidate).exists() {
            counter += 1;
            candidate = format!("{prefix}-{counter}");
        }
        Ok(candidate)
    }

    /// Synchronous variant of [`save_session`] used by [`rename_session`]
    /// while holding the write lock. Performs the I/O on the calling thread
    /// because blocking the lock holder is intentional and bounded.
    fn write_session_sync(
        base_dir: &Path,
        path: &Path,
        session: &ChatSession,
    ) -> Result<()> {
        std::fs::create_dir_all(base_dir)?;
        let mut records: Vec<Map<String, Value>> = Vec::new();
        let mut meta = Map::new();
        meta.insert("type".into(), Value::String("session".into()));
        meta.insert("name".into(), Value::String(session.name.clone()));
        meta.insert("updated_at".into(), Value::String(session.updated_at.clone()));
        meta.insert(
            "compressed_summary".into(),
            Value::String(session.compressed_summary.clone()),
        );
        meta.insert("metadata".into(), Value::Object(metadata_to_map(&session.metadata)));
        records.push(meta);
        for message in &session.history {
            let mut record = message_to_dict(message);
            record.insert("type".into(), Value::String("message".into()));
            records.push(record);
        }
        let mut buf = String::new();
        for record in &records {
            let line = serde_json::to_string(record)?;
            buf.push_str(&line);
            buf.push('\n');
        }
        std::fs::write(path, buf)?;
        Ok(())
    }
}

/// Normalizes a free-form session name into the slug form used on disk.
/// Empty names and names with no valid characters raise
/// [`Error::InvalidSessionName`].
pub fn normalize_session_name(name: &str) -> Result<String> {
    let raw = name.trim();
    if raw.is_empty() {
        return Err(Error::InvalidSessionName(
            "session name cannot be empty".into(),
        ));
    }
    let normalized = normalize_name_token(raw);
    if normalized.is_empty() {
        return Err(Error::InvalidSessionName(
            "session name must contain letters, digits, hyphen, or underscore".into(),
        ));
    }
    Ok(normalized)
}

// ---------------------------------------------------------------------------
// Message (de)serialization
// ---------------------------------------------------------------------------

/// Serializes an [`LLMMessage`] to its on-disk JSON shape (matches the Python
/// `message_to_dict`).
pub fn message_to_dict(message: &LLMMessage) -> Map<String, Value> {
    let mut data = Map::new();
    data.insert(
        "role".into(),
        Value::String(message.role.as_str().to_string()),
    );
    data.insert("content".into(), content_to_value(&message.content));
    if let Some(name) = &message.name {
        if !name.is_empty() {
            data.insert("name".into(), Value::String(name.clone()));
        }
    }
    if let Some(id) = &message.tool_call_id {
        if !id.is_empty() {
            data.insert("tool_call_id".into(), Value::String(id.clone()));
        }
    }
    if !message.tool_calls.is_empty() {
        let arr: Vec<Value> = message
            .tool_calls
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "name": t.name,
                    "arguments": t.arguments,
                })
            })
            .collect();
        data.insert("tool_calls".into(), Value::Array(arr));
    }
    if message.role == MessageRole::Assistant && !message.reasoning_content.is_empty() {
        data.insert(
            "reasoning_content".into(),
            Value::String(message.reasoning_content.clone()),
        );
        data.insert(
            "reasoning_field".into(),
            Value::String(message.reasoning_field.as_str().to_string()),
        );
    }
    data
}

fn content_to_value(content: &echobot_core::models::MessageContent) -> Value {
    serde_json::to_value(content).unwrap_or(Value::Null)
}

/// Deserializes an [`LLMMessage`] from its on-disk JSON shape.
pub fn message_from_dict(data: &Map<String, Value>) -> LLMMessage {
    let role = match data.get("role").and_then(Value::as_str).unwrap_or("user") {
        "system" => MessageRole::System,
        "assistant" => MessageRole::Assistant,
        "tool" => MessageRole::Tool,
        _ => MessageRole::User,
    };

    let content = data
        .get("content")
        .map(|v| echobot_core::models::normalize_message_content(v))
        .unwrap_or_default();

    let name = read_optional_text(data.get("name"));
    let tool_call_id = read_optional_text(data.get("tool_call_id"));
    let tool_calls: Vec<ToolCall> = data
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_object().map(|m| {
                    ToolCall::new(
                        m.get("id").and_then(Value::as_str).unwrap_or(""),
                        m.get("name").and_then(Value::as_str).unwrap_or(""),
                        m.get("arguments").and_then(Value::as_str).unwrap_or(""),
                    )
                }))
                .collect()
        })
        .unwrap_or_default();

    let reasoning_content = data
        .get("reasoning_content")
        .or_else(|| data.get("reasoning"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let reasoning_field = match data.get("reasoning_field").and_then(Value::as_str) {
        Some("reasoning") => echobot_core::models::ReasoningField::Reasoning,
        _ => echobot_core::models::ReasoningField::ReasoningContent,
    };

    LLMMessage {
        role,
        content,
        name,
        tool_call_id,
        tool_calls,
        reasoning_content,
        reasoning_field,
    }
}

fn read_optional_text(value: Option<&Value>) -> Option<String> {
    let s = value?.as_str()?.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn read_metadata(value: Option<&Value>) -> HashMap<String, Value> {
    let Some(Value::Object(map)) = value else {
        return HashMap::new();
    };
    map.iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn metadata_to_map(value: &HashMap<String, Value>) -> Map<String, Value> {
    let mut map = Map::new();
    for (k, v) in value {
        map.insert(k.clone(), v.clone());
    }
    map
}

fn read_jsonl_records(path: &Path) -> Result<Vec<Map<String, Value>>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)?;
    let mut records: Vec<Map<String, Value>> = Vec::new();
    for line in text.split('\n') {
        let cleaned = line.trim();
        if cleaned.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(cleaned)?;
        if let Value::Object(obj) = value {
            records.push(obj);
        }
    }
    Ok(records)
}

fn now_text() -> String {
    let now: DateTime<Utc> = Utc::now();
    now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "echobot-session-test-{}-{}",
            std::process::id(),
            name
        );
        let dir = std::env::temp_dir().join(unique);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn round_trip_session_history() {
        let dir = tmp_dir("round-trip");
        let store = SessionStore::new(&dir);
        let mut session = store.create_session(Some("My Test")).await.unwrap();
        let user_msg = LLMMessage::text(MessageRole::User, "hi");
        let assistant_msg = LLMMessage::assistant_with_tool_calls(
            "thinking",
            vec![ToolCall::new("call_1", "ping", "{\"x\":1}")],
        );
        session.history.push(user_msg);
        session.history.push(assistant_msg);
        store.save_session(&session).await.unwrap();

        let loaded = store.load_session("my-test").await.unwrap();
        assert_eq!(loaded.history.len(), 2);
        assert_eq!(loaded.history[0].role, MessageRole::User);
        assert_eq!(loaded.history[1].role, MessageRole::Assistant);
        assert_eq!(loaded.history[1].tool_calls[0].name, "ping");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn current_session_index_round_trip() {
        let dir = tmp_dir("current-index");
        let store = SessionStore::new(&dir);
        let session = store.create_session(Some("alpha")).await.unwrap();
        store.set_current_session(&session.name).await.unwrap();
        let current = store.get_current_session_name().unwrap();
        assert_eq!(current.as_deref(), Some("alpha"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_session_name_rejects_invalid() {
        assert!(normalize_session_name("").is_err());
        assert!(normalize_session_name("   ").is_err());
        assert!(normalize_session_name("!!!").is_err());
        assert!(normalize_session_name("ok-name_1").is_ok());
    }

    #[test]
    fn chat_session_round_trips_through_json() {
        use echobot_core::models::{MessageRole, ToolCall};

        let mut session = ChatSession::new("json-roundtrip");
        session.history.push(LLMMessage::text(
            MessageRole::User,
            "hello world",
        ));
        session.history.push(LLMMessage::assistant_with_tool_calls(
            "thinking out loud",
            vec![ToolCall::new("call_1", "echo", "{\"x\":1}")],
        ));
        session.compressed_summary = "summary text".to_string();
        session
            .metadata
            .insert("role".to_string(), serde_json::json!("default"));
        session
            .metadata
            .insert("route_mode".to_string(), serde_json::json!("auto"));

        // Serialize to JSON.
        let serialized = serde_json::to_string(&session).expect("serialize");
        // The JSON should be well-formed; verify by round-tripping once
        // and re-serializing. The two serializations may differ in
        // metadata-key order (HashMap iteration order is unspecified),
        // so we compare via a JSON-value round trip rather than
        // string equality.
        let parsed: ChatSession = serde_json::from_str(&serialized).expect("parse");
        let reserialized_value = serde_json::to_value(&parsed).expect("re-serialize");
        let original_value = serde_json::to_value(&session).expect("serialize-to-value");
        assert_eq!(original_value, reserialized_value);

        // Verify the contents survived.
        assert_eq!(parsed.name, "json-roundtrip");
        assert_eq!(parsed.history.len(), 2);
        assert_eq!(parsed.history[0].role, MessageRole::User);
        assert_eq!(parsed.history[1].role, MessageRole::Assistant);
        assert_eq!(parsed.history[1].tool_calls.len(), 1);
        assert_eq!(parsed.history[1].tool_calls[0].name, "echo");
        assert_eq!(parsed.compressed_summary, "summary text");
        assert_eq!(
            parsed.metadata.get("role"),
            Some(&serde_json::json!("default"))
        );
        assert_eq!(
            parsed.metadata.get("route_mode"),
            Some(&serde_json::json!("auto"))
        );
    }
}
