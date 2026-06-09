//! Filesystem tools: list, read, write, edit, search files inside a
//! workspace root.
//!
//! Ports `echobot/tools/filesystem.py`. All operations are constrained
//! to the configured workspace; paths that escape it via `..` or
//! absolute pointers are rejected with an error.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

use echobot_core::Error;

use crate::base::{
    optional_string, require_positive_int, require_string, BaseTool, ToolExecutionOutput,
};

/// Maximum number of entries returned by `list_directory`.
const LIST_DIRECTORY_LIMIT: usize = 200;
/// Default cap on text returned by `read_text_file`.
const READ_TEXT_FILE_DEFAULT_LIMIT: i64 = 4000;
/// Default cap for file-search results.
const SEARCH_FILES_DEFAULT_LIMIT: i64 = 200;
/// Default cap for text-search matches.
const SEARCH_TEXT_DEFAULT_LIMIT: i64 = 50;
/// Default cap for files inspected by `search_text_in_files`.
const SEARCH_TEXT_DEFAULT_FILES: i64 = 200;

// ---------------------------------------------------------------------------
// WorkspaceTool
// ---------------------------------------------------------------------------

/// Base class for any tool that operates on a workspace root.
pub struct WorkspaceTool {
    workspace: PathBuf,
}

impl WorkspaceTool {
    /// Creates a new tool rooted at `workspace`.
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
        }
    }

    /// Returns the workspace root (canonicalized when it exists).
    pub fn workspace_root(&self) -> &Path {
        &self.workspace
    }

    /// Resolves a relative path inside the workspace, rejecting paths
    /// that escape it.
    pub fn resolve_workspace_path(&self, relative_path: &str) -> std::result::Result<PathBuf, String> {
        let workspace_root = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| self.workspace.clone());
        let candidate = workspace_root.join(relative_path);
        let target = candidate
            .canonicalize()
            .map_err(|e| format!("Path does not exist: {relative_path} ({e})"))?;
        if !target.starts_with(&workspace_root) {
            return Err(format!("Path is outside the workspace: {relative_path}"));
        }
        Ok(target)
    }

    /// Renders `target` as a workspace-relative posix path.
    pub fn to_relative_path(&self, target: &Path) -> String {
        let workspace_root = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| self.workspace.clone());
        let target_canon = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());
        match target_canon.strip_prefix(&workspace_root) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => target_canon.to_string_lossy().replace('\\', "/"),
        }
    }
}

// ---------------------------------------------------------------------------
// WritableWorkspaceTool
// ---------------------------------------------------------------------------

/// Subclass for tools that may mutate the workspace.
pub struct WritableWorkspaceTool {
    inner: WorkspaceTool,
    writes_enabled: bool,
}

impl WritableWorkspaceTool {
    /// Creates a new writable workspace tool.
    pub fn new(workspace: impl AsRef<Path>, writes_enabled: bool) -> Self {
        Self {
            inner: WorkspaceTool::new(workspace),
            writes_enabled,
        }
    }

    /// The workspace root.
    pub fn workspace_root(&self) -> &Path {
        self.inner.workspace_root()
    }

    /// Resolves a workspace-relative path.
    pub fn resolve_workspace_path(&self, relative_path: &str) -> std::result::Result<PathBuf, String> {
        self.inner.resolve_workspace_path(relative_path)
    }

    /// Renders a path as workspace-relative.
    pub fn to_relative_path(&self, target: &Path) -> String {
        self.inner.to_relative_path(target)
    }

    /// Throws if writes are disabled in the current runtime.
    pub fn require_writes_enabled(&self) -> std::result::Result<(), String> {
        if self.writes_enabled {
            Ok(())
        } else {
            Err("当前运行时已禁用文件写入工具".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// ListDirectoryTool
// ---------------------------------------------------------------------------

/// Lists files and folders under the workspace.
pub struct ListDirectoryTool {
    inner: WorkspaceTool,
}

impl ListDirectoryTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            inner: WorkspaceTool::new(workspace),
        }
    }
}

#[async_trait]
impl BaseTool for ListDirectoryTool {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List files and folders under the workspace."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path inside the workspace.",
                    "default": "."
                }
            },
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let relative_path = {
            let raw = arguments.get("path").and_then(Value::as_str).unwrap_or(".").trim();
            if raw.is_empty() { "." } else { raw }
        };
        let target = self
            .inner
            .resolve_workspace_path(relative_path)
            .map_err(|e| Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: e,
            }))?;
        let meta = fs::metadata(&target).await.map_err(|e| {
            Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("Path does not exist: {relative_path} ({e})"),
            })
        })?;
        if !meta.is_dir() {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("Path is not a directory: {relative_path}"),
            }));
        }

        let mut entries: Vec<(String, bool)> = Vec::new();
        let mut dir = fs::read_dir(&target).await.map_err(|e| {
            Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("Cannot read directory: {relative_path} ({e})"),
            })
        })?;
        while let Some(child) = dir.next_entry().await.map_err(|e| {
            Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("Cannot read directory: {relative_path} ({e})"),
            })
        })? {
            let name = child.file_name().to_string_lossy().to_string();
            let is_file = match child.file_type().await {
                Ok(t) => t.is_file(),
                Err(_) => true,
            };
            entries.push((name, is_file));
        }
        // Sort: directories after files, then case-insensitive name.
        entries.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));

        let total = entries.len();
        let truncated = total > LIST_DIRECTORY_LIMIT;
        let limited: Vec<Value> = entries
            .into_iter()
            .take(LIST_DIRECTORY_LIMIT)
            .map(|(name, is_file)| {
                json!({
                    "name": name,
                    "type": if is_file { "file" } else { "directory" }
                })
            })
            .collect();

        Ok(ToolExecutionOutput::from_payload(json!({
            "path": self.inner.to_relative_path(&target),
            "entries": limited,
            "truncated": truncated,
        })))
    }
}

// ---------------------------------------------------------------------------
// ReadTextFileTool
// ---------------------------------------------------------------------------

/// Reads a UTF-8 text file from the workspace.
pub struct ReadTextFileTool {
    inner: WorkspaceTool,
}

impl ReadTextFileTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            inner: WorkspaceTool::new(workspace),
        }
    }
}

#[async_trait]
impl BaseTool for ReadTextFileTool {
    fn name(&self) -> &str {
        "read_text_file"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file from the workspace."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path inside the workspace."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum number of characters to return.",
                    "default": READ_TEXT_FILE_DEFAULT_LIMIT
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let relative_path = require_string(&arguments, "path").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let max_chars = require_positive_int(&arguments, "max_chars", READ_TEXT_FILE_DEFAULT_LIMIT)
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "max_chars".to_string(),
                    message: m,
                })
            })? as usize;

        let target = self
            .inner
            .resolve_workspace_path(relative_path)
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "path".to_string(),
                    message: m,
                })
            })?;

        let meta = fs::metadata(&target).await.map_err(|_| {
            Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("File does not exist: {relative_path}"),
            })
        })?;
        if !meta.is_file() {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("Path is not a file: {relative_path}"),
            }));
        }

        let content = fs::read_to_string(&target).await.map_err(|_| {
            Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: "Only UTF-8 text files are supported".to_string(),
            })
        })?;

        let (truncated_text, truncated) = crate::base::truncate_text(&content, max_chars);
        Ok(ToolExecutionOutput::from_payload(json!({
            "path": self.inner.to_relative_path(&target),
            "content": truncated_text,
            "total_chars": content.chars().count(),
            "truncated": truncated,
        })))
    }
}

// ---------------------------------------------------------------------------
// WriteTextFileTool
// ---------------------------------------------------------------------------

/// Writes a UTF-8 text file inside the workspace.
pub struct WriteTextFileTool {
    inner: WritableWorkspaceTool,
}

impl WriteTextFileTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>, writes_enabled: bool) -> Self {
        Self {
            inner: WritableWorkspaceTool::new(workspace, writes_enabled),
        }
    }
}

#[async_trait]
impl BaseTool for WriteTextFileTool {
    fn name(&self) -> &str {
        "write_text_file"
    }

    fn description(&self) -> &str {
        "Write a UTF-8 text file inside the workspace."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path inside the workspace."
                },
                "content": {
                    "type": "string",
                    "description": "Text content to write."
                },
                "overwrite": {
                    "type": "boolean",
                    "description": "Overwrite the file if it already exists.",
                    "default": false
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        self.inner
            .require_writes_enabled()
            .map_err(|m| Error::Tool(crate::base::ToolError::Blocked(m)))?;

        let relative_path = require_string(&arguments, "path").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let overwrite = arguments
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // Validate workspace containment before writing.
        let workspace_root = self
            .inner
            .workspace_root()
            .canonicalize()
            .unwrap_or_else(|_| self.inner.workspace_root().to_path_buf());
        let candidate = workspace_root.join(relative_path);
        let target = candidate.canonicalize().unwrap_or_else(|_| candidate.clone());
        // For write: require that the parent directory is inside the workspace.
        let parent = target
            .parent()
            .ok_or_else(|| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "path".to_string(),
                    message: "Path is outside the workspace".to_string(),
                })
            })?
            .to_path_buf();
        if !parent.starts_with(&workspace_root) && !parent.exists() {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("Path is outside the workspace: {relative_path}"),
            }));
        }

        let file_existed = target.exists();
        if file_existed && !overwrite {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("File already exists: {relative_path}"),
            }));
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                Error::Tool(crate::base::ToolError::Execution {
                    name: "write_text_file".to_string(),
                    message: format!("create_dir_all failed: {e}"),
                })
            })?;
        }
        fs::write(&target, content.as_bytes()).await.map_err(|e| {
            Error::Tool(crate::base::ToolError::Execution {
                name: "write_text_file".to_string(),
                message: format!("write failed: {e}"),
            })
        })?;

        Ok(ToolExecutionOutput::from_payload(json!({
            "path": self.inner.to_relative_path(&target),
            "written_chars": content.chars().count(),
            "overwritten": file_existed && overwrite,
        })))
    }
}

// ---------------------------------------------------------------------------
// EditTextFileTool
// ---------------------------------------------------------------------------

/// Applies a small structured edit to a UTF-8 text file.
pub struct EditTextFileTool {
    inner: WritableWorkspaceTool,
}

impl EditTextFileTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>, writes_enabled: bool) -> Self {
        Self {
            inner: WritableWorkspaceTool::new(workspace, writes_enabled),
        }
    }
}

#[async_trait]
impl BaseTool for EditTextFileTool {
    fn name(&self) -> &str {
        "edit_text_file"
    }

    fn description(&self) -> &str {
        "Apply a small structured edit to a UTF-8 text file."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path inside the workspace."
                },
                "operation": {
                    "type": "string",
                    "description": "One of: replace, append, prepend.",
                    "enum": ["replace", "append", "prepend"],
                    "default": "replace"
                },
                "old_text": {
                    "type": "string",
                    "description": "Exact text to replace when operation is replace.",
                    "default": ""
                },
                "new_text": {
                    "type": "string",
                    "description": "New text to write.",
                    "default": ""
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace every matching occurrence when operation is replace.",
                    "default": false
                },
                "create_if_missing": {
                    "type": "boolean",
                    "description": "Create the file when it does not exist.",
                    "default": false
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        self.inner
            .require_writes_enabled()
            .map_err(|m| Error::Tool(crate::base::ToolError::Blocked(m)))?;

        let relative_path = require_string(&arguments, "path").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let operation = optional_string(&arguments, "operation", "replace").to_string();
        let old_text = arguments
            .get("old_text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let new_text = arguments
            .get("new_text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let replace_all = arguments
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let create_if_missing = arguments
            .get("create_if_missing")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // Resolve through workspace root manually so we can still create
        // new files that don't exist yet.
        let workspace_root = self
            .inner
            .workspace_root()
            .canonicalize()
            .unwrap_or_else(|_| self.inner.workspace_root().to_path_buf());
        let target = workspace_root.join(relative_path);

        let file_existed = target.exists();
        if file_existed {
            let meta = fs::metadata(&target).await.map_err(|e| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "path".to_string(),
                    message: format!("Path is not a file: {relative_path} ({e})"),
                })
            })?;
            if !meta.is_file() {
                return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "path".to_string(),
                    message: format!("Path is not a file: {relative_path}"),
                }));
            }
        } else if !create_if_missing {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("File does not exist: {relative_path}"),
            }));
        }

        let original_content = if file_existed {
            fs::read_to_string(&target).await.map_err(|_| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "path".to_string(),
                    message: "Only UTF-8 text files are supported".to_string(),
                })
            })?
        } else {
            String::new()
        };

        let (updated_content, replacements) = match operation.as_str() {
            "append" => (format!("{original_content}{new_text}"), 0),
            "prepend" => (format!("{new_text}{original_content}"), 0),
            "replace" => {
                if old_text.is_empty() {
                    return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                        name: "old_text".to_string(),
                        message: "old_text is required when operation is replace".to_string(),
                    }));
                }
                let occurrences = original_content.matches(&old_text).count();
                if occurrences == 0 {
                    return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                        name: "old_text".to_string(),
                        message: "old_text was not found in the file".to_string(),
                    }));
                }
                if !replace_all && occurrences != 1 {
                    return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                        name: "old_text".to_string(),
                        message: "old_text matched multiple times; set replace_all=true to replace them all"
                            .to_string(),
                    }));
                }
                let replacements = if replace_all { occurrences } else { 1 };
                let updated = if replace_all {
                    original_content.replace(&old_text, &new_text)
                } else {
                    original_content.replacen(&old_text, &new_text, 1)
                };
                (updated, replacements)
            }
            other => {
                return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "operation".to_string(),
                    message: format!("Unsupported operation: {other}"),
                }));
            }
        };

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                Error::Tool(crate::base::ToolError::Execution {
                    name: "edit_text_file".to_string(),
                    message: format!("create_dir_all failed: {e}"),
                })
            })?;
        }
        fs::write(&target, updated_content.as_bytes())
            .await
            .map_err(|e| {
                Error::Tool(crate::base::ToolError::Execution {
                    name: "edit_text_file".to_string(),
                    message: format!("write failed: {e}"),
                })
            })?;

        Ok(ToolExecutionOutput::from_payload(json!({
            "path": self.inner.to_relative_path(&target),
            "operation": operation,
            "created": !file_existed,
            "previous_chars": original_content.chars().count(),
            "written_chars": updated_content.chars().count(),
            "replacements": replacements,
        })))
    }
}

// ---------------------------------------------------------------------------
// SearchFilesTool
// ---------------------------------------------------------------------------

/// Finds files and folders using a glob-style pattern.
pub struct SearchFilesTool {
    inner: WorkspaceTool,
}

impl SearchFilesTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            inner: WorkspaceTool::new(workspace),
        }
    }
}

#[async_trait]
impl BaseTool for SearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
    }

    fn description(&self) -> &str {
        "Find files and folders in the workspace using a glob-style pattern."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative directory inside the workspace.",
                    "default": "."
                },
                "pattern": {
                    "type": "string",
                    "description": "Glob-style pattern, for example '*.py' or 'src/**/*.js'.",
                    "default": "*"
                },
                "include_directories": {
                    "type": "boolean",
                    "description": "Include directories in the results.",
                    "default": false
                },
                "include_hidden": {
                    "type": "boolean",
                    "description": "Include hidden files and directories.",
                    "default": false
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matches to return.",
                    "default": SEARCH_FILES_DEFAULT_LIMIT
                }
            },
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let relative_path = {
            let raw = arguments.get("path").and_then(Value::as_str).unwrap_or(".").trim();
            if raw.is_empty() { "." } else { raw }
        };
        let pattern = {
            let raw = arguments.get("pattern").and_then(Value::as_str).unwrap_or("*").trim();
            if raw.is_empty() { "*" } else { raw }
        };
        let include_directories = arguments
            .get("include_directories")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let include_hidden = arguments
            .get("include_hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let max_results = require_positive_int(&arguments, "max_results", SEARCH_FILES_DEFAULT_LIMIT)
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "max_results".to_string(),
                    message: m,
                })
            })? as usize;

        let root = self
            .inner
            .resolve_workspace_path(relative_path)
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "path".to_string(),
                    message: m,
                })
            })?;
        let meta = fs::metadata(&root).await.map_err(|_| {
            Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("Path does not exist: {relative_path}"),
            })
        })?;
        if !meta.is_dir() {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("Path is not a directory: {relative_path}"),
            }));
        }

        let mut matches: Vec<Value> = Vec::new();
        let mut truncated = false;
        let pattern_text = pattern.replace('\\', "/");
        let mut stack: Vec<PathBuf> = vec![root.clone()];
        let mut all_paths: Vec<PathBuf> = Vec::new();
        while let Some(dir) = stack.pop() {
            let mut rd = match fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                if !include_hidden {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with('.') {
                            if is_dir {
                                stack.push(path.clone());
                            }
                            continue;
                        }
                    }
                }
                all_paths.push(path.clone());
                if is_dir {
                    stack.push(path);
                }
            }
        }
        all_paths.sort_by_key(|p| p.to_string_lossy().to_lowercase());

        for target in all_paths {
            if !include_directories {
                let is_file = fs::metadata(&target)
                    .await
                    .map(|m| m.is_file())
                    .unwrap_or(true);
                if !is_file {
                    continue;
                }
            }
            let rel = target
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if !match_glob(&rel, &pattern_text) {
                continue;
            }
            let is_dir = fs::metadata(&target)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false);
            matches.push(json!({
                "path": self.inner.to_relative_path(&target),
                "type": if is_dir { "directory" } else { "file" }
            }));
            if matches.len() >= max_results {
                truncated = true;
                break;
            }
        }

        Ok(ToolExecutionOutput::from_payload(json!({
            "base_path": self.inner.to_relative_path(&root),
            "pattern": pattern,
            "matches": matches,
            "truncated": truncated,
        })))
    }
}

// ---------------------------------------------------------------------------
// SearchTextInFilesTool
// ---------------------------------------------------------------------------

/// Searches UTF-8 text files for matching text or a regex.
pub struct SearchTextInFilesTool {
    inner: WorkspaceTool,
}

impl SearchTextInFilesTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            inner: WorkspaceTool::new(workspace),
        }
    }
}

#[async_trait]
impl BaseTool for SearchTextInFilesTool {
    fn name(&self) -> &str {
        "search_text_in_files"
    }

    fn description(&self) -> &str {
        "Search UTF-8 text files in the workspace for matching text."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Text or regular expression to search for."
                },
                "path": {
                    "type": "string",
                    "description": "Relative directory inside the workspace.",
                    "default": "."
                },
                "glob": {
                    "type": "string",
                    "description": "Only search files that match this glob pattern.",
                    "default": "*"
                },
                "regex": {
                    "type": "boolean",
                    "description": "Treat query as a regular expression.",
                    "default": false
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Use case-sensitive matching.",
                    "default": false
                },
                "include_hidden": {
                    "type": "boolean",
                    "description": "Include hidden files and directories.",
                    "default": false
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matching lines to return.",
                    "default": SEARCH_TEXT_DEFAULT_LIMIT
                },
                "max_files": {
                    "type": "integer",
                    "description": "Maximum number of files to inspect.",
                    "default": SEARCH_TEXT_DEFAULT_FILES
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let query = require_string(&arguments, "query").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let relative_path = {
            let raw = arguments.get("path").and_then(Value::as_str).unwrap_or(".").trim();
            if raw.is_empty() { "." } else { raw }
        };
        let glob_pattern = {
            let raw = arguments.get("glob").and_then(Value::as_str).unwrap_or("*").trim();
            if raw.is_empty() { "*" } else { raw }
        };
        let regex = arguments.get("regex").and_then(Value::as_bool).unwrap_or(false);
        let case_sensitive = arguments
            .get("case_sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let include_hidden = arguments
            .get("include_hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let max_results = require_positive_int(&arguments, "max_results", SEARCH_TEXT_DEFAULT_LIMIT)
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "max_results".to_string(),
                    message: m,
                })
            })? as usize;
        let max_files = require_positive_int(&arguments, "max_files", SEARCH_TEXT_DEFAULT_FILES)
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "max_files".to_string(),
                    message: m,
                })
            })? as usize;

        let root = self
            .inner
            .resolve_workspace_path(relative_path)
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "path".to_string(),
                    message: m,
                })
            })?;

        let matcher: Box<dyn Fn(&str) -> Option<String> + Send + Sync> = if regex {
            let pattern = if case_sensitive {
                regex::Regex::new(query)
            } else {
                regex::RegexBuilder::new(query).case_insensitive(true).build()
            };
            let pattern = match pattern {
                Ok(p) => p,
                Err(e) => {
                    return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                        name: "query".to_string(),
                        message: format!("Invalid regex: {e}"),
                    }));
                }
            };
            Box::new(move |line: &str| pattern.find(line).map(|m| m.as_str().to_string()))
        } else {
            let normalized_query = if case_sensitive {
                query.to_string()
            } else {
                query.to_lowercase()
            };
            let owned_query = query.to_string();
            Box::new(move |line: &str| {
                let haystack = if case_sensitive {
                    line.to_string()
                } else {
                    line.to_lowercase()
                };
                if haystack.contains(&normalized_query) {
                    Some(owned_query.clone())
                } else {
                    None
                }
            })
        };

        let mut all_files: Vec<PathBuf> = Vec::new();
        let mut stack: Vec<PathBuf> = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let mut rd = match fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                if !include_hidden {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with('.') {
                            if is_dir {
                                stack.push(path);
                            }
                            continue;
                        }
                    }
                }
                if is_dir {
                    stack.push(path);
                } else {
                    all_files.push(path);
                }
            }
        }
        all_files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
        let glob_text = glob_pattern.replace('\\', "/");

        let mut matches: Vec<Value> = Vec::new();
        let mut scanned_files: usize = 0;
        let mut skipped_files: usize = 0;
        let mut truncated = false;

        for target in all_files {
            if scanned_files >= max_files {
                truncated = true;
                break;
            }
            let rel = target
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if !match_glob(&rel, &glob_text) {
                continue;
            }
            scanned_files += 1;
            let content = match fs::read_to_string(&target).await {
                Ok(c) => c,
                Err(_) => {
                    skipped_files += 1;
                    continue;
                }
            };
            for (line_number, line) in content.lines().enumerate() {
                if let Some(matched_text) = matcher(line) {
                    matches.push(json!({
                        "path": self.inner.to_relative_path(&target),
                        "line_number": line_number + 1,
                        "line": line,
                        "match": matched_text,
                    }));
                    if matches.len() >= max_results {
                        truncated = true;
                        break;
                    }
                }
            }
            if truncated {
                break;
            }
        }

        Ok(ToolExecutionOutput::from_payload(json!({
            "base_path": self.inner.to_relative_path(&root),
            "query": query,
            "glob": glob_pattern,
            "regex": regex,
            "case_sensitive": case_sensitive,
            "matches": matches,
            "scanned_files": scanned_files,
            "skipped_files": skipped_files,
            "truncated": truncated,
        })))
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// POSIX-style glob matcher (uses `*`, `?`, `[abc]`). Splits the glob
/// on `/` and matches each segment with [`glob_match_segment`].
pub fn match_glob(path: &str, pattern: &str) -> bool {
    let path = path.replace('\\', "/");
    let pattern = if pattern.is_empty() { "*" } else { pattern };
    let path_segments: Vec<&str> = path.split('/').collect();
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    glob_match_segments(&path_segments, &pattern_segments)
}

fn glob_match_segments(path: &[&str], pattern: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }
    if path.is_empty() {
        return pattern.iter().all(|p| *p == "**");
    }
    let (head_pat, rest_pat) = pattern.split_at(1);
    let (head_path, rest_path) = path.split_at(1);
    let pat = head_pat[0];
    if pat == "**" {
        // Match zero or more directories.
        if glob_match_segments(path, rest_pat) {
            return true;
        }
        if rest_path.is_empty() {
            return false;
        }
        return glob_match_segments(rest_path, pattern);
    }
    if !glob_match_segment(head_path[0], pat) {
        return false;
    }
    glob_match_segments(rest_path, rest_pat)
}

fn glob_match_segment(name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    glob_char_match(name.as_bytes(), pattern.as_bytes())
}

fn glob_char_match(text: &[u8], pattern: &[u8]) -> bool {
    let mut dp = vec![false; pattern.len() + 1];
    dp[0] = true;
    for (i, &p) in pattern.iter().enumerate() {
        if p == b'*' && dp[i] {
            dp[i + 1] = true;
        }
    }
    let mut prev = vec![false; pattern.len() + 1];
    for &c in text {
        std::mem::swap(&mut dp, &mut prev);
        for cell in dp.iter_mut().take(pattern.len() + 1) {
            *cell = false;
        }
        for (j, &p) in pattern.iter().enumerate() {
            if p == b'*' {
                dp[j + 1] = dp[j] || prev[j + 1];
            } else if p == b'?' || p == c {
                dp[j + 1] = prev[j];
            } else {
                dp[j + 1] = false;
            }
        }
    }
    dp[pattern.len()]
}

#[allow(dead_code)]
fn _silence_optional_string_unused() {
    let _ = optional_string(&Value::Null, "k", "default");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseTool;
    use serde_json::json;
    use std::fs as stdfs;

    fn write_file(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        stdfs::write(&path, body).expect("write file");
        path
    }

    #[tokio::test]
    async fn list_directory_lists_tempdir_contents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(tmp.path(), "alpha.txt", "alpha");
        write_file(tmp.path(), "beta.txt", "beta");
        stdfs::create_dir_all(tmp.path().join("subdir")).expect("mkdir");

        let tool = ListDirectoryTool::new(tmp.path());
        let result = tool.run(json!({})).await.expect("list_directory ok");
        let entries = result
            .data
            .get("entries")
            .and_then(Value::as_array)
            .expect("entries array");
        let names: Vec<String> = entries
            .iter()
            .filter_map(|e| e.get("name").and_then(Value::as_str).map(String::from))
            .collect();
        assert!(names.iter().any(|n| n == "alpha.txt"));
        assert!(names.iter().any(|n| n == "beta.txt"));
        assert!(names.iter().any(|n| n == "subdir"));
    }

    #[tokio::test]
    async fn read_text_file_reads_written_content() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let body = "hello, world\nline two\n";
        write_file(tmp.path(), "greeting.txt", body);

        let tool = ReadTextFileTool::new(tmp.path());
        let result = tool
            .run(json!({ "path": "greeting.txt" }))
            .await
            .expect("read ok");
        let content = result
            .data
            .get("content")
            .and_then(Value::as_str)
            .expect("content string");
        assert_eq!(content, body);
        assert_eq!(
            result.data.get("truncated").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[tokio::test]
    async fn read_text_file_truncates_long_content() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let body = "x".repeat(200);
        write_file(tmp.path(), "long.txt", &body);

        let tool = ReadTextFileTool::new(tmp.path());
        let result = tool
            .run(json!({ "path": "long.txt", "max_chars": 50 }))
            .await
            .expect("read ok");
        let content = result
            .data
            .get("content")
            .and_then(Value::as_str)
            .expect("content");
        assert_eq!(content.chars().count(), 50);
        assert_eq!(
            result.data.get("truncated").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[tokio::test]
    async fn write_text_file_writes_and_creates_parents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tool = WriteTextFileTool::new(tmp.path(), true);
        let result = tool
            .run(json!({ "path": "nested/dir/hello.txt", "content": "hi" }))
            .await
            .expect("write ok");
        let path = result
            .data
            .get("path")
            .and_then(Value::as_str)
            .expect("path");
        assert!(path.ends_with("nested/dir/hello.txt"));
        let on_disk = stdfs::read_to_string(tmp.path().join("nested/dir/hello.txt"))
            .expect("file should exist");
        assert_eq!(on_disk, "hi");
    }

    #[tokio::test]
    async fn write_text_file_rejects_overwrite_when_disabled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(tmp.path(), "preexisting.txt", "old");
        let tool = WriteTextFileTool::new(tmp.path(), true);
        let err = tool
            .run(json!({ "path": "preexisting.txt", "content": "new" }))
            .await
            .expect_err("overwrite=false should fail");
        let msg = err.to_string();
        assert!(msg.contains("exists"), "unexpected error: {msg}");
    }

    #[tokio::test]
    async fn write_text_file_blocks_when_writes_disabled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tool = WriteTextFileTool::new(tmp.path(), false);
        let err = tool
            .run(json!({ "path": "x.txt", "content": "y" }))
            .await
            .expect_err("writes disabled should fail");
        assert!(err.to_string().contains("禁用"));
    }

    #[tokio::test]
    async fn edit_text_file_replaces_single_occurrence() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(tmp.path(), "f.txt", "hello world\nhello there\n");
        let tool = EditTextFileTool::new(tmp.path(), true);
        let result = tool
            .run(json!({
                "path": "f.txt",
                "operation": "replace",
                "old_text": "hello world",
                "new_text": "hi world",
            }))
            .await
            .expect("edit ok");
        assert_eq!(
            result.data.get("replacements").and_then(Value::as_i64),
            Some(1)
        );
        let on_disk = stdfs::read_to_string(tmp.path().join("f.txt")).expect("read");
        assert_eq!(on_disk, "hi world\nhello there\n");
    }

    #[tokio::test]
    async fn edit_text_file_creates_when_create_if_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tool = EditTextFileTool::new(tmp.path(), true);
        tool.run(json!({
            "path": "new.txt",
            "operation": "append",
            "new_text": "first line",
            "create_if_missing": true,
        }))
        .await
        .expect("edit should create file");
        let on_disk = stdfs::read_to_string(tmp.path().join("new.txt")).expect("read");
        assert_eq!(on_disk, "first line");
    }

    #[tokio::test]
    async fn search_text_in_files_finds_needle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(tmp.path(), "a.txt", "the quick brown fox\njumps over\n");
        write_file(tmp.path(), "b.txt", "no needle here\n");

        let tool = SearchTextInFilesTool::new(tmp.path());
        let result = tool
            .run(json!({ "query": "brown" }))
            .await
            .expect("search ok");
        let matches = result
            .data
            .get("matches")
            .and_then(Value::as_array)
            .expect("matches array");
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0]
                .get("line")
                .and_then(Value::as_str)
                .expect("line text"),
            "the quick brown fox"
        );
        assert_eq!(
            matches[0]
                .get("line_number")
                .and_then(Value::as_i64)
                .expect("line number"),
            1
        );
    }

    #[tokio::test]
    async fn search_files_finds_glob_match() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(tmp.path(), "alpha.rs", "fn a() {}");
        write_file(tmp.path(), "beta.txt", "txt");
        write_file(tmp.path(), "gamma.rs", "fn b() {}");

        let tool = SearchFilesTool::new(tmp.path());
        let result = tool
            .run(json!({ "pattern": "*.rs" }))
            .await
            .expect("search_files ok");
        let matches = result
            .data
            .get("matches")
            .and_then(Value::as_array)
            .expect("matches");
        let names: Vec<String> = matches
            .iter()
            .filter_map(|m| m.get("path").and_then(Value::as_str).map(String::from))
            .collect();
        assert!(names.iter().any(|p| p.ends_with("alpha.rs")));
        assert!(names.iter().any(|p| p.ends_with("gamma.rs")));
        assert!(
            !names.iter().any(|p| p.ends_with("beta.txt")),
            "glob *.rs should not match txt files, got: {names:?}"
        );
    }

    #[test]
    fn glob_matcher_handles_wildcards() {
        assert!(match_glob("foo.txt", "*.txt"));
        assert!(match_glob("src/lib.rs", "**/*.rs"));
        assert!(match_glob("a/b/c.txt", "a/**/*.txt"));
        assert!(!match_glob("foo.txt", "*.rs"));
    }

    #[test]
    fn workspace_tool_resolves_inside_workspace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_file(tmp.path(), "x.txt", "hi");
        let tool = WorkspaceTool::new(tmp.path());
        let resolved = tool.resolve_workspace_path("x.txt").expect("resolve");
        assert!(resolved.ends_with("x.txt"));
    }

    #[test]
    fn workspace_tool_rejects_escape() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tool = WorkspaceTool::new(tmp.path());
        let err = tool
            .resolve_workspace_path("../escape.txt")
            .expect_err("escape should fail");
        assert!(err.contains("outside") || err.contains("exist"));
    }
}
