//! Internal types matching the OpenAI chat completions API schema.
//!
//! These types are not part of the public API — they handle serialization
//! to/from OpenAI's specific JSON format. The public API uses pulsehive-core types.

// Types are defined here but used in tickets #13-14 (chat/streaming implementation)
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::llm::{
    LlmChunk, LlmError, LlmErrorKind, LlmResponse, ReasoningEffort, TokenUsage, ToolCall,
    ToolChoice,
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenAIToolChoice>,
}

/// The OpenAI wire shape of a [`ToolChoice`] constraint: `"auto"`, `"none"`,
/// `"required"`, or `{"type":"function","function":{"name":…}}`.
///
/// Serialized by hand: serde's derived enum taggings do not produce the nested
/// function-object shape OpenAI expects.
#[derive(Debug)]
pub(crate) enum OpenAIToolChoice {
    Auto,
    None,
    Required,
    Function { name: String },
}

/// The `{"name": …}` object inside a function tool choice.
#[derive(Serialize)]
struct OpenAIToolFunctionName<'a> {
    name: &'a str,
}

impl Serialize for OpenAIToolChoice {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::None => serializer.serialize_str("none"),
            Self::Required => serializer.serialize_str("required"),
            Self::Function { name } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "function")?;
                map.serialize_entry("function", &OpenAIToolFunctionName { name })?;
                map.end()
            }
        }
    }
}

impl From<&ToolChoice> for OpenAIToolChoice {
    fn from(choice: &ToolChoice) -> Self {
        match choice {
            ToolChoice::Auto => Self::Auto,
            ToolChoice::None => Self::None,
            ToolChoice::Required => Self::Required,
            ToolChoice::Function { name } => Self::Function { name: name.clone() },
            // `ToolChoice` is non-exhaustive: a future variant has no OpenAI
            // spelling yet. Never panic (MASTER-SPEC §9.4) — fall back to the
            // model-decides posture, which is what an unset tool_choice means.
            _ => {
                tracing::warn!("no OpenAI wire form for this tool choice; sending \"auto\"");
                Self::Auto
            }
        }
    }
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
    pub finish_reason: Option<String>,
}

/// The message object inside a choice
#[derive(Debug, Deserialize)]
pub(crate) struct ChatMessage {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    /// Reasoning trace, under OpenAI's `reasoning` key or the
    /// `reasoning_content` spelling several compatible providers use.
    #[serde(
        default,
        alias = "reasoning_content",
        deserialize_with = "deserialize_reasoning"
    )]
    pub reasoning: Option<String>,
}

/// Deserializes a reasoning field permissively: a string populates it, any
/// other shape — the structured reasoning objects/arrays some
/// OpenAI-compatible endpoints return — is ignored as `None` so the
/// response still parses (before this field existed, serde ignored the
/// key entirely; failing the whole response is a compat regression).
fn deserialize_reasoning<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        Some(Value::String(text)) => Ok(Some(text)),
        _ => Ok(None),
    }
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
    /// Converts the OpenAI response into PulseHive's `LlmResponse`.
    ///
    /// Fails with `LlmErrorKind::MalformedToolCall` when a tool call's
    /// `arguments` string does not parse as a JSON object: a truncated call
    /// surfaces as a typed error carrying the raw arguments and the choice's
    /// `finish_reason`, never as a dispatchable call with empty `{}` arguments
    /// (E6). A model that legitimately sends `"{}"` still yields `{}` here —
    /// the rule is about parse failure, not empty objects. `attempts` is the
    /// number of requests actually sent to reach this response, carried onto
    /// the error so every post-response failure reports it (spec 4.1).
    pub(crate) fn into_llm_response(self, attempts: u32) -> Result<LlmResponse> {
        let choice = self.choices.into_iter().next();
        let (content, tool_calls, reasoning, finish_reason) = match choice {
            Some(c) => {
                let ChatChoice {
                    message,
                    finish_reason,
                } = c;
                let mut tool_calls = Vec::new();
                for tc in message.tool_calls.unwrap_or_default() {
                    let arguments =
                        parse_tool_arguments(&tc.function.arguments, &finish_reason, attempts)?;
                    tool_calls.push(ToolCall {
                        id: tc.id,
                        name: tc.function.name,
                        arguments,
                    });
                }
                (
                    message.content,
                    tool_calls,
                    message.reasoning,
                    finish_reason,
                )
            }
            None => (None, vec![], None, None),
        };

        let usage = self.usage.map_or(TokenUsage::default(), |u| TokenUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
        });

        let mut response = LlmResponse::new(content, tool_calls, usage);
        if let Some(reasoning) = reasoning {
            response = response.with_reasoning(reasoning);
        }
        if let Some(finish_reason) = finish_reason {
            response = response.with_finish_reason(finish_reason);
        }
        Ok(response)
    }
}

/// Parses one tool call's `arguments` string, requiring a JSON object.
///
/// An empty or whitespace-only string is a zero-argument call —
/// OpenAI-compatible backends (Ollama, LM Studio, vLLM) emit `""` for those —
/// and parses as `{}`, unless the completion was truncated
/// (`finish_reason: "length"`): an empty string then means the arguments
/// were cut off before any JSON was emitted, which is a typed
/// [`LlmErrorKind::MalformedToolCall`], not a dispatchable call. Anything
/// else non-object — a parse error, a non-object value — is the same typed
/// failure carrying the raw arguments as the body and the choice's finish
/// reason when present.
fn parse_tool_arguments(raw: &str, finish_reason: &Option<String>, attempts: u32) -> Result<Value> {
    let invalid = |message: String| {
        let mut err = LlmError::new(LlmErrorKind::MalformedToolCall, message)
            .with_status(200)
            .with_attempts(attempts)
            .with_body(raw);
        if let Some(reason) = finish_reason {
            err = err.with_finish_reason(reason.clone());
        }
        PulseHiveError::llm_transport(err)
    };

    if raw.trim().is_empty() {
        if finish_reason.as_deref() == Some("length") {
            return Err(invalid(
                "arguments are empty and the completion was truncated".into(),
            ));
        }
        return Ok(serde_json::json!({}));
    }

    match serde_json::from_str::<Value>(raw) {
        Ok(value) if value.is_object() => Ok(value),
        Ok(_) => Err(invalid("arguments are not a JSON object".into())),
        Err(e) => Err(invalid(e.to_string())),
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
        let llm_response = response.into_llm_response(1).unwrap();
        assert_eq!(llm_response.content, Some("Hello!".into()));
        assert!(llm_response.tool_calls.is_empty());
        assert_eq!(llm_response.usage.input_tokens, 10);
        assert_eq!(llm_response.usage.output_tokens, 5);
        assert_eq!(llm_response.finish_reason.as_deref(), Some("stop"));
        assert_eq!(llm_response.reasoning, None);
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
        let llm_response = response.into_llm_response(1).unwrap();
        assert!(llm_response.content.is_none());
        assert_eq!(llm_response.tool_calls.len(), 1);
        assert_eq!(llm_response.tool_calls[0].name, "read_file");
        // Arguments parsed from string into Value
        assert_eq!(llm_response.tool_calls[0].arguments["path"], "config.toml");
    }

    #[test]
    fn test_empty_tool_arguments_parse_as_empty_object() {
        // OpenAI-compatible backends (Ollama, LM Studio, vLLM) emit "" for
        // zero-argument tool calls; whitespace-only is the same shape.
        for raw in ["", "   "] {
            let value = parse_tool_arguments(raw, &Some("tool_calls".into()), 1)
                .expect("empty arguments are a zero-argument call");
            assert_eq!(value, serde_json::json!({}), "raw: {raw:?}");
        }

        // An empty finish_reason (or a non-truncation one) keeps the
        // compatibility behavior too.
        for reason in [None, Some("stop".to_string())] {
            let value = parse_tool_arguments("", &reason, 1)
                .expect("empty arguments on a completed call are zero-argument");
            assert_eq!(value, serde_json::json!({}), "reason: {reason:?}");
        }
    }

    #[test]
    fn test_empty_tool_arguments_with_truncation_finish_reason_are_rejected() {
        // A completion cut off by finish_reason "length" can emit an empty
        // arguments string: the arguments were truncated before any JSON was
        // emitted, which is a malformed call, not a dispatchable one.
        let err = match parse_tool_arguments("", &Some("length".into()), 2) {
            Err(PulseHiveError::LlmTransport(err)) => err,
            other => panic!("expected MalformedToolCall, got: {other:?}"),
        };
        assert_eq!(err.kind, LlmErrorKind::MalformedToolCall);
        assert_eq!(err.attempts, 2);
        assert_eq!(err.finish_reason.as_deref(), Some("length"));
    }

    #[test]
    fn test_truncated_tool_arguments_are_rejected() {
        let json = r#"{
            "id": "chatcmpl-abc",
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\": "
                        }
                    }]
                },
                "finish_reason": "length"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 5}
        }"#;

        let response: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        let err = match response.into_llm_response(1) {
            Err(PulseHiveError::LlmTransport(err)) => err,
            other => panic!("expected LlmTransport, got: {other:?}"),
        };
        assert_eq!(err.kind, LlmErrorKind::MalformedToolCall);
        assert_eq!(err.status, Some(200));
        assert_eq!(err.body.as_deref(), Some("{\"path\": "));
        assert_eq!(err.finish_reason.as_deref(), Some("length"));
    }

    #[test]
    fn test_non_object_tool_arguments_are_rejected() {
        // Parses as JSON, but is not an object.
        let json = r#"{
            "id": "chatcmpl-abc",
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "f", "arguments": "[1,2]"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 5}
        }"#;

        let response: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        let err = match response.into_llm_response(1) {
            Err(PulseHiveError::LlmTransport(err)) => err,
            other => panic!("expected LlmTransport, got: {other:?}"),
        };
        assert_eq!(err.kind, LlmErrorKind::MalformedToolCall);
        assert_eq!(err.message, "arguments are not a JSON object");
    }

    #[test]
    fn test_reasoning_and_alias_come_off_the_wire() {
        for key in ["reasoning", "reasoning_content"] {
            let json = format!(
                r#"{{
                    "id": "chatcmpl-abc",
                    "choices": [{{
                        "message": {{"content": "", "{key}": "thinking", "tool_calls": null}},
                        "finish_reason": "length"
                    }}],
                    "usage": {{"prompt_tokens": 9, "completion_tokens": 9}}
                }}"#
            );
            let response: ChatCompletionResponse = serde_json::from_str(&json).unwrap();
            let llm = response.into_llm_response(1).unwrap();
            assert_eq!(llm.reasoning.as_deref(), Some("thinking"), "key: {key}");
            assert_eq!(llm.finish_reason.as_deref(), Some("length"), "key: {key}");
        }
    }

    #[test]
    fn test_structured_reasoning_is_ignored_not_fatal() {
        // Some OpenAI-compatible endpoints return a structured object or
        // array for reasoning/reasoning_content; before the field existed
        // serde ignored them, so failing the whole response would be a
        // compat regression. Any non-string shape deserializes as None.
        for key in ["reasoning", "reasoning_content"] {
            for shape in [
                r#"{"summary":["thought"],"effort":1}"#,
                r#"["step 1","step 2"]"#,
                r#"42"#,
                r#"null"#,
            ] {
                let json = format!(
                    r#"{{
                        "id": "chatcmpl-abc",
                        "choices": [{{
                            "message": {{"content": "ok", "{key}": {shape}, "tool_calls": null}},
                            "finish_reason": "stop"
                        }}],
                        "usage": {{"prompt_tokens": 1, "completion_tokens": 1}}
                    }}"#
                );
                let response: ChatCompletionResponse =
                    serde_json::from_str(&json).unwrap_or_else(|e| {
                        panic!("response must parse, key {key} shape {shape}: {e}")
                    });
                let llm = response.into_llm_response(1).unwrap();
                assert_eq!(llm.reasoning, None, "key: {key}, shape: {shape}");
                assert_eq!(llm.content.as_deref(), Some("ok"));
            }
        }
    }

    #[test]
    fn test_tool_choice_wire_shape() {
        let cases: Vec<(ToolChoice, String)> = vec![
            (ToolChoice::Auto, "\"auto\"".into()),
            (ToolChoice::None, "\"none\"".into()),
            (ToolChoice::Required, "\"required\"".into()),
            (
                ToolChoice::Function { name: "f".into() },
                "{\"type\":\"function\",\"function\":{\"name\":\"f\"}}".into(),
            ),
        ];
        for (choice, expected) in cases {
            // Serialize straight to a string: routing through `Value` would
            // re-order keys via its BTreeMap, which is not the wire path.
            let wire = serde_json::to_string(&OpenAIToolChoice::from(&choice)).unwrap();
            assert_eq!(wire, expected);
        }
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
            reasoning_effort: None,
            tool_choice: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "gpt-4");
        assert!(json.get("tools").is_none()); // skipped when empty
        assert!(json.get("stream").is_none()); // skipped when false
        assert!(json.get("reasoning_effort").is_none()); // skipped when unset
        assert!(json.get("tool_choice").is_none()); // skipped when unset
    }
}
