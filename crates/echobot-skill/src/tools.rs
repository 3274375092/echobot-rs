//! Skill-management tools exposed to the LLM.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use echobot_core::Error;
use echobot_tools::{truncate_text, BaseTool, ToolExecutionOutput};

use crate::models::{Skill, SkillRuntimeState, RESOURCE_FOLDERS};
use crate::registry::SkillRegistry;

// ---------------------------------------------------------------------------
// ActivateSkillTool
// ---------------------------------------------------------------------------

/// Loads a skill's core instructions by name.
pub struct ActivateSkillTool {
    registry: Arc<SkillRegistry>,
    runtime_state: SkillRuntimeState,
}

impl ActivateSkillTool {
    /// Creates a new tool.
    pub fn new(registry: Arc<SkillRegistry>, runtime_state: SkillRuntimeState) -> Self {
        Self {
            registry,
            runtime_state,
        }
    }
}

#[async_trait]
impl BaseTool for ActivateSkillTool {
    fn name(&self) -> &str {
        "activate_skill"
    }

    fn description(&self) -> &str {
        "Load a skill's core instructions by name. This only loads the main skill text. Bundled resource files stay unloaded until you inspect or read them explicitly."
    }

    fn parameters(&self) -> Value {
        let mut names = self.registry.names();
        if names.is_empty() {
            names = vec!["<no skills registered>".to_string()];
        }
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The exact skill name to activate.",
                    "enum": names
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let raw_name = arguments
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::Tool(echobot_core::ToolError::MissingArgument("name".to_string()))
            })?;
        let mut state = self.runtime_state.clone();
        let skill = self
            .registry
            .require_skill(raw_name)
            .map_err(|m| Error::Tool(echobot_core::ToolError::InvalidValue {
                name: "name".to_string(),
                message: m,
            }))?;
        let already_active = state.is_active(&skill.name);
        if !already_active {
            state.activate(&skill.name);
        }
        let payload = json!({
            "kind": "skill_activation",
            "name": skill.name,
            "description": skill.description,
            "directory": skill.directory.to_string_lossy(),
            "already_active": already_active,
            "resource_summary": skill.resource_summary(),
            "content": skill.to_activation_text(),
        });
        Ok(ToolExecutionOutput::from_payload(payload))
    }
}

// ---------------------------------------------------------------------------
// ListSkillResourcesTool
// ---------------------------------------------------------------------------

/// Lists bundled files for an activated skill.
pub struct ListSkillResourcesTool {
    registry: Arc<SkillRegistry>,
    runtime_state: SkillRuntimeState,
}

impl ListSkillResourcesTool {
    /// Creates a new tool.
    pub fn new(registry: Arc<SkillRegistry>, runtime_state: SkillRuntimeState) -> Self {
        Self {
            registry,
            runtime_state,
        }
    }
}

#[async_trait]
impl BaseTool for ListSkillResourcesTool {
    fn name(&self) -> &str {
        "list_skill_resources"
    }

    fn description(&self) -> &str {
        "List bundled files for an activated skill. Use this after activate_skill when you need to inspect which scripts, references, assets, or agents are available."
    }

    fn parameters(&self) -> Value {
        let mut names = self.registry.names();
        if names.is_empty() {
            names = vec!["<no skills registered>".to_string()];
        }
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The exact activated skill name.",
                    "enum": names
                },
                "folder": {
                    "type": "string",
                    "description": "Optional resource folder: scripts, references, assets, or agents."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of file paths to return.",
                    "default": 50
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let raw_name = arguments.get("name").and_then(Value::as_str).unwrap_or("");
        let folder = read_optional_folder_name(arguments.get("folder"))?;
        let limit = read_positive_int(arguments.get("limit"), "limit", 50).map_err(|m| {
            Error::Tool(echobot_core::ToolError::InvalidValue {
                name: "limit".to_string(),
                message: m,
            })
        })? as usize;
        let skill = self
            .registry
            .require_active_skill(raw_name, &self.runtime_state)
            .map_err(|m| Error::Tool(echobot_core::ToolError::InvalidValue {
                name: "name".to_string(),
                message: m,
            }))?;
        let files = skill
            .resource_files(folder.as_deref())
            .map_err(|m| {
                Error::Tool(echobot_core::ToolError::InvalidValue {
                    name: "folder".to_string(),
                    message: m,
                })
            })?;
        let total = files.len();
        let truncated = total > limit;
        let entries: Vec<String> = files.into_iter().take(limit).collect();
        let payload = json!({
            "kind": "skill_resource_list",
            "name": skill.name,
            "folder": folder.as_deref().unwrap_or("all"),
            "entries": entries,
            "total_files": total,
            "truncated": truncated,
        });
        Ok(ToolExecutionOutput::from_payload(payload))
    }
}

// ---------------------------------------------------------------------------
// ReadSkillResourceTool
// ---------------------------------------------------------------------------

/// Reads one UTF-8 resource file from an activated skill.
pub struct ReadSkillResourceTool {
    registry: Arc<SkillRegistry>,
    runtime_state: SkillRuntimeState,
}

impl ReadSkillResourceTool {
    /// Creates a new tool.
    pub fn new(registry: Arc<SkillRegistry>, runtime_state: SkillRuntimeState) -> Self {
        Self {
            registry,
            runtime_state,
        }
    }
}

#[async_trait]
impl BaseTool for ReadSkillResourceTool {
    fn name(&self) -> &str {
        "read_skill_resource"
    }

    fn description(&self) -> &str {
        "Read one UTF-8 resource file from an activated skill. This is for loading a single reference or script only when the task actually needs it."
    }

    fn parameters(&self) -> Value {
        let mut names = self.registry.names();
        if names.is_empty() {
            names = vec!["<no skills registered>".to_string()];
        }
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The exact activated skill name.",
                    "enum": names
                },
                "path": {
                    "type": "string",
                    "description": "Relative file path inside the skill's scripts, references, assets, or agents folders."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum number of characters to return.",
                    "default": 4000
                }
            },
            "required": ["name", "path"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let raw_name = arguments.get("name").and_then(Value::as_str).unwrap_or("");
        let relative_path = arguments
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if relative_path.is_empty() {
            return Err(Error::Tool(echobot_core::ToolError::MissingArgument(
                "path".to_string(),
            )));
        }
        let max_chars = read_positive_int(arguments.get("max_chars"), "max_chars", 4000)
            .map_err(|m| {
                Error::Tool(echobot_core::ToolError::InvalidValue {
                    name: "max_chars".to_string(),
                    message: m,
                })
            })? as usize;
        let skill = self
            .registry
            .require_active_skill(raw_name, &self.runtime_state)
            .map_err(|m| Error::Tool(echobot_core::ToolError::InvalidValue {
                name: "name".to_string(),
                message: m,
            }))?;
        let target: PathBuf = skill
            .resolve_resource_path(relative_path)
            .map_err(|m| {
                Error::Tool(echobot_core::ToolError::InvalidValue {
                    name: "path".to_string(),
                    message: m,
                })
            })?;
        let meta = std::fs::metadata(&target).map_err(|_| {
            Error::Tool(echobot_core::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("File does not exist: {relative_path}"),
            })
        })?;
        if !meta.is_file() {
            return Err(Error::Tool(echobot_core::ToolError::InvalidValue {
                name: "path".to_string(),
                message: format!("Path is not a file: {relative_path}"),
            }));
        }
        let content = std::fs::read_to_string(&target).map_err(|_| {
            Error::Tool(echobot_core::ToolError::InvalidValue {
                name: "path".to_string(),
                message: "Only UTF-8 text skill resources are supported".to_string(),
            })
        })?;
        let (text, truncated) = truncate_text(&content, max_chars);
        let display = target
            .strip_prefix(&skill.directory)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| target.to_string_lossy().replace('\\', "/"));
        let payload = json!({
            "kind": "skill_resource_content",
            "name": skill.name,
            "path": display,
            "content": text,
            "total_chars": content.chars().count(),
            "truncated": truncated,
        });
        Ok(ToolExecutionOutput::from_payload(payload))
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

fn read_optional_folder_name(value: Option<&Value>) -> Result<Option<String>, Error> {
    let Some(v) = value else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let Some(s) = v.as_str() else {
        return Err(Error::Tool(echobot_core::ToolError::InvalidValue {
            name: "folder".to_string(),
            message: "folder must be a string".to_string(),
        }));
    };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !RESOURCE_FOLDERS.contains(&trimmed) {
        return Err(Error::Tool(echobot_core::ToolError::InvalidValue {
            name: "folder".to_string(),
            message: format!("folder must be one of: {}", RESOURCE_FOLDERS.join(", ")),
        }));
    }
    Ok(Some(trimmed.to_string()))
}

fn read_positive_int(value: Option<&Value>, name: &str, default: i64) -> Result<i64, String> {
    let raw = match value {
        None | Some(Value::Null) => default,
        Some(v) => v
            .as_i64()
            .ok_or_else(|| format!("{name} must be an integer"))?,
    };
    if raw <= 0 {
        return Err(format!("{name} must be greater than 0"));
    }
    Ok(raw)
}

#[allow(dead_code)]
fn _silence_unused(_skill: &Skill) {}
