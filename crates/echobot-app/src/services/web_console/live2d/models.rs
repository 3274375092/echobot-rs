//! Live2D data models — verbatim port of `echobot/app/services/web_console/live2d/models.py`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// A discovered Live2D model under a source root.
#[derive(Debug, Clone)]
pub struct Live2DModelCandidate {
    /// `"workspace"` or `"builtin"`.
    pub source: String,
    /// The absolute root directory for this source.
    pub source_root: PathBuf,
    /// Absolute path to the `.model3.json` file.
    pub model_path: PathBuf,
    /// The runtime directory (= model_path's parent).
    pub runtime_root: PathBuf,
}

impl Live2DModelCandidate {
    /// Relative path of the model json under `source_root`.
    pub fn model_relative_path(&self) -> PathBuf {
        // Save the fallback first so we can borrow self.model_path again
        let fallback: &Path = Path::new(
            self.model_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("."),
        );
        self.model_path
            .strip_prefix(&self.source_root)
            .unwrap_or(fallback)
            .to_path_buf()
    }

    /// Relative path of the runtime dir under `source_root`.
    pub fn runtime_relative_path(&self) -> PathBuf {
        self.runtime_root
            .strip_prefix(&self.source_root)
            .unwrap_or_else(|_| Path::new("."))
            .to_path_buf()
    }

    /// Model name = filename without `.model3.json`.
    pub fn model_name(&self) -> String {
        self.model_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .strip_suffix(".model3.json")
            .unwrap_or("")
            .to_string()
    }
}

/// A single uploaded file (relative path + raw bytes).
#[derive(Debug, Clone)]
pub struct Live2DUploadFile {
    pub relative_path: String,
    pub file_bytes: Vec<u8>,
}

/// Parsed `.vtube.json` config.
#[derive(Debug, Clone)]
pub struct Live2DVTubeConfig {
    pub path: PathBuf,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct Live2DDiscoveredExpression {
    pub name: String,
    pub file: String,
    pub asset_relative_path: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Live2DDiscoveredMotion {
    pub name: String,
    pub file: String,
    pub asset_relative_path: String,
    pub note: String,
    pub group: String,
    pub index: usize,
    pub definition: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Live2DDiscoveredHotkey {
    pub hotkey_key: String,
    pub hotkey_id: String,
    pub name: String,
    pub action: String,
    pub file: String,
    pub shortcut_tokens: Vec<String>,
    pub shortcut_label: String,
    pub target_kind: String,
    pub supported: bool,
}

#[derive(Debug, Clone)]
pub struct Live2DDiscoveredMetadata {
    pub expressions: Vec<Live2DDiscoveredExpression>,
    pub motions: Vec<Live2DDiscoveredMotion>,
    pub hotkeys: Vec<Live2DDiscoveredHotkey>,
    pub annotations_writable: bool,
}
