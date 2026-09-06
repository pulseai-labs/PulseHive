//! LLM provider abstraction and message types.
//!
//! [`LlmProvider`] is the trait that provider crates (`pulsehive-openai`, `pulsehive-anthropic`)
//! implement. Products can also implement custom providers.
//!
//! Messages serialize to OpenAI's chat completions format:
//! ```json
//! {"role": "system", "content": "You are helpful"}
//! {"role": "user", "content": "Hello"}
//! {"role": "assistant", "content": null, "tool_calls": [...]}
//! {"role": "tool", "tool_call_id": "call_1", "content": "result"}
//! ```

use std::fmt;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::tool::Tool;

/// LLM model selection and generation parameters.
///
/// The `provider` field routes to a named [`LlmProvider`] instance registered
/// with the HiveMind builder (e.g., `"openai"`, `"anthropic"`).
///
/// The type is non-exhaustive, so later fields are additive. Construct it with
/// [`LlmConfig::new`] and the `with_*` builders — a struct literal outside this
/// crate no longer compiles:
///
/// ```compile_fail,E0639
/// let config = pulsehive_core::llm::LlmConfig {
///     provider: "openai".into(),
///     model: "gpt-4o".into(),
///     temperature: 0.7,
///     max_tokens: 4096,
///     timeout_secs: None,
///     max_retries: None,
///     reasoning_effort: None,
///     tool_choice: None,
///     cancel: None,
/// };
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Provider name — matches the key used in `HiveMind::builder().llm_provider(name, ...)`.
    pub provider: String,
    /// Model identifier (e.g., `"claude-sonnet-4-6"`, `"gpt-4"`, `"glm-5"`).
    pub model: String,
    /// Sampling temperature (0.0 = deterministic, 1.0+ = creative).
    pub temperature: f32,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Per-call override of the provider's configured request timeout, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Per-call override of the provider's configured retry budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Reasoning budget hint for models that expose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Constraint on whether and which tool the model may call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Cancellation carrier for the in-flight call. Runtime state, never wire
    /// state — it is skipped by serde in both directions.
    #[serde(skip)]
    pub cancel: Option<CancellationToken>,
}

impl LlmConfig {
    /// Creates a config with sensible defaults (temperature 0.7, max_tokens 4096).
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            temperature: 0.7,
            max_tokens: 4096,
            timeout_secs: None,
            max_retries: None,
            reasoning_effort: None,
            tool_choice: None,
            cancel: None,
        }
    }

    /// Sets the sampling temperature.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }

    /// Sets the maximum number of tokens to generate.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Overrides the provider's configured request timeout for this call.
    pub fn with_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = Some(timeout_secs);
        self
    }

    /// Overrides the provider's configured retry budget for this call.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Sets the reasoning budget hint.
    pub fn with_reasoning_effort(mut self, reasoning_effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(reasoning_effort);
        self
    }

    /// Sets the tool-choice constraint.
    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }

    /// Attaches a cancellation token to this call.
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// Reasoning budget hint for models that expose one.
///
/// Providers map this to their own wire representation; the mapping is not part
/// of this contract.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// Least reasoning the model will do.
    Minimal,
    /// Low reasoning budget.
    Low,
    /// Balanced reasoning budget.
    Medium,
    /// Highest reasoning budget.
    High,
}

/// Constraint on whether and which tool the model may call.
///
/// The exact provider wire object (OpenAI's
/// `{"type":"function","function":{"name":…}}`, for one) is the provider crate's
/// mapping, not this enum's serde form.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    /// The model decides whether to call a tool.
    Auto,
    /// The model must not call a tool.
    None,
    /// The model must call some tool.
    Required,
    /// The model must call this specific tool.
    Function { name: String },
}

/// A message in a multi-turn conversation.
///
/// Serializes to OpenAI's chat completions format with `"role"` as the discriminator.
/// This format is compatible with OpenAI, GLM, vLLM, Ollama, and other OpenAI-compatible APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// System prompt that configures agent behavior.
    /// Serializes as: `{"role": "system", "content": "..."}`
    System { content: String },

    /// User message (task description, follow-up, etc.).
    /// Serializes as: `{"role": "user", "content": "..."}`
    User { content: String },

    /// Assistant response, optionally with tool calls.
    /// Serializes as: `{"role": "assistant", "content": "...", "tool_calls": [...]}`
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },

    /// Result of a tool execution, sent back to the LLM.
    /// Serializes as: `{"role": "tool", "tool_call_id": "...", "content": "..."}`
    #[serde(rename = "tool")]
    ToolResult {
        tool_call_id: String,
        content: String,
    },
}

impl Message {
    /// Creates a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    /// Creates a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    /// Creates an assistant message with text content (no tool calls).
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: Some(content.into()),
            tool_calls: vec![],
        }
    }

    /// Creates an assistant message with tool calls (no text content).
    pub fn assistant_with_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self::Assistant {
            content: None,
            tool_calls,
        }
    }

    /// Creates a tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
        }
    }
}

/// An LLM's request to invoke a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this tool call (used to match with ToolResult).
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// JSON arguments parsed from the LLM's output.
    pub arguments: Value,
}

/// Tool schema sent to the LLM so it knows what tools are available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name (must match `Tool::name()`).
    pub name: String,
    /// Description the LLM uses to decide when to invoke this tool.
    pub description: String,
    /// JSON Schema describing the tool's parameters.
    pub parameters: Value,
}

impl ToolDefinition {
    /// Creates a ToolDefinition from a Tool trait object.
    ///
    /// Extracts name, description, and parameters from the tool for sending to the LLM.
    pub fn from_tool(tool: &dyn Tool) -> Self {
        Self {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            parameters: tool.parameters(),
        }
    }
}

/// Token usage statistics from an LLM call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Tokens consumed by the input (prompt + context).
    pub input_tokens: u32,
    /// Tokens generated in the output.
    pub output_tokens: u32,
}

/// Complete response from a non-streaming LLM call.
///
/// The type is non-exhaustive, so later fields are additive. Construct it with
/// [`LlmResponse::new`] or [`LlmResponse::text`] and the `with_*` builders — a
/// struct literal outside this crate no longer compiles:
///
/// ```compile_fail,E0639
/// let response = pulsehive_core::llm::LlmResponse {
///     content: Some("hi".into()),
///     tool_calls: vec![],
///     usage: pulsehive_core::llm::TokenUsage::default(),
///     finish_reason: None,
///     reasoning: None,
/// };
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// Text content of the response (`None` if only tool calls).
    pub content: Option<String>,
    /// Tool calls requested by the LLM (empty if text-only response).
    pub tool_calls: Vec<ToolCall>,
    /// Token usage statistics.
    pub usage: TokenUsage,
    /// Why the model stopped, verbatim as the provider reported it.
    pub finish_reason: Option<String>,
    /// Reasoning trace, when the provider returned one.
    pub reasoning: Option<String>,
}

impl LlmResponse {
    /// Creates a response from the three fields every provider fills.
    pub fn new(content: Option<String>, tool_calls: Vec<ToolCall>, usage: TokenUsage) -> Self {
        Self {
            content,
            tool_calls,
            usage,
            finish_reason: None,
            reasoning: None,
        }
    }

    /// Creates a text-only response with no tool calls and no usage reported.
    pub fn text(content: impl Into<String>) -> Self {
        Self::new(Some(content.into()), vec![], TokenUsage::default())
    }

    /// Sets the tool calls.
    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Sets the token usage.
    pub fn with_usage(mut self, usage: TokenUsage) -> Self {
        self.usage = usage;
        self
    }

    /// Sets the provider's verbatim finish reason.
    pub fn with_finish_reason(mut self, finish_reason: impl Into<String>) -> Self {
        self.finish_reason = Some(finish_reason.into());
        self
    }

    /// Sets the reasoning trace.
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reasoning = Some(reasoning.into());
        self
    }
}

/// What went wrong on the transport between PulseHive and an LLM provider.
///
/// Whether a kind is worth retrying is provider policy, not part of this
/// contract.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmErrorKind {
    /// The call exceeded its deadline.
    Timeout,
    /// The connection could not be established.
    Connect,
    /// The provider rate-limited the call.
    RateLimited,
    /// The provider returned a 5xx.
    ServerError,
    /// The provider returned a 4xx that is not a rate limit.
    ClientError,
    /// The response body could not be parsed.
    Parse,
    /// The model emitted a tool call that could not be read as one.
    MalformedToolCall,
    /// The caller cancelled the in-flight call.
    Cancelled,
}

impl LlmErrorKind {
    /// The snake_case name, identical to this kind's serde representation.
    fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::RateLimited => "rate_limited",
            Self::ServerError => "server_error",
            Self::ClientError => "client_error",
            Self::Parse => "parse",
            Self::MalformedToolCall => "malformed_tool_call",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A structured transport failure from an LLM provider.
///
/// This replaces stringly-typed transport errors: the caller can branch on
/// [`LlmErrorKind`] instead of matching on message text.
///
/// The type is non-exhaustive, so later fields are additive. Construct it with
/// [`LlmError::new`] and the `with_*` builders — a struct literal outside this
/// crate no longer compiles:
///
/// ```compile_fail,E0639
/// let err = pulsehive_core::llm::LlmError {
///     kind: pulsehive_core::llm::LlmErrorKind::Timeout,
///     message: "deadline".into(),
///     status: None,
///     attempts: 1,
///     body: None,
///     finish_reason: None,
///     retry_after: None,
/// };
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmError {
    /// What class of failure this is.
    pub kind: LlmErrorKind,
    /// Human-readable description.
    pub message: String,
    /// HTTP status, when the failure carried one.
    pub status: Option<u16>,
    /// How many attempts were made before giving up (at least 1).
    pub attempts: u32,
    /// Response body, verbatim. The consumer decides what to redact.
    pub body: Option<String>,
    /// The finish reason that accompanied the failure — set for
    /// [`LlmErrorKind::MalformedToolCall`].
    pub finish_reason: Option<String>,
    /// How long the provider asked the caller to wait before retrying.
    pub retry_after: Option<Duration>,
}

impl LlmError {
    /// Creates an error for a single attempt with no transport detail attached.
    pub fn new(kind: LlmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status: None,
            attempts: 1,
            body: None,
            finish_reason: None,
            retry_after: None,
        }
    }

    /// Sets the HTTP status.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Sets how many attempts were made.
    pub fn with_attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts;
        self
    }

    /// Sets the verbatim response body.
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Sets the accompanying finish reason.
    pub fn with_finish_reason(mut self, finish_reason: impl Into<String>) -> Self {
        self.finish_reason = Some(finish_reason.into());
        self
    }

    /// Sets the provider's requested retry delay.
    pub fn with_retry_after(mut self, retry_after: Duration) -> Self {
        self.retry_after = Some(retry_after);
        self
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} after {} attempt(s): {}",
            self.kind.as_str(),
            self.attempts,
            self.message
        )?;
        if let Some(status) = self.status {
            write!(f, " (HTTP {status})")?;
        }
        Ok(())
    }
}

impl std::error::Error for LlmError {}

/// A chunk from a streaming LLM response.
#[derive(Debug, Clone)]
pub enum LlmChunk {
    /// A text token delta.
    Text(String),
    /// Start of a tool call (id and name known).
    ToolCallStart { id: String, name: String },
    /// Incremental arguments for an in-progress tool call.
    ToolCallDelta { id: String, arguments_delta: String },
    /// Stream is complete.
    Done,
}

/// Trait for LLM provider implementations.
///
/// Provider crates (`pulsehive-openai`, `pulsehive-anthropic`) implement this trait.
/// Products can also implement custom providers for self-hosted models.
///
/// Must be `Send + Sync` for use across Tokio tasks and object-safe for
/// `Arc<dyn LlmProvider>` in HiveMind.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat completion request and return the full response.
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<LlmResponse>;

    /// Send a chat completion request and stream tokens.
    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk>> + Send>>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_provider_is_object_safe() {
        fn _assert_object_safe(_: &dyn LlmProvider) {}
    }

    #[test]
    fn test_llm_config_new_defaults() {
        let config = LlmConfig::new("openai", "gpt-4");
        assert_eq!(config.provider, "openai");
        assert_eq!(config.model, "gpt-4");
        assert!((config.temperature - 0.7).abs() < f32::EPSILON);
        assert_eq!(config.max_tokens, 4096);
    }

    #[test]
    fn test_llm_config_serialization() {
        let config = LlmConfig::new("anthropic", "claude-sonnet-4-6");
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: LlmConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.provider, "anthropic");
        assert_eq!(deserialized.model, "claude-sonnet-4-6");
    }

    // ── OpenAI format serialization tests ────────────────────────────

    #[test]
    fn test_message_system_openai_format() {
        let msg = Message::system("You are helpful");
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "system");
        assert_eq!(json["content"], "You are helpful");
        assert_eq!(json.as_object().unwrap().len(), 2); // only role + content
    }

    #[test]
    fn test_message_user_openai_format() {
        let msg = Message::user("Hello");
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "Hello");
    }

    #[test]
    fn test_message_assistant_text_only_format() {
        let msg = Message::assistant("The answer is 42");
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["content"], "The answer is 42");
        // tool_calls should be absent (skip_serializing_if empty)
        assert!(json.get("tool_calls").is_none());
    }

    #[test]
    fn test_message_assistant_with_tool_calls_format() {
        let msg = Message::assistant_with_tool_calls(vec![ToolCall {
            id: "call_abc".into(),
            name: "search".into(),
            arguments: serde_json::json!({"query": "rust"}),
        }]);
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "assistant");
        assert!(json["content"].is_null());
        assert_eq!(json["tool_calls"][0]["id"], "call_abc");
        assert_eq!(json["tool_calls"][0]["name"], "search");
    }

    #[test]
    fn test_message_tool_result_openai_format() {
        let msg = Message::tool_result("call_abc", "Search results here");
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "tool"); // NOT "tool_result"
        assert_eq!(json["tool_call_id"], "call_abc");
        assert_eq!(json["content"], "Search results here");
    }

    #[test]
    fn test_message_serde_roundtrip_all_variants() {
        let messages = [
            Message::system("Be helpful"),
            Message::user("Hi"),
            Message::assistant("Hello!"),
            Message::assistant_with_tool_calls(vec![ToolCall {
                id: "c1".into(),
                name: "read".into(),
                arguments: serde_json::json!({}),
            }]),
            Message::tool_result("c1", "file contents"),
        ];

        for msg in &messages {
            let json = serde_json::to_string(msg).unwrap();
            let deserialized: Message = serde_json::from_str(&json).unwrap();
            // Verify roundtrip produces same JSON
            let json2 = serde_json::to_string(&deserialized).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn test_message_deserialize_from_openai_response() {
        // Simulate what we'd get back from OpenAI's API
        let openai_json = r#"{"role": "assistant", "content": "Hello!", "tool_calls": []}"#;
        let msg: Message = serde_json::from_str(openai_json).unwrap();
        assert!(matches!(msg, Message::Assistant { content: Some(c), .. } if c == "Hello!"));
    }

    #[test]
    fn test_message_deserialize_assistant_without_tool_calls() {
        // OpenAI sometimes omits tool_calls entirely
        let openai_json = r#"{"role": "assistant", "content": "Hello!"}"#;
        let msg: Message = serde_json::from_str(openai_json).unwrap();
        match msg {
            Message::Assistant {
                content,
                tool_calls,
            } => {
                assert_eq!(content, Some("Hello!".into()));
                assert!(tool_calls.is_empty()); // default
            }
            _ => panic!("Expected Assistant"),
        }
    }

    // ── Convenience constructor tests ────────────────────────────────

    #[test]
    fn test_message_convenience_constructors() {
        assert!(matches!(Message::system("x"), Message::System { content } if content == "x"));
        assert!(matches!(Message::user("y"), Message::User { content } if content == "y"));
        assert!(
            matches!(Message::assistant("z"), Message::Assistant { content: Some(c), tool_calls } if c == "z" && tool_calls.is_empty())
        );
        assert!(
            matches!(Message::assistant_with_tool_calls(vec![]), Message::Assistant { content: None, tool_calls } if tool_calls.is_empty())
        );
        assert!(
            matches!(Message::tool_result("id", "res"), Message::ToolResult { tool_call_id, content } if tool_call_id == "id" && content == "res")
        );
    }

    // ── ToolDefinition tests ─────────────────────────────────────────

    #[test]
    fn test_tool_definition_from_tool() {
        use crate::error::PulseHiveError;
        use crate::tool::{ToolContext, ToolResult};

        struct MockTool;

        #[async_trait]
        impl Tool for MockTool {
            fn name(&self) -> &str {
                "mock_tool"
            }
            fn description(&self) -> &str {
                "A mock tool for testing"
            }
            fn parameters(&self) -> Value {
                serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}})
            }
            async fn execute(
                &self,
                _params: Value,
                _ctx: &ToolContext,
            ) -> std::result::Result<ToolResult, PulseHiveError> {
                Ok(ToolResult::text("ok"))
            }
        }

        let def = ToolDefinition::from_tool(&MockTool);
        assert_eq!(def.name, "mock_tool");
        assert_eq!(def.description, "A mock tool for testing");
        assert_eq!(def.parameters["type"], "object");
    }

    #[test]
    fn test_multi_turn_conversation_serialization() {
        let conversation = [
            Message::system("You are a code assistant."),
            Message::user("Read the config file."),
            Message::assistant_with_tool_calls(vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "config.toml"}),
            }]),
            Message::tool_result("call_1", "[package]\nname = \"test\""),
            Message::assistant("The config file defines a package named 'test'."),
        ];

        // Verify all serialize to valid JSON with role field
        for msg in &conversation {
            let json = serde_json::to_value(msg).unwrap();
            assert!(json.get("role").is_some(), "Missing role field");
        }
        assert_eq!(conversation.len(), 5);
    }

    // ── Other type tests ─────────────────────────────────────────────

    #[test]
    fn test_tool_definition_construction() {
        let tool = ToolDefinition {
            name: "search".into(),
            description: "Search the codebase".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
        };
        assert_eq!(tool.name, "search");
    }

    #[test]
    fn test_token_usage_default() {
        let usage = TokenUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }

    #[test]
    fn test_llm_chunk_variants() {
        let text = LlmChunk::Text("hello".into());
        assert!(matches!(text, LlmChunk::Text(s) if s == "hello"));

        let start = LlmChunk::ToolCallStart {
            id: "1".into(),
            name: "search".into(),
        };
        assert!(matches!(start, LlmChunk::ToolCallStart { .. }));

        let delta = LlmChunk::ToolCallDelta {
            id: "1".into(),
            arguments_delta: "{\"q".into(),
        };
        assert!(matches!(delta, LlmChunk::ToolCallDelta { .. }));

        let done = LlmChunk::Done;
        assert!(matches!(done, LlmChunk::Done));
    }
}
