//! Anthropic Messages API request and response types.
//!
//! Handles conversion between PulseHive's `Message` format and Anthropic's
//! content-block-based format. Key differences from OpenAI:
//! - System prompt is a top-level field, not in messages
//! - Content is an array of typed blocks (text, tool_use, tool_result)
//! - Tool results are sent as user messages with tool_result blocks

use serde::{Deserialize, Serialize};
use serde_json::Value;

use pulsehive_core::llm::{LlmResponse, Message, TokenUsage, ToolCall, ToolDefinition};

// ── Request Types ────────────────────────────────────────────────────

/// Top-level request to the Anthropic Messages API.
#[derive(Debug, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AnthropicTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

/// A message in Anthropic format.
#[derive(Debug, Serialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: AnthropicContent,
}

/// Content can be a simple string or an array of content blocks.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum AnthropicContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// A content block in a message.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// Tool definition in Anthropic format.
#[derive(Debug, Serialize)]
pub struct AnthropicTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl From<&ToolDefinition> for AnthropicTool {
    fn from(td: &ToolDefinition) -> Self {
        Self {
            name: td.name.clone(),
            description: td.description.clone(),
            input_schema: td.parameters.clone(),
        }
    }
}

// ── Response Types ───────────────────────────────────────────────────

/// Response from the Anthropic Messages API.
#[derive(Debug, Deserialize)]
pub struct MessagesResponse {
    pub id: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    pub usage: Option<AnthropicUsage>,
}

/// Token usage in Anthropic format.
#[derive(Debug, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Error response from Anthropic.
#[derive(Debug, Deserialize)]
pub struct AnthropicError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub error: AnthropicErrorDetail,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

// ── Conversions ──────────────────────────────────────────────────────

/// Convert PulseHive Messages to Anthropic format.
///
/// Extracts the system prompt (first System message) to a separate string,
/// and converts remaining messages to Anthropic's content-block format.
pub fn convert_messages(messages: &[Message]) -> (Option<String>, Vec<AnthropicMessage>) {
    let mut system: Option<String> = None;
    let mut anthropic_msgs: Vec<AnthropicMessage> = Vec::new();

    for msg in messages {
        match msg {
            Message::System { content } => {
                // Anthropic: system prompt is top-level, not in messages
                if system.is_none() {
                    system = Some(content.clone());
                } else {
                    // Append additional system messages
                    if let Some(ref mut s) = system {
                        s.push('\n');
                        s.push_str(content);
                    }
                }
            }
            Message::User { content } => {
                anthropic_msgs.push(AnthropicMessage {
                    role: "user".into(),
                    content: AnthropicContent::Text(content.clone()),
                });
            }
            Message::Assistant {
                content,
                tool_calls,
            } => {
                if tool_calls.is_empty() {
                    anthropic_msgs.push(AnthropicMessage {
                        role: "assistant".into(),
                        content: AnthropicContent::Text(content.clone().unwrap_or_default()),
                    });
                } else {
                    // Assistant with tool calls → content blocks
                    let mut blocks: Vec<ContentBlock> = Vec::new();
                    if let Some(text) = content {
                        if !text.is_empty() {
                            blocks.push(ContentBlock::Text { text: text.clone() });
                        }
                    }
                    for tc in tool_calls {
                        blocks.push(ContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            input: tc.arguments.clone(),
                        });
                    }
                    anthropic_msgs.push(AnthropicMessage {
                        role: "assistant".into(),
                        content: AnthropicContent::Blocks(blocks),
                    });
                }
            }
            Message::ToolResult {
                tool_call_id,
                content,
            } => {
                // Anthropic: tool results are user messages with tool_result blocks
                anthropic_msgs.push(AnthropicMessage {
                    role: "user".into(),
                    content: AnthropicContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: tool_call_id.clone(),
                        content: content.clone(),
                    }]),
                });
            }
        }
    }

    (system, anthropic_msgs)
}

/// Convert Anthropic response to PulseHive LlmResponse.
pub fn convert_response(response: MessagesResponse) -> LlmResponse {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in response.content {
        match block {
            ContentBlock::Text { text } => {
                text_parts.push(text);
            }
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments: input,
                });
            }
            ContentBlock::ToolResult { .. } => {
                // Shouldn't appear in responses, ignore
            }
        }
    }

    let content = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join(""))
    };

    let usage = response
        .usage
        .map(|u| TokenUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
        })
        .unwrap_or_default();

    LlmResponse {
        content,
        tool_calls,
        usage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_messages_extracts_system() {
        let messages = vec![Message::system("You are helpful"), Message::user("Hello")];
        let (system, msgs) = convert_messages(&messages);
        assert_eq!(system, Some("You are helpful".into()));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn test_convert_messages_tool_result() {
        let messages = vec![Message::tool_result("call_1", "result text")];
        let (_, msgs) = convert_messages(&messages);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
        match &msgs[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert!(
                    matches!(&blocks[0], ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_1")
                );
            }
            _ => panic!("Expected Blocks content"),
        }
    }

    #[test]
    fn test_convert_response_text_only() {
        let response = MessagesResponse {
            id: "msg_1".into(),
            content: vec![ContentBlock::Text {
                text: "Hello!".into(),
            }],
            stop_reason: Some("end_turn".into()),
            usage: Some(AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
            }),
        };
        let result = convert_response(response);
        assert_eq!(result.content, Some("Hello!".into()));
        assert!(result.tool_calls.is_empty());
        assert_eq!(result.usage.output_tokens, 5);
    }

    #[test]
    fn test_convert_response_with_tool_use() {
        let response = MessagesResponse {
            id: "msg_2".into(),
            content: vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "search".into(),
                input: serde_json::json!({"query": "rust"}),
            }],
            stop_reason: Some("tool_use".into()),
            usage: None,
        };
        let result = convert_response(response);
        assert!(result.content.is_none());
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "search");
    }

    #[test]
    fn test_anthropic_tool_from_definition() {
        let td = ToolDefinition {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let at = AnthropicTool::from(&td);
        assert_eq!(at.name, "read_file");
        assert_eq!(at.description, "Read a file");
    }
}
