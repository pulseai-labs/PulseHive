//! Internal types matching the OpenAI chat completions API schema.
//!
//! These types are not part of the public API — they handle serialization
//! to/from OpenAI's specific JSON format. The public API uses pulsehive-core types.

// Types are defined here but used in tickets #13-14 (chat/streaming implementation)
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

use pulsehive_core::llm::{LlmChunk, LlmResponse, TokenUsage, ToolCall};

/// Request body for POST /chat/completions
#[derive(Debug, Serialize)]
pub(crate) struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Value>, // Pre-serialized Message values
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAITool>,
    pub temperature: f32,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
}

/// OpenAI tool definition wrapper: `{"type": "function", "function": {...}}`
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAITool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunction,
}

/// Function definition inside an OpenAI tool
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAIFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl OpenAITool {
    /// Creates an OpenAI tool definition from a PulseHive ToolDefinition.
    pub fn from_tool_def(def: &pulsehive_core::llm::ToolDefinition) -> Self {
        Self {
            tool_type: "function".into(),
            function: OpenAIFunction {
                name: def.name.clone(),
                description: def.description.clone(),
                parameters: def.parameters.clone(),
            },
        }
    }
}

/// Response body from POST /chat/completions (non-streaming)
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionResponse {
    #[allow(dead_code)]
    pub id: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Option<OpenAIUsage>,
}

/// A single choice in the response
#[derive(Debug, Deserialize)]
pub(crate) struct ChatChoice {
    pub message: ChatMessage,
    #[allow(dead_code)]
    pub finish_reason: Option<String>,
}

/// The message object inside a choice
#[derive(Debug, Deserialize)]
pub(crate) struct ChatMessage {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
}

/// Tool call as returned by OpenAI
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIToolCall {
    pub id: String,
    #[allow(dead_code)]
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: OpenAIFunctionCall,
}

/// Function call details — NOTE: arguments is a JSON STRING, not parsed JSON
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIFunctionCall {
    pub name: String,
    /// Arguments as a JSON string (OpenAI does NOT send parsed JSON here)
    pub arguments: String,
}

/// Token usage from OpenAI response
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

// ── Streaming types ──────────────────────────────────────────────────

/// A single SSE chunk from streaming response
#[derive(Debug, Deserialize)]
pub(crate) struct StreamChunk {
    pub choices: Vec<StreamChoice>,
}

/// A choice in a streaming chunk
#[derive(Debug, Deserialize)]
pub(crate) struct StreamChoice {
    pub delta: StreamDelta,
    #[allow(dead_code)]
    pub finish_reason: Option<String>,
}

/// The delta object in a streaming choice
#[derive(Debug, Deserialize)]
pub(crate) struct StreamDelta {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<StreamToolCall>>,
}

/// Tool call delta in streaming
#[derive(Debug, Deserialize)]
pub(crate) struct StreamToolCall {
    pub index: usize,
    pub id: Option<String>,
    pub function: Option<StreamFunctionCall>,
}

/// Function call delta in streaming
#[derive(Debug, Deserialize)]
pub(crate) struct StreamFunctionCall {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

// ── Conversion helpers ───────────────────────────────────────────────

impl ChatCompletionResponse {
    /// Converts the OpenAI response into PulseHive's LlmResponse.
    pub fn into_llm_response(self) -> LlmResponse {
        let choice = self.choices.into_iter().next();
        let (content, tool_calls) = match choice {
            Some(c) => {
                let tool_calls = c
                    .message
                    .tool_calls
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tc| {
                        // Parse arguments string into Value
                        let args = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(Value::Object(serde_json::Map::new()));
                        ToolCall {
                            id: tc.id,
                            name: tc.function.name,
                            arguments: args,
                        }
                    })
                    .collect();
                (c.message.content, tool_calls)
            }
            None => (None, vec![]),
        };

        let usage = self.usage.map_or(TokenUsage::default(), |u| TokenUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
        });

        LlmResponse {
            content,
            tool_calls,
            usage,
        }
    }
}

impl StreamDelta {
    /// Converts a streaming delta into LlmChunk(s).
    pub fn into_chunks(self) -> Vec<LlmChunk> {
        let mut chunks = vec![];

        if let Some(text) = self.content {
            if !text.is_empty() {
                chunks.push(LlmChunk::Text(text));
            }
        }

        if let Some(tool_calls) = self.tool_calls {
            for tc in tool_calls {
                if let Some(func) = tc.function {
                    if let (Some(id), Some(name)) = (tc.id, func.name) {
                        // First appearance — tool call start
                        chunks.push(LlmChunk::ToolCallStart { id, name });
                    } else if let Some(args) = func.arguments {
                        if !args.is_empty() {
                            // Subsequent — arguments delta (id may not be present)
                            chunks.push(LlmChunk::ToolCallDelta {
                                id: String::new(), // filled by caller tracking state
                                arguments_delta: args,
                            });
                        }
                    }
                }
            }
        }

        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_tool_from_tool_def() {
        let def = pulsehive_core::llm::ToolDefinition {
            name: "search".into(),
            description: "Search code".into(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let tool = OpenAITool::from_tool_def(&def);
        assert_eq!(tool.tool_type, "function");
        assert_eq!(tool.function.name, "search");

        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "search");
    }

    #[test]
    fn test_chat_completion_response_parsing() {
        let json = r#"{
            "id": "chatcmpl-abc",
            "choices": [{
                "message": {
                    "content": "Hello!",
                    "tool_calls": null
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5
            }
        }"#;

        let response: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        let llm_response = response.into_llm_response();
        assert_eq!(llm_response.content, Some("Hello!".into()));
        assert!(llm_response.tool_calls.is_empty());
        assert_eq!(llm_response.usage.input_tokens, 10);
        assert_eq!(llm_response.usage.output_tokens, 5);
    }

    #[test]
    fn test_chat_completion_with_tool_calls() {
        let json = r#"{
            "id": "chatcmpl-abc",
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\": \"config.toml\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 20
            }
        }"#;

        let response: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        let llm_response = response.into_llm_response();
        assert!(llm_response.content.is_none());
        assert_eq!(llm_response.tool_calls.len(), 1);
        assert_eq!(llm_response.tool_calls[0].name, "read_file");
        // Arguments parsed from string into Value
        assert_eq!(llm_response.tool_calls[0].arguments["path"], "config.toml");
    }

    #[test]
    fn test_stream_delta_text() {
        let delta = StreamDelta {
            content: Some("Hello".into()),
            tool_calls: None,
        };
        let chunks = delta.into_chunks();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], LlmChunk::Text(t) if t == "Hello"));
    }

    #[test]
    fn test_stream_delta_tool_call_start() {
        let delta = StreamDelta {
            content: None,
            tool_calls: Some(vec![StreamToolCall {
                index: 0,
                id: Some("call_1".into()),
                function: Some(StreamFunctionCall {
                    name: Some("search".into()),
                    arguments: None,
                }),
            }]),
        };
        let chunks = delta.into_chunks();
        assert_eq!(chunks.len(), 1);
        assert!(
            matches!(&chunks[0], LlmChunk::ToolCallStart { id, name } if id == "call_1" && name == "search")
        );
    }

    #[test]
    fn test_request_serialization() {
        let req = ChatCompletionRequest {
            model: "gpt-4".into(),
            messages: vec![serde_json::json!({"role": "user", "content": "hi"})],
            tools: vec![],
            temperature: 0.7,
            max_tokens: 100,
            stream: false,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "gpt-4");
        assert!(json.get("tools").is_none()); // skipped when empty
        assert!(json.get("stream").is_none()); // skipped when false
    }
}
