//! Git tools: `git_status` and `git_diff`. Both run `git` directly via
//! [`tokio::process::Command`] and decode stdout / stderr with the same
//! locale-aware decoder as [`crate::shell`].

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use echobot_core::Error;

use crate::base::{require_positive_int, require_string, BaseTool, ToolExecutionOutput};
use crate::shell::decode_command_output;

// ---------------------------------------------------------------------------
// Shared git runner
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct GitResult {
    return_code: Option<i32>,
    stdout: String,
    stderr: String,
}

async fn run_git(workspace: &Path, args: &[&str], allow_failure: bool) -> Result<GitResult, Error> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().await.map_err(|e| {
        Error::Tool(crate::base::ToolError::Execution {
            name: "git".to_string(),
            message: format!("git {} failed to spawn: {e}", args.join(" ")),
        })
    })?;
    let stdout = decode_command_output(&output.stdout);
    let stderr = decode_command_output(&output.stderr);
    let return_code = output.status.code();
    if output.status.code() != Some(0) && !allow_failure {
        let message = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            format!("git {} failed", args.join(" "))
        };
        return Err(Error::Tool(crate::base::ToolError::Execution {
            name: "git".to_string(),
            message,
        }));
    }
    Ok(GitResult {
        return_code,
        stdout,
        stderr,
    })
}

async fn ensure_git_repository(workspace: &Path) -> Result<(), Error> {
    let result = run_git(workspace, &["rev-parse", "--show-toplevel"], true).await?;
    if result.return_code == Some(0) {
        return Ok(());
    }
    let stderr = result.stderr.trim().to_string();
    if !stderr.is_empty() {
        return Err(Error::Tool(crate::base::ToolError::InvalidValue {
            name: "workspace".to_string(),
            message: stderr,
        }));
    }
    Err(Error::Tool(crate::base::ToolError::InvalidValue {
        name: "workspace".to_string(),
        message: format!("Workspace is not inside a git repository: {}", workspace.display()),
    }))
}

fn workspace_root(workspace: &Path) -> PathBuf {
    workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf())
}

fn resolve_within_workspace(
    workspace: &Path,
    relative_path: &str,
) -> std::result::Result<PathBuf, String> {
    let root = workspace.canonicalize().unwrap_or_else(|_| workspace.to_path_buf());
    let target = root.join(relative_path);
    let target = target
        .canonicalize()
        .map_err(|e| format!("Path does not exist: {relative_path} ({e})"))?;
    if !target.starts_with(&root) {
        return Err(format!("Path is outside the workspace: {relative_path}"));
    }
    Ok(target)
}

fn to_relative_path(workspace: &Path, target: &Path) -> String {
    let root = workspace.canonicalize().unwrap_or_else(|_| workspace.to_path_buf());
    let target_canon = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());
    target_canon
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| target_canon.to_string_lossy().replace('\\', "/"))
}

// ---------------------------------------------------------------------------
// GitStatusTool
// ---------------------------------------------------------------------------

/// Runs `git status --short --branch` in the workspace.
pub struct GitStatusTool {
    workspace: PathBuf,
}

impl GitStatusTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
        }
    }
}

#[async_trait]
impl BaseTool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show the current git status for the workspace."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn run(&self, _arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let root = workspace_root(&self.workspace);
        ensure_git_repository(&root).await?;
        let result = run_git(&root, &["status", "--short", "--branch"], false).await?;
        let lines: Vec<&str> = result
            .stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        Ok(ToolExecutionOutput::from_payload(json!({
            "workspace": root.to_string_lossy(),
            "text": result.stdout,
            "lines": lines,
        })))
    }
}

// ---------------------------------------------------------------------------
// GitDiffTool
// ---------------------------------------------------------------------------

/// Runs `git diff` in the workspace.
pub struct GitDiffTool {
    workspace: PathBuf,
}

impl GitDiffTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
        }
    }
}

#[async_trait]
impl BaseTool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show a git diff for the workspace or one file."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional relative file or directory path inside the workspace.",
                    "default": ""
                },
                "staged": {
                    "type": "boolean",
                    "description": "Show the staged diff instead of the working tree diff.",
                    "default": false
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum number of characters to return.",
                    "default": 12000
                }
            },
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let root = workspace_root(&self.workspace);
        ensure_git_repository(&root).await?;

        let relative_path = arguments
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let staged = arguments
            .get("staged")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let max_chars = require_positive_int(&arguments, "max_chars", 12000)
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "max_chars".to_string(),
                    message: m,
                })
            })? as usize;

        let mut args: Vec<String> = vec!["--no-pager".to_string(), "diff".to_string()];
        if staged {
            args.push("--cached".to_string());
        }
        let mut normalized_path = String::new();
        if !relative_path.is_empty() {
            // Validate the path is within the workspace but pass the
            // relative form to git for prettier diff headers.
            resolve_within_workspace(&root, relative_path).map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "path".to_string(),
                    message: m,
                })
            })?;
            let target = root.join(relative_path);
            normalized_path = to_relative_path(&root, &target);
            args.push("--".to_string());
            args.push(normalized_path.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = run_git(&root, &arg_refs, false).await?;
        let diff = result.stdout;
        let (truncated_diff, truncated) = crate::base::truncate_text(&diff, max_chars);
        Ok(ToolExecutionOutput::from_payload(json!({
            "workspace": root.to_string_lossy(),
            "path": normalized_path,
            "staged": staged,
            "diff": truncated_diff,
            "total_chars": diff.chars().count(),
            "truncated": truncated,
        })))
    }
}

#[allow(dead_code)]
fn _silence_unused() {
    let _ = require_string(&Value::Null, "k");
}
