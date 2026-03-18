//! Tool trait and execution context.
//!
//! Products implement [`Tool`] to give agents domain-specific capabilities.
//! The framework calls tools during the Act phase of the agentic loop.
//!
//! # Example
//! ```rust,ignore
//! struct FileReader;
//!
//! #[async_trait]
//! impl Tool for FileReader {
//!     fn name(&self) -> &str { "read_file" }
//!     fn description(&self) -> &str { "Read file contents at a path" }
//!     fn parameters(&self) -> Value {
//!         json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]})
//!     }
//!     async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
//!         let path = params["path"].as_str().ok_or(PulseHiveError::validation("path required"))?;
//!         Ok(ToolResult::text(tokio::fs::read_to_string(path).await.unwrap()))
//!     }
//! }
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use pulsedb::{CollectiveId, SubstrateProvider};
use serde_json::Value;

use crate::error::Result;
use crate::event::EventEmitter;

/// Trait for domain-specific tool implementations.
///
/// Tools are the capabilities that agents can invoke during task execution.
/// The LLM decides which tool to call based on the `name()`, `description()`,
/// and `parameters()` (JSON Schema) exposed to it.
///
/// Must be `Send + Sync` for concurrent execution across Tokio tasks.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name shown to the LLM for selection.
    fn name(&self) -> &str;

    /// Description the LLM uses to decide when to invoke this tool.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's parameters.
    fn parameters(&self) -> Value;

    /// Execute the tool with the given parameters.
    async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult>;

    /// Whether this tool requires human approval before execution.
    ///
    /// When `true`, the framework calls the [`ApprovalHandler`](crate::approval::ApprovalHandler)
    /// before executing. Default: `false`.
    fn requires_approval(&self) -> bool {
        false
    }
}

/// Runtime context available to tools during execution.
///
/// Provides access to the agent's identity, the shared substrate, and the event
/// emitter for tools that need to read/write experiences or emit custom events.
pub struct ToolContext {
    /// ID of the agent executing this tool.
    pub agent_id: String,
    /// Collective (namespace) the agent belongs to.
    pub collective_id: CollectiveId,
    /// Shared substrate for reading/writing experiences during tool execution.
    pub substrate: Arc<dyn SubstrateProvider>,
    /// Event emitter for tools that need to emit custom events.
    pub event_emitter: EventEmitter,
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub enum ToolResult {
    /// Plain text result.
    Text(String),
    /// Structured JSON result.
    Json(Value),
    /// Error message (tool failed but execution continues — LLM is informed).
    Error(String),
}

impl ToolResult {
    /// Creates a text result.
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }

    /// Creates a JSON result.
    pub fn json(v: Value) -> Self {
        Self::Json(v)
    }

    /// Creates an error result.
    pub fn error(s: impl Into<String>) -> Self {
        Self::Error(s.into())
    }

    /// Returns the result as a string (for sending back to the LLM).
    pub fn to_content(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Json(v) => v.to_string(),
            Self::Error(s) => format!("Error: {s}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PulseHiveError;

    #[test]
    fn test_tool_is_object_safe() {
        fn _assert_object_safe(_: &dyn Tool) {}
        fn _assert_boxable(_: Box<dyn Tool>) {}
    }

    // Mock tool for testing
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes input back"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
            let text = params["text"]
                .as_str()
                .ok_or_else(|| PulseHiveError::validation("text required"))?;
            Ok(ToolResult::text(text))
        }
    }

    #[test]
    fn test_mock_tool_metadata() {
        let tool = EchoTool;
        assert_eq!(tool.name(), "echo");
        assert_eq!(tool.description(), "Echoes input back");
        assert!(!tool.requires_approval());
    }

    #[test]
    fn test_tool_result_constructors() {
        let text = ToolResult::text("hello");
        assert!(matches!(text, ToolResult::Text(s) if s == "hello"));

        let json = ToolResult::json(serde_json::json!({"key": "value"}));
        assert!(matches!(json, ToolResult::Json(_)));

        let err = ToolResult::error("not found");
        assert!(matches!(err, ToolResult::Error(s) if s == "not found"));
    }

    #[test]
    fn test_tool_result_to_content() {
        assert_eq!(ToolResult::text("hello").to_content(), "hello");
        assert_eq!(
            ToolResult::json(serde_json::json!({"a": 1})).to_content(),
            r#"{"a":1}"#
        );
        assert_eq!(ToolResult::error("oops").to_content(), "Error: oops");
    }
}
