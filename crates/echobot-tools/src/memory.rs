//! `MemorySearchTool` — semantic search over `MEMORY.md` and
//! `memory/*.md`.
//!
//! **Phase-2 stub.** The Python runtime wires this tool to a
//! long-term-memory subsystem (vector search over embedded memory
//! files). The Rust port returns an empty result for now; the
//! signature, parameter schema, and tool name match the Python tool
//! so the LLM-facing contract is stable.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use echobot_core::Error;

use crate::base::{require_string, BaseTool, ToolExecutionOutput};

/// The trait the runtime will implement in phase 2 to back the
/// memory search tool. For now the default implementation returns an
/// empty list so the registry can include the tool without
/// unconditional panics.
#[async_trait::async_trait]
pub trait MemorySupport: Send + Sync {
    /// Performs a semantic search and returns a list of matches.
    async fn search(
        &self,
        query: &str,
        max_results: usize,
        min_score: f32,
    ) -> Result<Vec<MemoryHit>, Error>;
}

/// A single memory hit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryHit {
    /// Source file path (relative to the workspace).
    pub path: String,
    /// Score (0..1).
    pub score: f32,
    /// Matched snippet.
    pub snippet: String,
}

/// Empty / no-op memory backend; the registry uses this by default.
pub struct NoopMemorySupport;

#[async_trait::async_trait]
impl MemorySupport for NoopMemorySupport {
    async fn search(
        &self,
        _query: &str,
        _max_results: usize,
        _min_score: f32,
    ) -> Result<Vec<MemoryHit>, Error> {
        Ok(Vec::new())
    }
}

/// Memory search tool.
pub struct MemorySearchTool {
    support: Arc<dyn MemorySupport>,
}

impl MemorySearchTool {
    /// Creates a new tool backed by `support`.
    pub fn new(support: Arc<dyn MemorySupport>) -> Self {
        Self { support }
    }
}

#[async_trait]
impl BaseTool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search MEMORY.md and memory/*.md for prior work, user preferences, decisions, dates, or todos."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Semantic search query for stored memory."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return.",
                    "default": 5
                },
                "min_score": {
                    "type": "number",
                    "description": "Minimum match score between 0 and 1.",
                    "default": 0.1
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        // TODO(phase-2): wire to long-term memory subsystem.
        let query = require_string(&arguments, "query").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let max_results = arguments
            .get("max_results")
            .and_then(Value::as_i64)
            .unwrap_or(5)
            .max(1) as usize;
        let min_score = arguments
            .get("min_score")
            .and_then(Value::as_f64)
            .unwrap_or(0.1)
            .clamp(0.0, 1.0) as f32;
        let hits = self.support.search(query, max_results, min_score).await?;
        Ok(ToolExecutionOutput::from_payload(json!({
            "kind": "memory_search",
            "query": query,
            "hits": hits,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseTool;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn memory_tool_metadata_is_well_formed() {
        let tool = MemorySearchTool::new(Arc::new(NoopMemorySupport));
        assert_eq!(tool.name(), "memory_search");
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        let required = params["required"].as_array().expect("required array");
        assert!(required.iter().any(|v| v == "query"));
    }

    #[tokio::test]
    async fn memory_search_tool_returns_empty_result() {
        let tool = MemorySearchTool::new(Arc::new(NoopMemorySupport));
        let result = tool
            .run(json!({ "query": "what did we decide about x?" }))
            .await
            .expect("memory_search should not panic");
        assert_eq!(result.data["kind"], "memory_search");
        let hits = result
            .data
            .get("hits")
            .and_then(Value::as_array)
            .expect("hits array");
        assert!(hits.is_empty(), "noop backend should return no hits");
    }

    #[tokio::test]
    async fn memory_search_tool_requires_query() {
        let tool = MemorySearchTool::new(Arc::new(NoopMemorySupport));
        let err = tool
            .run(json!({}))
            .await
            .expect_err("missing query should fail");
        assert!(err.to_string().contains("query"));
    }

    #[test]
    fn noop_memory_support_clamps_inputs() {
        // Just confirm the trait object is `Send + Sync` and usable in
        // the registry's default-args world.
        let support: Arc<dyn MemorySupport> = Arc::new(NoopMemorySupport);
        let tool = MemorySearchTool::new(support);
        assert_eq!(tool.name(), "memory_search");
    }
}
