//! Planning tools: `update_plan` and `request_user_input`.
//!
//! Ports `echobot/tools/planning.py`. `request_user_input` pauses the
//! agent loop via a [`ToolLoopControl`] signal.

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{json, Value};

use echobot_core::models::MessageContent;

use echobot_core::Error;

use crate::base::{require_string, BaseTool, ToolExecutionOutput, ToolLoopControl, ToolTraceEvent};

// ---------------------------------------------------------------------------
// UpdatePlanTool
// ---------------------------------------------------------------------------

/// Valid plan statuses.
const PLAN_STATUSES: &[&str] = &["pending", "in_progress", "completed"];

/// Records or updates a short plan.
pub struct UpdatePlanTool {
    latest_plan: Mutex<Vec<(String, String)>>,
}

impl UpdatePlanTool {
    /// Creates a new tool.
    pub fn new() -> Self {
        Self {
            latest_plan: Mutex::new(Vec::new()),
        }
    }
}

impl Default for UpdatePlanTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BaseTool for UpdatePlanTool {
    fn name(&self) -> &str {
        "update_plan"
    }

    fn description(&self) -> &str {
        "Record or update a short plan for the current task. Use this for multi-step work and keep the statuses current."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "explanation": {
                    "type": "string",
                    "description": "Optional short explanation for the plan update.",
                    "default": ""
                },
                "plan": {
                    "type": "array",
                    "description": "The current plan.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": {
                                "type": "string",
                                "description": "A short step description."
                            },
                            "status": {
                                "type": "string",
                                "description": "One of pending, in_progress, completed.",
                                "enum": ["pending", "in_progress", "completed"]
                            }
                        },
                        "required": ["step", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["plan"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let explanation = arguments
            .get("explanation")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let plan = normalize_plan(arguments.get("plan"))?;
        let normalized: Vec<Value> = plan
            .iter()
            .map(|(step, status)| json!({ "step": step, "status": status }))
            .collect();
        {
            let mut guard = self.latest_plan.lock().expect("plan lock");
            *guard = plan.clone();
        }
        let current_step = current_plan_step(&plan);

        let mut details_lines: Vec<String> = Vec::new();
        if !explanation.is_empty() {
            details_lines.push(explanation.clone());
            details_lines.push(String::new());
        }
        for (step, status) in &plan {
            details_lines.push(format!("[{status}] {step}"));
        }
        let trace = ToolTraceEvent {
            event: "plan_updated".to_string(),
            data: json!({
                "title": "Plan updated",
                "summary": plan_summary(&plan),
                "details": details_lines.join("\n").trim().to_string(),
                "explanation": explanation,
                "plan": normalized,
                "current_step": current_step,
            }),
        };
        let mut output = ToolExecutionOutput::from_payload(json!({
            "kind": "plan_update",
            "explanation": explanation,
            "plan": normalized,
            "current_step": current_step,
        }));
        output.trace_events.push(trace);
        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// RequestUserInputTool
// ---------------------------------------------------------------------------

/// Pauses the current task and asks the user a focused follow-up.
pub struct RequestUserInputTool;

impl RequestUserInputTool {
    /// Creates a new tool.
    pub fn new() -> Self {
        Self
    }
}

impl Default for RequestUserInputTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BaseTool for RequestUserInputTool {
    fn name(&self) -> &str {
        "request_user_input"
    }

    fn description(&self) -> &str {
        "Pause the current task and ask the user a focused follow-up question when you are blocked by missing information."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The exact short question or request the user should see."
                },
                "choices": {
                    "type": "array",
                    "description": "Optional short answer choices to help the user reply.",
                    "items": { "type": "string" },
                    "default": []
                },
                "why_needed": {
                    "type": "string",
                    "description": "Optional short internal note explaining why this answer is needed.",
                    "default": ""
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let prompt = require_string(&arguments, "prompt").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;

        let raw_choices = arguments.get("choices").and_then(Value::as_array);
        let choices: Vec<String> = match raw_choices {
            Some(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
            None => Vec::new(),
        };
        let why_needed = arguments
            .get("why_needed")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();

        let mut response_lines: Vec<String> = vec![prompt.to_string()];
        if !choices.is_empty() {
            response_lines.push(String::new());
            response_lines.push("可参考的回答：".to_string());
            for choice in &choices {
                response_lines.push(format!("- {choice}"));
            }
        }
        let response_text = response_lines.join("\n").trim().to_string();
        let pending_request = json!({
            "prompt": prompt,
            "choices": choices,
            "why_needed": why_needed,
        });
        let trace = ToolTraceEvent {
            event: "user_input_requested".to_string(),
            data: json!({
                "title": "Waiting for user input",
                "summary": prompt,
                "details": user_input_details(&prompt, &choices, &why_needed),
                "request": pending_request,
            }),
        };
        let mut output = ToolExecutionOutput::from_payload(json!({
            "kind": "user_input_request",
            "request": pending_request,
        }));
        output.trace_events.push(trace);
        output.control = Some(ToolLoopControl {
            action: "await_user_input".to_string(),
            status: "waiting_for_input".to_string(),
            response_content: MessageContent::Text(response_text),
            metadata: pending_request,
        });
        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

fn normalize_plan(value: Option<&Value>) -> Result<Vec<(String, String)>, Error> {
    let Some(arr) = value.and_then(Value::as_array) else {
        return Err(Error::Tool(crate::base::ToolError::InvalidValue {
            name: "plan".to_string(),
            message: "plan must be a non-empty array".to_string(),
        }));
    };
    if arr.is_empty() {
        return Err(Error::Tool(crate::base::ToolError::InvalidValue {
            name: "plan".to_string(),
            message: "plan must be a non-empty array".to_string(),
        }));
    }
    let mut normalized: Vec<(String, String)> = Vec::new();
    let mut in_progress = 0;
    for item in arr {
        let Some(obj) = item.as_object() else {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "plan".to_string(),
                message: "each plan item must be an object".to_string(),
            }));
        };
        let step = obj
            .get("step")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if step.is_empty() {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "plan".to_string(),
                message: "each plan item must include a non-empty step".to_string(),
            }));
        }
        let status = obj
            .get("status")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if !PLAN_STATUSES.contains(&status.as_str()) {
            return Err(Error::Tool(crate::base::ToolError::InvalidValue {
                name: "plan".to_string(),
                message: "plan status must be pending, in_progress, or completed".to_string(),
            }));
        }
        if status == "in_progress" {
            in_progress += 1;
        }
        normalized.push((step, status));
    }
    if in_progress > 1 {
        return Err(Error::Tool(crate::base::ToolError::InvalidValue {
            name: "plan".to_string(),
            message: "only one plan item can be in_progress".to_string(),
        }));
    }
    Ok(normalized)
}

fn current_plan_step(plan: &[(String, String)]) -> String {
    for (step, status) in plan {
        if status == "in_progress" {
            return step.clone();
        }
    }
    String::new()
}

fn plan_summary(plan: &[(String, String)]) -> String {
    let current = current_plan_step(plan);
    if current.is_empty() {
        format!("{} steps", plan.len())
    } else {
        format!("{} steps, current: {current}", plan.len())
    }
}

fn user_input_details(prompt: &str, choices: &[String], why_needed: &str) -> String {
    let mut lines: Vec<String> = vec![prompt.to_string()];
    if !choices.is_empty() {
        lines.push(String::new());
        lines.push("Choices:".to_string());
        for c in choices {
            lines.push(format!("- {c}"));
        }
    }
    if !why_needed.is_empty() {
        lines.push(String::new());
        lines.push("Why needed:".to_string());
        lines.push(why_needed.to_string());
    }
    lines.join("\n").trim().to_string()
}
