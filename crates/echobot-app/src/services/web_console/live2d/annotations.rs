//! Live2D annotations repository — verbatim port of
//! `echobot/app/services/web_console/live2d/annotations.py`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::constants::LIVE2D_ANNOTATIONS_FILENAME;

type PathLockMap = Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>;

/// Thread-safe repository for Live2D user annotations.
pub struct Live2DAnnotationsRepository {
    filename: String,
    path_locks: PathLockMap,
    guard: Arc<Mutex<()>>,
}

impl Clone for Live2DAnnotationsRepository {
    fn clone(&self) -> Self {
        Self {
            filename: self.filename.clone(),
            path_locks: Arc::clone(&self.path_locks),
            guard: Arc::clone(&self.guard),
        }
    }
}

impl Live2DAnnotationsRepository {
    pub fn new(filename: Option<String>) -> Self {
        Self {
            filename: filename.unwrap_or_else(|| LIVE2D_ANNOTATIONS_FILENAME.to_string()),
            path_locks: Arc::new(Mutex::new(HashMap::new())),
            guard: Arc::new(Mutex::new(())),
        }
    }

    pub fn load(&self, runtime_root: &Path) -> HashMap<String, Value> {
        self.load_from_path(&runtime_root.join(&self.filename))
    }

    pub fn save_annotation(&self, runtime_root: &Path, kind: &str, file: &str, note: &str) {
        let annotations_key = format!("{kind}s");
        let note_owned = note.trim().to_string();
        let file_owned = file.to_string();

        self.update_payload(runtime_root, move |payload| {
            let map = payload
                .entry(annotations_key.clone())
                .or_insert_with(|| json!({}));
            let am = map.as_object_mut().expect("must be object");
            let prev = am
                .get(&file_owned)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if note_owned.is_empty() {
                am.remove(&file_owned);
            } else {
                am.insert(file_owned.clone(), Value::String(note_owned.clone()));
            }
            prev.as_deref().unwrap_or("") != note_owned
        });
    }

    pub fn save_hotkey(
        &self,
        runtime_root: &Path,
        hotkey_key: &str,
        shortcut_tokens: &[String],
        restore_default: bool,
    ) {
        let hk = hotkey_key.to_string();
        let tokens = shortcut_tokens.to_vec();

        self.update_payload(runtime_root, move |payload| {
            let hm = payload
                .entry("hotkeys".to_string())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .expect("must be object");
            if restore_default {
                hm.remove(&hk).is_some()
            } else {
                let next = json!({"shortcut_tokens": tokens});
                let prev = hm.get(&hk).map(|v| v.to_string());
                hm.insert(hk.clone(), next.clone());
                prev != Some(next.to_string())
            }
        });
    }

    // --- private ---

    fn update_payload<F>(&self, runtime_root: &Path, update: F)
    where
        F: FnOnce(&mut HashMap<String, Value>) -> bool,
    {
        let annotations_path = runtime_root.join(&self.filename);
        let lock = self.lock_for(&annotations_path);
        let _guard = lock.lock().expect("path lock poisoned");

        let mut payload = self.load_from_path(&annotations_path);
        if !update(&mut payload) {
            return;
        }
        payload.insert("version".to_string(), json!(1));
        self.write_payload(&annotations_path, &payload);
    }

    fn lock_for(&self, annotations_path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.path_locks.lock().expect("path_locks poisoned");
        locks
            .entry(annotations_path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn empty_payload() -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert("version".to_string(), json!(1));
        m.insert("expressions".to_string(), json!({}));
        m.insert("motions".to_string(), json!({}));
        m.insert("hotkeys".to_string(), json!({}));
        m
    }

    fn load_from_path(&self, annotations_path: &Path) -> HashMap<String, Value> {
        if !annotations_path.exists() {
            return Self::empty_payload();
        }
        let text = match std::fs::read_to_string(annotations_path) {
            Ok(t) => t,
            Err(_) => return Self::empty_payload(),
        };
        let parsed: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return Self::empty_payload(),
        };
        let Some(obj) = parsed.as_object() else {
            return Self::empty_payload();
        };
        let mut payload = Self::empty_payload();
        if let Some(e) = obj.get("expressions").and_then(|v| v.as_object()) {
            payload.insert("expressions".to_string(), Value::Object(e.clone()));
        }
        if let Some(m) = obj.get("motions").and_then(|v| v.as_object()) {
            payload.insert("motions".to_string(), Value::Object(m.clone()));
        }
        if let Some(h) = obj.get("hotkeys").and_then(|v| v.as_object()) {
            payload.insert("hotkeys".to_string(), Value::Object(h.clone()));
        }
        payload
    }

    fn write_payload(&self, annotations_path: &Path, payload: &HashMap<String, Value>) {
        if let Some(parent) = annotations_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let base = annotations_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("live2d");
        let temp_path = annotations_path.with_file_name(format!(
            ".{base}.{}.tmp",
            std::process::id()
        ));
        let text =
            serde_json::to_string_pretty(payload).expect("serialization failed");
        if std::fs::write(&temp_path, text).is_ok() {
            let _ = std::fs::rename(&temp_path, annotations_path);
        }
        let _ = std::fs::remove_file(&temp_path);
    }
}
