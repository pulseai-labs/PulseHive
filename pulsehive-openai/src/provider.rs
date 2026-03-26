//! OpenAI-compatible LLM provider implementation.

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::StreamExt;
use futures_core::Stream;
use serde_json::Value;

use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::llm::{LlmChunk, LlmConfig, LlmProvider, LlmResponse, Message, ToolDefinition};

use crate::config::OpenAIConfig;
use crate::types::{ChatCompletionRequest, ChatCompletionResponse, OpenAITool, StreamChunk};

/// LLM provider for any OpenAI-compatible API.
///
/// Supports OpenAI, GLM (BigModel), vLLM, LM Studio, Ollama, Together, Groq,
/// and any other service exposing the OpenAI chat completions endpoint.
pub struct OpenAICompatibleProvider {
    pub(crate) config: OpenAIConfig,
    pub(crate) client: reqwest::Client,
}

impl OpenAICompatibleProvider {
    /// Creates a new provider with the given configuration.
    ///
    /// Builds an HTTP client with the configured timeout and Bearer auth header.
    pub fn new(config: OpenAIConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                if let Ok(val) =
                    reqwest::header::HeaderValue::from_str(&format!("Bearer {}", config.api_key))
                {
                    headers.insert(reqwest::header::AUTHORIZATION, val);
                }
                headers.insert(
                    reqwest::header::CONTENT_TYPE,
                    reqwest::header::HeaderValue::from_static("application/json"),
                );
                headers
            })
            .build()
            .expect("Failed to build HTTP client");

        Self { config, client }
    }

    /// Build a ChatCompletionRequest from PulseHive types.
    fn build_request(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &LlmConfig,
        stream: bool,
    ) -> Result<ChatCompletionRequest> {
        let mut message_values: Vec<Value> = messages
            .iter()
            .map(|m| serde_json::to_value(m).map_err(|e| PulseHiveError::llm(e.to_string())))
            .collect::<Result<Vec<_>>>()?;

        // Transform tool_calls in assistant messages to OpenAI wire format.
        // The internal ToolCall format (id, name, arguments:Value) must become
        // the wire format (id, type:"function", function:{name, arguments:String}).
        for msg in &mut message_values {
            if let Some(obj) = msg.as_object_mut() {
                if obj.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                    if let Some(Value::Array(tool_calls)) = obj.get_mut("tool_calls") {
                        let fixed: Vec<Value> = tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc["id"],
                                    "type": "function",
                                    "function": {
                                        "name": tc["name"],
                                        "arguments": tc["arguments"].to_string()
                                    }
                                })
                            })
                            .collect();
                        *tool_calls = fixed;
                    }
                }
            }
        }

        let openai_tools: Vec<OpenAITool> = tools.iter().map(OpenAITool::from_tool_def).collect();

        let model = if config.model.is_empty() {
            self.config.model.clone()
        } else {
            config.model.clone()
        };

        Ok(ChatCompletionRequest {
            model,
            messages: message_values,
            tools: openai_tools,
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            stream,
        })
    }

    /// Send a request with automatic retry for transient errors.
    ///
    /// Retries on: 429 (rate limit), 500, 502, 503, 529 (server overloaded).
    /// Fails immediately on: 400, 401, 403, 404 (client errors).
    /// Uses exponential backoff: 1s → 2s → 4s, respects Retry-After header on 429.
    async fn send_request(&self, request: &ChatCompletionRequest) -> Result<reqwest::Response> {
        let url = self.config.chat_completions_url();
        let max_attempts = self.config.max_retries + 1;
        let mut last_err = PulseHiveError::llm("No attempts made");

        for attempt in 1..=max_attempts {
            tracing::debug!(
                url = %url,
                model = %request.model,
                attempt = attempt,
                max = max_attempts,
                "Sending chat request"
            );

            let response = self.client.post(&url).json(request).send().await;

            match response {
                Ok(resp) if resp.status().is_success() => {
                    return Ok(resp);
                }
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = parse_retry_after(&resp);
                    let body = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "<failed to read body>".into());

                    let err_msg = format!("OpenAI API error (HTTP {status}): {body}");

                    if is_retryable_status(status) && attempt < max_attempts {
                        let delay = retry_after.unwrap_or_else(|| retry_delay(attempt));
                        tracing::warn!(
                            attempt = attempt,
                            status = %status,
                            delay_ms = delay.as_millis(),
                            "Retrying after transient error"
                        );
                        tokio::time::sleep(delay).await;
                        last_err = PulseHiveError::llm(err_msg);
                        continue;
                    }

                    return Err(PulseHiveError::llm(err_msg));
                }
                Err(e) => {
                    let err_msg = format!("HTTP request failed: {e}");

                    if attempt < max_attempts {
                        let delay = retry_delay(attempt);
                        tracing::warn!(
                            attempt = attempt,
                            delay_ms = delay.as_millis(),
                            "Retrying after connection error: {e}"
                        );
                        tokio::time::sleep(delay).await;
                        last_err = PulseHiveError::llm(err_msg);
                        continue;
                    }

                    return Err(PulseHiveError::llm(err_msg));
                }
            }
        }

        Err(last_err)
    }
}

#[async_trait]
impl LlmProvider for OpenAICompatibleProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<LlmResponse> {
        let request = self.build_request(&messages, &tools, config, false)?;
        let response = self.send_request(&request).await?;

        let body = response
            .text()
            .await
            .map_err(|e| PulseHiveError::llm(format!("Failed to read response body: {e}")))?;

        let completion: ChatCompletionResponse = serde_json::from_str(&body).map_err(|e| {
            let excerpt = if body.len() > 200 {
                format!("{}...", &body[..200])
            } else {
                body.clone()
            };
            PulseHiveError::llm(format!("Failed to parse response: {e}\nBody: {excerpt}"))
        })?;

        Ok(completion.into_llm_response())
    }

    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk>> + Send>>> {
        let request = self.build_request(&messages, &tools, config, true)?;
        let response = self.send_request(&request).await?;

        let stream = response
            .bytes_stream()
            .scan(SseParseState::new(), |state, bytes_result| {
                let chunks = match bytes_result {
                    Ok(bytes) => {
                        state.buffer.extend_from_slice(&bytes);
                        state.emit_chunks()
                    }
                    Err(e) => {
                        vec![Err(PulseHiveError::llm(format!("Stream error: {e}")))]
                    }
                };
                // Return Some to keep scanning, None would stop
                futures::future::ready(Some(chunks))
            })
            .flat_map(futures::stream::iter)
            .boxed();

        Ok(stream)
    }
}

// ── SSE Parse State Machine ──────────────────────────────────────────

/// State machine for parsing SSE events from a byte stream.
///
/// Buffers incoming bytes until a complete event (delimited by `\n\n`) is found,
/// then parses the `data: {json}` payload into `LlmChunk` items.
struct SseParseState {
    /// Buffer for accumulating bytes until `\n\n` delimiter.
    buffer: Vec<u8>,
    /// Maps tool call index → tool call ID for delta fixup.
    active_tool_calls: HashMap<usize, String>,
    /// Set to true after receiving `data: [DONE]`.
    finished: bool,
}

impl SseParseState {
    fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(4096),
            active_tool_calls: HashMap::new(),
            finished: false,
        }
    }

    /// Parse all complete SSE events from the buffer and return LlmChunks.
    fn emit_chunks(&mut self) -> Vec<Result<LlmChunk>> {
        if self.finished {
            return vec![];
        }

        let mut results = Vec::new();

        loop {
            // Find the next complete event (delimited by \n\n)
            let pos = self.buffer.windows(2).position(|w| w == b"\n\n");
            let Some(pos) = pos else {
                break; // No complete event yet, wait for more bytes
            };

            // Extract the event bytes and advance the buffer
            let event_bytes: Vec<u8> = self.buffer.drain(..pos + 2).collect();
            let event_str = String::from_utf8_lossy(&event_bytes);

            // Parse each line in the event (SSE can have multiple lines per event)
            for line in event_str.lines() {
                let Some(data) = line.strip_prefix("data: ") else {
                    continue; // Skip non-data lines (e.g., comments, empty lines)
                };

                if data == "[DONE]" {
                    self.finished = true;
                    results.push(Ok(LlmChunk::Done));
                    return results;
                }

                match serde_json::from_str::<StreamChunk>(data) {
                    Ok(chunk) => {
                        let Some(choice) = chunk.choices.into_iter().next() else {
                            continue;
                        };

                        let mut chunks = choice.delta.into_chunks();

                        // Fix tool call delta IDs using tracked state
                        for chunk in &mut chunks {
                            match chunk {
                                LlmChunk::ToolCallStart { id, .. } => {
                                    // Track this tool call for future deltas
                                    let index = self.active_tool_calls.len();
                                    self.active_tool_calls.insert(index, id.clone());
                                }
                                LlmChunk::ToolCallDelta { id, .. } if id.is_empty() => {
                                    // Fill in the ID from the most recent active tool call
                                    if let Some((_, active_id)) =
                                        self.active_tool_calls.iter().max_by_key(|(idx, _)| *idx)
                                    {
                                        *id = active_id.clone();
                                    }
                                }
                                _ => {}
                            }
                        }

                        results.extend(chunks.into_iter().map(Ok));
                    }
                    Err(e) => {
                        tracing::warn!(data = %data, error = %e, "Failed to parse SSE event");
                    }
                }
            }
        }

        results
    }
}

// ── Retry helpers ────────────────────────────────────────────────────

/// Returns true if the HTTP status indicates a transient error worth retrying.
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 529)
}

/// Computes exponential backoff delay: 1s * 2^(attempt-1), capped at 8s.
fn retry_delay(attempt: u32) -> Duration {
    let secs = (1u64 << (attempt - 1).min(3)).min(8);
    Duration::from_secs(secs)
}

/// Parses the Retry-After header from a response (if present).
/// Returns None if header is missing or unparseable.
fn parse_retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpenAIConfig;
    use pulsehive_core::llm::ToolCall;

    #[test]
    fn test_provider_construction() {
        let config = OpenAIConfig::new("sk-test", "gpt-4");
        let _provider = OpenAICompatibleProvider::new(config);
    }

    #[test]
    fn test_provider_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<OpenAICompatibleProvider>();
    }

    #[test]
    fn test_provider_is_object_safe() {
        fn _assert_object_safe(_: &dyn LlmProvider) {}
        let config = OpenAIConfig::new("sk-test", "gpt-4");
        let provider = OpenAICompatibleProvider::new(config);
        _assert_object_safe(&provider);
    }

    // ── build_request tests ──────────────────────────────────────────

    #[test]
    fn test_build_request_basic() {
        let config = OpenAIConfig::new("sk-test", "gpt-4");
        let provider = OpenAICompatibleProvider::new(config);
        let messages = vec![Message::system("Be helpful"), Message::user("Hello")];
        let llm_config = LlmConfig::new("openai", "gpt-4o");

        let req = provider
            .build_request(&messages, &[], &llm_config, false)
            .unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.messages.len(), 2);
        assert!(!req.stream);
    }

    #[test]
    fn test_build_request_stream_flag() {
        let config = OpenAIConfig::new("sk-test", "gpt-4");
        let provider = OpenAICompatibleProvider::new(config);
        let req = provider
            .build_request(
                &[Message::user("hi")],
                &[],
                &LlmConfig::new("openai", "gpt-4"),
                true,
            )
            .unwrap();
        assert!(req.stream);
    }

    #[test]
    fn test_build_request_with_tool_calls_wire_format() {
        let config = OpenAIConfig::new("sk-test", "gpt-4");
        let provider = OpenAICompatibleProvider::new(config);
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("Search for something"),
            Message::assistant_with_tool_calls(vec![ToolCall {
                id: "call_1".into(),
                name: "search".into(),
                arguments: serde_json::json!({"query": "test"}),
            }]),
            Message::tool_result("call_1", "Found it"),
        ];
        let llm_config = LlmConfig::new("openai", "gpt-4");
        let req = provider
            .build_request(&messages, &[], &llm_config, false)
            .unwrap();

        // The assistant message (index 2) should have wire-format tool_calls
        let assistant_msg = &req.messages[2];
        let tool_calls = assistant_msg["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls.len(), 1);

        let tc = &tool_calls[0];
        // Must have "type": "function"
        assert_eq!(tc["type"], "function", "Missing type:function wrapper");
        // Must have nested "function" object
        assert!(tc["function"].is_object(), "Missing function nesting");
        assert_eq!(tc["function"]["name"], "search");
        // arguments must be a JSON STRING, not an object
        assert!(
            tc["function"]["arguments"].is_string(),
            "arguments should be a JSON string, got: {}",
            tc["function"]["arguments"]
        );
        let args_str = tc["function"]["arguments"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(parsed["query"], "test");
    }

    // ── SSE parsing tests ────────────────────────────────────────────

    #[test]
    fn test_sse_parse_text_chunks() {
        let mut state = SseParseState::new();

        // Simulate receiving SSE data
        state.buffer.extend_from_slice(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
        );
        let chunks = state.emit_chunks();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], Ok(LlmChunk::Text(t)) if t == "Hello"));

        state.buffer.extend_from_slice(
            b"data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n",
        );
        let chunks = state.emit_chunks();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], Ok(LlmChunk::Text(t)) if t == " world"));
    }

    #[test]
    fn test_sse_parse_done_sentinel() {
        let mut state = SseParseState::new();

        state.buffer.extend_from_slice(b"data: [DONE]\n\n");
        let chunks = state.emit_chunks();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], Ok(LlmChunk::Done)));

        // After DONE, no more chunks
        state
            .buffer
            .extend_from_slice(b"data: {\"choices\":[{\"delta\":{\"content\":\"ignored\"}}]}\n\n");
        let chunks = state.emit_chunks();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_sse_parse_partial_event() {
        let mut state = SseParseState::new();

        // First byte chunk: partial event (no \n\n yet)
        state
            .buffer
            .extend_from_slice(b"data: {\"choices\":[{\"delta\":{\"content\":");
        let chunks = state.emit_chunks();
        assert!(chunks.is_empty()); // Not enough data

        // Second byte chunk: completes the event
        state.buffer.extend_from_slice(b"\"Hello\"}}]}\n\n");
        let chunks = state.emit_chunks();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], Ok(LlmChunk::Text(t)) if t == "Hello"));
    }

    #[test]
    fn test_sse_parse_multiple_events_in_one_chunk() {
        let mut state = SseParseState::new();

        // Two events arrive in one byte chunk
        state.buffer.extend_from_slice(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"A\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"B\"}}]}\n\n",
        );
        let chunks = state.emit_chunks();
        assert_eq!(chunks.len(), 2);
        assert!(matches!(&chunks[0], Ok(LlmChunk::Text(t)) if t == "A"));
        assert!(matches!(&chunks[1], Ok(LlmChunk::Text(t)) if t == "B"));
    }

    #[test]
    fn test_sse_parse_tool_call_start() {
        let mut state = SseParseState::new();

        state.buffer.extend_from_slice(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_123\",\"function\":{\"name\":\"search\"}}]}}]}\n\n",
        );
        let chunks = state.emit_chunks();
        assert_eq!(chunks.len(), 1);
        assert!(
            matches!(&chunks[0], Ok(LlmChunk::ToolCallStart { id, name }) if id == "call_123" && name == "search")
        );
    }

    #[test]
    fn test_sse_parse_tool_call_delta_with_id_fixup() {
        let mut state = SseParseState::new();

        // First: tool call start (registers the ID)
        state.buffer.extend_from_slice(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_123\",\"function\":{\"name\":\"search\"}}]}}]}\n\n",
        );
        state.emit_chunks();

        // Then: arguments delta (no id field — should be fixed up from state)
        state.buffer.extend_from_slice(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"q\\\":\"}}]}}]}\n\n",
        );
        let chunks = state.emit_chunks();
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            Ok(LlmChunk::ToolCallDelta {
                id,
                arguments_delta,
            }) => {
                assert_eq!(id, "call_123"); // Fixed up from state
                assert_eq!(arguments_delta, "{\"q\":");
            }
            other => panic!("Expected ToolCallDelta, got: {other:?}"),
        }
    }

    #[test]
    fn test_sse_parse_empty_content_delta_skipped() {
        let mut state = SseParseState::new();

        // Empty content delta (some providers send these)
        state
            .buffer
            .extend_from_slice(b"data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n");
        let chunks = state.emit_chunks();
        assert!(chunks.is_empty()); // Skipped by into_chunks()
    }

    #[test]
    fn test_sse_parse_malformed_json_skipped() {
        let mut state = SseParseState::new();

        state.buffer.extend_from_slice(b"data: {invalid json}\n\n");
        let chunks = state.emit_chunks();
        assert!(chunks.is_empty()); // Malformed event skipped
    }

    #[test]
    fn test_sse_full_conversation_flow() {
        let mut state = SseParseState::new();

        // Simulate a complete streaming conversation
        let events = [
            b"data: {\"choices\":[{\"delta\":{\"content\":\"The \"}}]}\n\n".as_slice(),
            b"data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\n",
            b"data: {\"choices\":[{\"delta\":{\"content\":\" is 42.\"}}]}\n\n",
            b"data: [DONE]\n\n",
        ];

        let mut all_text = String::new();
        let mut got_done = false;

        for event in events {
            state.buffer.extend_from_slice(event);
            for chunk in state.emit_chunks() {
                match chunk.unwrap() {
                    LlmChunk::Text(t) => all_text.push_str(&t),
                    LlmChunk::Done => got_done = true,
                    _ => panic!("Unexpected chunk type"),
                }
            }
        }

        assert_eq!(all_text, "The answer is 42.");
        assert!(got_done);
    }

    // ── Retry helper tests ─────────────────────────────────────────

    #[test]
    fn test_retryable_statuses() {
        use reqwest::StatusCode;
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS)); // 429
        assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR)); // 500
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY)); // 502
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE)); // 503

        // Non-retryable
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST)); // 400
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED)); // 401
        assert!(!is_retryable_status(StatusCode::FORBIDDEN)); // 403
        assert!(!is_retryable_status(StatusCode::NOT_FOUND)); // 404
        assert!(!is_retryable_status(StatusCode::OK)); // 200
    }

    #[test]
    fn test_retry_delay_exponential_backoff() {
        assert_eq!(retry_delay(1), Duration::from_secs(1)); // 2^0
        assert_eq!(retry_delay(2), Duration::from_secs(2)); // 2^1
        assert_eq!(retry_delay(3), Duration::from_secs(4)); // 2^2
        assert_eq!(retry_delay(4), Duration::from_secs(8)); // 2^3, capped
        assert_eq!(retry_delay(5), Duration::from_secs(8)); // still capped
    }

    // ── HTTP-level tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_chat_with_invalid_url_returns_error() {
        let config =
            OpenAIConfig::new("sk-test", "gpt-4").with_base_url("http://localhost:1/invalid");
        let provider = OpenAICompatibleProvider::new(config);

        let result = provider
            .chat(
                vec![Message::user("hi")],
                vec![],
                &LlmConfig::new("openai", "gpt-4"),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_chat_stream_with_invalid_url_returns_error() {
        let config =
            OpenAIConfig::new("sk-test", "gpt-4").with_base_url("http://localhost:1/invalid");
        let provider = OpenAICompatibleProvider::new(config);

        let result = provider
            .chat_stream(
                vec![Message::user("hi")],
                vec![],
                &LlmConfig::new("openai", "gpt-4"),
            )
            .await;
        assert!(result.is_err());
    }
}
