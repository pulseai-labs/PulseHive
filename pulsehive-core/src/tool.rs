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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

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

    /// If this tool supports streaming execution, return `Some(self)` as a
    /// [`StreamingTool`]; otherwise `None`.
    ///
    /// This is an object-safe capability probe — no `Any`, no `unsafe`, no second
    /// registry. Non-streaming tools use the `None` default unchanged; a streaming
    /// tool overrides this single method to return `Some(self)`. The agent loop
    /// (v2.1.0) dispatches on this to decide whether to open a progress channel.
    fn as_streaming(&self) -> Option<&dyn StreamingTool> {
        None
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

/// Severity level for a streaming tool [`ToolProgress::Log`] line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Fine-grained diagnostic detail.
    Debug,
    /// Normal informational progress.
    Info,
    /// A recoverable concern worth surfacing.
    Warn,
    /// A failure the consumer should see.
    Error,
}

/// A progress event emitted by a streaming tool during execution.
///
/// Tools implementing [`StreamingTool`] push these over an [`mpsc::Sender`]. The
/// agent loop (v2.1.0) forwards each one as a `HiveEvent::ToolProgress`. The
/// `Started` / `Completed` bookends are emitted by the loop, not by tool bodies.
///
/// Serializes to tagged JSON: `{"kind": "progress", "fraction": 0.5, ...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolProgress {
    /// Tool has begun. Emitted automatically by the loop before the tool body runs.
    Started {
        /// Optional caller estimate of total duration, for UI ETA rendering.
        estimated_duration_ms: Option<u64>,
    },
    /// Fractional progress in `0.0..=1.0`, with an optional human-readable label.
    Progress {
        /// Completion fraction in `0.0..=1.0`.
        fraction: f32,
        /// Optional human-readable status label.
        message: Option<String>,
    },
    /// A partial result available before the tool completes (e.g. the first 100
    /// trades of a backtest, one sweep combination's score).
    PartialResult {
        /// Untyped JSON so partial results from all tools aggregate on one stream.
        payload: Value,
    },
    /// A log line surfaced to the consumer's session timeline.
    Log {
        /// Severity of the log line.
        level: LogLevel,
        /// The log message.
        message: String,
    },
    /// Tool finished. Emitted automatically by the loop after the tool body returns.
    Completed {
        /// Total execution duration in milliseconds.
        duration_ms: u64,
    },
}

/// Opt-in extension for tools that report progress during execution.
///
/// Tools that implement only [`Tool`] are still fully supported: the agent loop
/// wraps them so they emit `Started` → `Completed` with no intermediate events.
/// Implement this trait when a tool is long-running and the consumer wants live
/// feedback (progress bars, partial results, log streams).
///
/// `StreamingTool: Tool` — every streaming tool is also a regular [`Tool`], so it
/// can be stored as `Arc<dyn Tool>` and registered the same way. A streaming tool
/// exposes itself by overriding [`Tool::as_streaming`] to return `Some(self)`.
#[async_trait]
pub trait StreamingTool: Tool {
    /// Execute the tool, pushing [`ToolProgress`] events as work proceeds.
    ///
    /// `progress_tx` is a bounded channel owned by the agent loop. Implementations
    /// SHOULD send `Progress` / `PartialResult` / `Log` events; they MUST NOT send
    /// `Started` or `Completed` (the loop emits those as bookends). Returns the
    /// final [`ToolResult`] after the stream is drained. If the receiver is dropped
    /// (consumer gone), `progress_tx.send().await` errors — implementations SHOULD
    /// treat that as a soft signal, keep computing, and return the result anyway.
    ///
    /// **Progress is observability, not control.** `progress_tx` is the *internal*
    /// loop→forwarder channel — NOT the consumer's `HiveMind::deploy()` stream. A
    /// consumer dropping the `deploy()` stream does **not** cancel this tool or stop
    /// it computing; cooperative cancellation is a separate mechanism (a future
    /// release). The channel is bounded, so `send().await` can apply backpressure if
    /// the loop's forwarder falls behind — treat progress as best-effort telemetry,
    /// and prefer coalescing high-frequency updates rather than relying on every
    /// send being delivered. Do NOT retain or clone `progress_tx` beyond this call:
    /// the loop closes the channel by observing your returned future drop it, and a
    /// leaked/cloned sender keeps the forwarder alive.
    async fn execute_streaming(
        &self,
        params: Value,
        context: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PulseHiveError;

    #[test]
    fn test_tool_is_object_safe() {
        fn _assert_object_safe(_: &dyn Tool) {}
        fn _assert_boxable(_: Box<dyn Tool>) {}
        fn _assert_arcable(_: Arc<dyn Tool>) {}
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

    // Mock streaming tool that ignores its context and pushes a fixed
    // Progress(0.5) → PartialResult → Progress(1.0) sequence over the channel.
    struct MockStreamingTool;

    #[async_trait]
    impl Tool for MockStreamingTool {
        fn name(&self) -> &str {
            "mock_streaming"
        }
        fn description(&self) -> &str {
            "mock streaming tool for tests"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
            Ok(ToolResult::text("final"))
        }
        fn as_streaming(&self) -> Option<&dyn StreamingTool> {
            Some(self)
        }
    }

    #[async_trait]
    impl StreamingTool for MockStreamingTool {
        async fn execute_streaming(
            &self,
            _params: Value,
            _context: &ToolContext,
            progress_tx: mpsc::Sender<ToolProgress>,
        ) -> Result<ToolResult> {
            // Dropped receiver is a soft signal: ignore send errors, keep going.
            let _ = progress_tx
                .send(ToolProgress::Progress {
                    fraction: 0.5,
                    message: None,
                })
                .await;
            let _ = progress_tx
                .send(ToolProgress::PartialResult {
                    payload: serde_json::json!({"n": 1}),
                })
                .await;
            let _ = progress_tx
                .send(ToolProgress::Progress {
                    fraction: 1.0,
                    message: Some("done".into()),
                })
                .await;
            Ok(ToolResult::text("final"))
        }
    }

    /// Builds a minimal `ToolContext`. The substrate/emitter are never touched by
    /// the mock tool; a temp `Config::default()` PulseDB avoids the ONNX path.
    fn test_context() -> ToolContext {
        let dir = tempfile::tempdir().unwrap();
        let db =
            pulsedb::PulseDB::open(dir.path().join("test.db"), pulsedb::Config::default()).unwrap();
        // Leak the tempdir so its files outlive this context.
        Box::leak(Box::new(dir));
        ToolContext {
            agent_id: "agent-test".into(),
            collective_id: CollectiveId::new(),
            substrate: Arc::new(pulsedb::PulseDBSubstrate::from_db(db)),
            event_emitter: EventEmitter::default(),
        }
    }

    #[tokio::test]
    async fn streaming_tool_progress_channel_order() {
        let ctx = test_context();
        let tool = MockStreamingTool;

        // Capability probe: a streaming tool exposes itself via `as_streaming`.
        assert!(tool.as_streaming().is_some());

        // Capacity > number of sends so `execute_streaming` never blocks on a
        // receiver we only drain after it returns.
        let (tx, mut rx) = mpsc::channel::<ToolProgress>(8);
        let result = tool.execute_streaming(Value::Null, &ctx, tx).await.unwrap();

        // Drain the channel; the sender is dropped once `execute_streaming` returns.
        let mut events = Vec::new();
        while let Some(p) = rx.recv().await {
            events.push(p);
        }

        assert_eq!(events.len(), 3, "expected 3 progress events in order");
        match &events[0] {
            ToolProgress::Progress { fraction, message } => {
                assert!((*fraction - 0.5).abs() < f32::EPSILON);
                assert!(message.is_none());
            }
            other => panic!("event[0] expected Progress(0.5), got {other:?}"),
        }
        assert!(matches!(events[1], ToolProgress::PartialResult { .. }));
        match &events[2] {
            ToolProgress::Progress { fraction, message } => {
                assert!((*fraction - 1.0).abs() < f32::EPSILON);
                assert_eq!(message.as_deref(), Some("done"));
            }
            other => panic!("event[2] expected Progress(1.0), got {other:?}"),
        }

        // `execute_streaming` returns the final `ToolResult`.
        assert!(matches!(result, ToolResult::Text(s) if s == "final"));
    }
}
