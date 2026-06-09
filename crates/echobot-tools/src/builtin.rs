//! Built-in tool registry factory + `CurrentTimeTool`.
//!
//! Ports `echobot/tools/builtin.py`. The factory takes a
//! [`BasicToolDeps`] bundle and returns a [`ToolRegistry`] preloaded
//! with the standard tools. Optional dependencies (cron, memory, the
//! media / image tools) are included only when the caller supplies
//! them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Local;
use serde_json::{json, Value};

use echobot_core::attachments::AttachmentStore;
use echobot_core::Error;

use crate::base::{BaseTool, ToolExecutionOutput, ToolRegistry};
use crate::cron::CronTool;
use crate::filesystem::{
    EditTextFileTool, ListDirectoryTool, ReadTextFileTool, SearchFilesTool,
    SearchTextInFilesTool, WriteTextFileTool,
};
use crate::git::{GitDiffTool, GitStatusTool};
use crate::media::{SendFileToUserTool, SendImageToUserTool, ViewImageTool};
use crate::memory::MemorySearchTool;
use crate::planning::{RequestUserInputTool, UpdatePlanTool};
use crate::shell::{CommandExecutionTool, ShellSafetyMode};
use crate::web::WebRequestTool;

/// Dependencies for [`create_basic_tool_registry`].
#[derive(Clone)]
pub struct BasicToolDeps {
    /// Workspace root (defaults to `.`).
    pub workspace: Option<PathBuf>,
    /// Optional attachment store; required for the media tools.
    pub attachment_store: Option<Arc<AttachmentStore>>,
    /// Whether the model can ingest image inputs.
    pub supports_image_input: bool,
    /// Optional memory subsystem.
    pub memory_support: Option<Arc<dyn crate::memory::MemorySupport>>,
    /// Optional cron service.
    pub cron_service: Option<Arc<dyn crate::cron::CronService>>,
    /// Session name used by the cron tool when none is provided.
    pub session_name: String,
    /// Whether file-writing tools are enabled.
    pub allow_file_writes: bool,
    /// Whether cron mutations are allowed.
    pub allow_cron_mutations: bool,
    /// Whether the web tool is allowed to hit private-network addresses.
    pub allow_private_network: bool,
    /// Shell safety mode (default: `DangerFullAccess`).
    pub shell_safety_mode: ShellSafetyMode,
}

impl Default for BasicToolDeps {
    fn default() -> Self {
        Self {
            workspace: None,
            attachment_store: None,
            supports_image_input: true,
            memory_support: None,
            cron_service: None,
            session_name: "default".to_string(),
            allow_file_writes: true,
            allow_cron_mutations: true,
            allow_private_network: false,
            shell_safety_mode: ShellSafetyMode::DangerFullAccess,
        }
    }
}

impl BasicToolDeps {
    /// Resolves the workspace to a canonical path.
    pub fn workspace(&self) -> PathBuf {
        self.workspace
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Returns the current local time.
pub struct CurrentTimeTool;

#[async_trait]
impl BaseTool for CurrentTimeTool {
    fn name(&self) -> &str {
        "get_current_time"
    }

    fn description(&self) -> &str {
        "Get the current local time."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn run(&self, _arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let now = Local::now();
        let payload = json!({
            "current_time": now.to_rfc3339(),
            "timezone": now.offset().to_string(),
        });
        Ok(ToolExecutionOutput::from_payload(payload))
    }
}

/// Builds a [`ToolRegistry`] preloaded with the standard tools.
pub fn create_basic_tool_registry(deps: BasicToolDeps) -> ToolRegistry {
    let workspace: PathBuf = deps.workspace();
    let mut tools: Vec<Arc<dyn BaseTool>> = vec![
        Arc::new(CurrentTimeTool),
        Arc::new(ListDirectoryTool::new(&workspace)),
        Arc::new(SearchFilesTool::new(&workspace)),
        Arc::new(SearchTextInFilesTool::new(&workspace)),
        Arc::new(ReadTextFileTool::new(&workspace)),
        Arc::new(WriteTextFileTool::new(&workspace, deps.allow_file_writes)),
        Arc::new(EditTextFileTool::new(&workspace, deps.allow_file_writes)),
        Arc::new(GitStatusTool::new(&workspace)),
        Arc::new(GitDiffTool::new(&workspace)),
        Arc::new(UpdatePlanTool::new()),
        Arc::new(RequestUserInputTool::new()),
        Arc::new(WebRequestTool::new(deps.allow_private_network)),
        Arc::new(CommandExecutionTool::new(
            &workspace,
            deps.shell_safety_mode,
            deps.allow_file_writes,
        )),
    ];

    if let Some(store) = &deps.attachment_store {
        if deps.supports_image_input {
            tools.push(Arc::new(ViewImageTool::new(&workspace, store.clone())));
        }
        tools.push(Arc::new(SendImageToUserTool::new(&workspace, store.clone())));
        tools.push(Arc::new(SendFileToUserTool::new(&workspace, store.clone())));
    }
    if let Some(memory) = &deps.memory_support {
        tools.push(Arc::new(MemorySearchTool::new(memory.clone())));
    }
    if let Some(cron) = &deps.cron_service {
        tools.push(Arc::new(CronTool::new(
            cron.clone(),
            deps.session_name.clone(),
            deps.allow_cron_mutations,
        )));
    }
    ToolRegistry::from_tools(tools)
}

/// Convenience: builds a [`BasicToolDeps`] with a workspace path.
pub fn basic_deps_for_workspace(workspace: impl AsRef<Path>) -> BasicToolDeps {
    BasicToolDeps {
        workspace: Some(workspace.as_ref().to_path_buf()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseTool;
    use serde_json::json;

    #[tokio::test]
    async fn current_time_tool_returns_non_empty_timestamp() {
        let tool = CurrentTimeTool;
        let result = tool.run(json!({})).await.expect("run succeeds");
        let payload = result.data;
        let ts = payload
            .get("current_time")
            .and_then(Value::as_str)
            .expect("current_time field is a string");
        assert!(!ts.is_empty(), "timestamp should be non-empty: {ts}");
        // RFC3339 timestamps include 'T' separator and timezone offset.
        assert!(ts.contains('T'), "timestamp should look like RFC3339: {ts}");
        let tz = payload
            .get("timezone")
            .and_then(Value::as_str)
            .expect("timezone field is a string");
        assert!(!tz.is_empty(), "timezone should be non-empty: {tz}");
    }

    #[test]
    fn current_time_tool_metadata_is_well_formed() {
        let tool = CurrentTimeTool;
        assert_eq!(tool.name(), "get_current_time");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
    }

    #[test]
    fn basic_tool_registry_is_populated() {
        let registry = create_basic_tool_registry(BasicToolDeps::default());
        let names = registry.names();
        // The standard set of tools must always be present.
        for required in [
            "get_current_time",
            "list_directory",
            "read_text_file",
            "write_text_file",
            "edit_text_file",
            "search_files",
            "search_text_in_files",
            "git_status",
            "git_diff",
            "update_plan",
            "request_user_input",
            "fetch_web_page",
            "run_shell_command",
        ] {
            assert!(
                names.iter().any(|n| n == required),
                "registry should include {required}, got: {names:?}"
            );
        }
    }

    #[test]
    fn basic_deps_for_workspace_overrides_workspace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let deps = basic_deps_for_workspace(tmp.path());
        assert_eq!(deps.workspace(), tmp.path());
    }
}
