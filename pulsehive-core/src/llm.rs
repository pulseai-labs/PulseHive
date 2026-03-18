//! LLM provider abstraction and message types.
//!
//! [`LlmProvider`] is the trait that provider crates (`pulsehive-openai`, `pulsehive-anthropic`)
//! implement. Products can also implement custom providers.
//!
//! The message types follow the standard tool-use conversation pattern:
//! System → User → Assistant (with optional tool_calls) → ToolResult → Assistant → ...

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// LLM model selection and generation parameters.
///
/// The `provider` field routes to a named [`LlmProvider`] instance registered
/// with the HiveMind builder (e.g., `"openai"`, `"anthropic"`).
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
}

impl LlmConfig {
    /// Creates a config with sensible defaults (temperature 0.7, max_tokens 4096).
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            temperature: 0.7,
            max_tokens: 4096,
        }
    }
}

/// A message in a multi-turn conversation.
///
/// Follows the standard tool-use conversation pattern used by OpenAI and Anthropic APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    /// System prompt that configures agent behavior.
    System { content: String },
    /// User message (task description, follow-up, etc.).
    User { content: String },
    /// Assistant response, optionally with tool calls.
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    /// Result of a tool execution, sent back to the LLM.
    ToolResult {
        tool_call_id: String,
        content: String,
    },
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

/// Token usage statistics from an LLM call.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    /// Tokens consumed by the input (prompt + context).
    pub input_tokens: u32,
    /// Tokens generated in the output.
    pub output_tokens: u32,
}

/// Complete response from a non-streaming LLM call.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// Text content of the response (`None` if only tool calls).
    pub content: Option<String>,
    /// Tool calls requested by the LLM (empty if text-only response).
    pub tool_calls: Vec<ToolCall>,
    /// Token usage statistics.
    pub usage: TokenUsage,
}

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
        // This verifies the trait can be used as a trait object
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

    #[test]
    fn test_message_system_serde_roundtrip() {
        let msg = Message::System {
            content: "You are a helpful agent.".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(deserialized, Message::System { content } if content == "You are a helpful agent.")
        );
    }

    #[test]
    fn test_message_assistant_with_tool_calls() {
        let msg = Message::Assistant {
            content: None,
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "/tmp/test.txt"}),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        match deserialized {
            Message::Assistant {
                content,
                tool_calls,
            } => {
                assert!(content.is_none());
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].name, "read_file");
            }
            _ => panic!("Expected Assistant variant"),
        }
    }

    #[test]
    fn test_message_tool_result_serde() {
        let msg = Message::ToolResult {
            tool_call_id: "call_1".into(),
            content: "File contents here".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("call_1"));
        assert!(json.contains("File contents here"));
    }

    #[test]
    fn test_multi_turn_conversation() {
        let conversation = [
            Message::System {
                content: "You are a code assistant.".into(),
            },
            Message::User {
                content: "Read the config file.".into(),
            },
            Message::Assistant {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "config.toml"}),
                }],
            },
            Message::ToolResult {
                tool_call_id: "call_1".into(),
                content: "[package]\nname = \"test\"".into(),
            },
            Message::Assistant {
                content: Some("The config file defines a package named 'test'.".into()),
                tool_calls: vec![],
            },
        ];
        assert_eq!(conversation.len(), 5);
    }

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
