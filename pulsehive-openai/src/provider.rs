//! OpenAI-compatible LLM provider implementation.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::StreamExt;
use futures_core::Stream;
use serde_json::Value;

use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::llm::{
    LlmChunk, LlmConfig, LlmError, LlmErrorKind, LlmProvider, LlmResponse, Message, ToolDefinition,
};

use crate::config::{OpenAIConfig, OpenAIConfigView};
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, OpenAITool, OpenAIToolChoice, StreamChunk,
};

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
            // New wire fields go out only when the caller set them, so an
            // unset config sends a body byte-identical to 2.0.2 (#47 R1, R6).
            reasoning_effort: config.reasoning_effort,
            // tool_choice without tools is a 400 on every OpenAI-compatible
            // endpoint, so it is omitted whenever the request carries no
            // tools, caller intent or not.
            tool_choice: if tools.is_empty() {
                None
            } else {
                config.tool_choice.as_ref().map(OpenAIToolChoice::from)
            },
        })
    }

    /// The provider's configuration (E7): a non-secret view of the defaults
    /// for endpoint, model, timeout and retry budget as constructed. There is
    /// no path from the view to the API key, and neither the view nor
    /// [`OpenAIConfig`] renders the key in its `Debug` output.
    pub fn config(&self) -> OpenAIConfigView {
        self.config.view()
    }

    /// Send a request under the provider's transport policy.
    ///
    /// Every failure is a typed [`LlmError`] inside
    /// [`PulseHiveError::LlmTransport`] — never a bare string, except a
    /// builder error (malformed `base_url`), which never reaches the
    /// network, fails immediately without retrying, and surfaces as the
    /// request-build failure it is via [`PulseHiveError::llm`]. The two
    /// failure branches are distinguishable by kind (#46's second complaint):
    ///
    /// * **Status failures** — the provider answered a non-success status.
    ///   `429`, `500`, `502`, `503` and `529` are retried up to the attempt
    ///   budget. `429` and `529` — the overload statuses — wait for an
    ///   integer-seconds `Retry-After` when present, capped at the same 8s
    ///   ceiling as the exponential backoff (1s → 2s → 4s → 8s) that `500` /
    ///   `502` / `503` and a header-less `429`/`529` always use. Every other
    ///   4xx is [`LlmErrorKind::ClientError`] and every other 5xx
    ///   [`LlmErrorKind::ServerError`], both failing immediately with the raw
    ///   body attached verbatim.
    /// * **Transport failures** — the request never completed. A deadline
    ///   expiry ([`LlmErrorKind::Timeout`]) is **never retried**: the request
    ///   timed out and re-sending it would bill the consumer for work already
    ///   spent (#46). A connection-level error ([`LlmErrorKind::Connect`]) is
    ///   retried with the same backoff. A cancelled
    ///   [`LlmConfig::cancel`] token ([`LlmErrorKind::Cancelled`]) aborts the
    ///   in-flight request, the body read and every backoff sleep, and is
    ///   never retried.
    ///
    /// Per-call `LlmConfig` overrides: `max_retries` replaces this provider's
    /// configured budget for this one call (`Some(0)` means exactly one
    /// attempt), and `timeout_secs` replaces the client-level deadline for
    /// this request only.
    ///
    /// `attempts` on every error counts the requests actually sent, the failed
    /// one included; it is `0` only for a cancellation observed before the
    /// first send. On success the count travels with the response, so errors
    /// built downstream (parse, malformed tool call) carry it too.
    async fn send_request(
        &self,
        request: &ChatCompletionRequest,
        call: &LlmConfig,
    ) -> Result<(reqwest::Response, u32)> {
        let url = self.config.chat_completions_url();
        // Saturating: `LlmConfig` is an unvalidated Deserialize, so a
        // `max_retries` of `u32::MAX` must still yield at least one attempt
        // instead of overflowing (debug) or wrapping to zero (release).
        let max_attempts = call
            .max_retries
            .unwrap_or(self.config.max_retries)
            .saturating_add(1);
        let cancel = call.cancel.as_ref();

        // A token cancelled before the call never sends anything.
        if let Some(token) = cancel {
            if token.is_cancelled() {
                return Err(cancelled_error(0));
            }
        }

        for attempt in 1..=max_attempts {
            tracing::debug!(
                url = %url,
                model = %request.model,
                attempt = attempt,
                max = max_attempts,
                "Sending chat request"
            );

            let mut builder = self.client.post(&url).json(request);
            if let Some(secs) = call.timeout_secs {
                // Per-request deadline: replaces the client-level timeout for
                // this call only (4.2).
                builder = builder.timeout(Duration::from_secs(secs));
            }

            // The send future is latched as started so a cancellation that
            // wins before it was ever polled reports the previous attempt
            // count: attempts counts requests actually sent.
            let started = Arc::new(AtomicBool::new(false));
            let send = MarkStarted {
                future: Box::pin(builder.send()),
                started: Arc::clone(&started),
            };
            let response = match cancel {
                Some(token) => tokio::select! {
                    // Dropping the send future is what aborts the connection.
                    biased;
                    _ = token.cancelled() => {
                        let sent = if started.load(Ordering::Relaxed) {
                            attempt
                        } else {
                            attempt - 1
                        };
                        return Err(cancelled_error(sent));
                    }
                    response = send => response,
                },
                None => send.await,
            };

            let response = match response {
                Ok(resp) => resp,
                // A timed-out request fails once and is never re-sent (E5).
                Err(e) if e.is_timeout() => {
                    tracing::warn!(
                        attempt = attempt,
                        "request timed out after {attempt} attempt(s); not retrying"
                    );
                    return Err(PulseHiveError::llm_transport(
                        LlmError::new(LlmErrorKind::Timeout, e.to_string()).with_attempts(attempt),
                    ));
                }
                // A builder error (malformed base_url) never reaches the
                // network: a request-build failure, surfaced immediately —
                // not retried through the budget and not classified as
                // Connect.
                Err(e) if e.is_builder() => {
                    return Err(PulseHiveError::llm(format!("failed to build request: {e}")));
                }
                Err(e) => {
                    if attempt < max_attempts {
                        let delay = retry_delay(attempt);
                        tracing::warn!(
                            attempt = attempt,
                            kind = "connect",
                            delay_ms = delay.as_millis() as u64,
                            error = %e,
                            "Retrying after connection error"
                        );
                        if let Some(token) = cancel {
                            tokio::select! {
                                biased;
                                _ = token.cancelled() => return Err(cancelled_error(attempt)),
                                _ = tokio::time::sleep(delay) => {}
                            }
                        } else {
                            tokio::time::sleep(delay).await;
                        }
                        continue;
                    }
                    return Err(PulseHiveError::llm_transport(
                        LlmError::new(LlmErrorKind::Connect, e.to_string()).with_attempts(attempt),
                    ));
                }
            };

            if response.status().is_success() {
                return Ok((response, attempt));
            }

            let status = response.status();
            // Retry-After is honored only on the overload statuses (429/529):
            // 500/502/503 use the same exponential backoff as any other
            // retryable failure, and the header is not reported as retry
            // guidance where it is not honored.
            let retry_after = parse_retry_after(&response).filter(|_| honors_retry_after(status));
            let text = response.text();
            let text = match cancel {
                Some(token) => tokio::select! {
                    biased;
                    _ = token.cancelled() => return Err(cancelled_error(attempt)),
                    text = text => text,
                },
                None => text.await,
            };
            // Only a successfully read body proceeds to status classification:
            // a body-read failure is a transport failure, not a status failure
            // with a placeholder body.
            let body = match text {
                Ok(body) => body,
                Err(e) if e.is_timeout() => {
                    tracing::warn!(
                        attempt = attempt,
                        "request timed out after {attempt} attempt(s); not retrying"
                    );
                    return Err(PulseHiveError::llm_transport(
                        LlmError::new(LlmErrorKind::Timeout, e.to_string()).with_attempts(attempt),
                    ));
                }
                Err(e) => {
                    if attempt < max_attempts {
                        let delay = retry_delay(attempt);
                        tracing::warn!(
                            attempt = attempt,
                            kind = "connect",
                            delay_ms = delay.as_millis() as u64,
                            error = %e,
                            "Retrying after body-read connection error"
                        );
                        if let Some(token) = cancel {
                            tokio::select! {
                                biased;
                                _ = token.cancelled() => return Err(cancelled_error(attempt)),
                                _ = tokio::time::sleep(delay) => {}
                            }
                        } else {
                            tokio::time::sleep(delay).await;
                        }
                        continue;
                    }
                    return Err(PulseHiveError::llm_transport(
                        LlmError::new(LlmErrorKind::Connect, e.to_string()).with_attempts(attempt),
                    ));
                }
            };

            let kind = if status.as_u16() == 429 {
                LlmErrorKind::RateLimited
            } else if is_retryable_status(status) {
                LlmErrorKind::ServerError
            } else if status.is_client_error() {
                LlmErrorKind::ClientError
            } else {
                LlmErrorKind::ServerError
            };

            let mut err = LlmError::new(kind, format!("HTTP {status}"))
                .with_status(status.as_u16())
                .with_attempts(attempt)
                .with_body(body);
            if let Some(delay) = retry_after {
                // The error carries the server's verbatim guidance; only the
                // honored sleep below is capped.
                err = err.with_retry_after(delay);
            }

            if is_retryable_status(status) && attempt < max_attempts {
                let delay = honored_retry_delay(retry_after, attempt);
                tracing::warn!(
                    attempt = attempt,
                    status = %status,
                    delay_ms = delay.as_millis() as u64,
                    "Retrying after transient error"
                );
                if let Some(token) = cancel {
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => return Err(cancelled_error(attempt)),
                        _ = tokio::time::sleep(delay) => {}
                    }
                } else {
                    tokio::time::sleep(delay).await;
                }
                continue;
            }

            return Err(PulseHiveError::llm_transport(err));
        }

        // Unreachable in practice: the final iteration always returns. Kept as
        // a typed error rather than a panic (MASTER-SPEC §9.4).
        Err(PulseHiveError::llm_transport(
            LlmError::new(LlmErrorKind::ServerError, "retry budget exhausted")
                .with_attempts(max_attempts),
        ))
    }
}

/// The typed error for a caller cancellation.
fn cancelled_error(attempts: u32) -> PulseHiveError {
    PulseHiveError::llm_transport(
        LlmError::new(LlmErrorKind::Cancelled, "call cancelled by the caller")
            .with_attempts(attempts),
    )
}

/// A future that latches whether it has been polled, so a cancellation
/// racing it can distinguish "never started" from "in flight": `attempts`
/// counts requests actually sent, and a cancellation observed before the
/// send future's first poll never sent anything.
struct MarkStarted<F> {
    future: Pin<Box<F>>,
    started: Arc<AtomicBool>,
}

impl<F: Future> Future for MarkStarted<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        this.started.store(true, Ordering::Relaxed);
        this.future.as_mut().poll(cx)
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
        let (response, attempts) = self.send_request(&request, config).await?;

        let status = response.status();
        let read = response.text();
        let read = match config.cancel.as_ref() {
            Some(token) => tokio::select! {
                biased;
                _ = token.cancelled() => return Err(cancelled_error(attempts)),
                body = read => body,
            },
            None => read.await,
        };
        let body = match read {
            Ok(body) => body,
            // A timeout while reading the body is a Timeout, not a Parse (4.1).
            Err(e) if e.is_timeout() => {
                return Err(PulseHiveError::llm_transport(
                    LlmError::new(LlmErrorKind::Timeout, e.to_string()).with_attempts(attempts),
                ))
            }
            Err(e) => {
                return Err(PulseHiveError::llm_transport(
                    LlmError::new(LlmErrorKind::Parse, e.to_string())
                        .with_status(status.as_u16())
                        .with_attempts(attempts),
                ))
            }
        };

        // The body is attached verbatim — the consumer decides what to redact.
        let completion: ChatCompletionResponse = serde_json::from_str(&body).map_err(|e| {
            PulseHiveError::llm_transport(
                LlmError::new(LlmErrorKind::Parse, e.to_string())
                    .with_status(status.as_u16())
                    .with_attempts(attempts)
                    .with_body(body.clone()),
            )
        })?;

        completion.into_llm_response(attempts)
    }

    /// Streams a chat completion as SSE chunks.
    ///
    /// Limitation (E11, #47 R4): the streaming path carries neither
    /// `reasoning` nor `finish_reason` — only [`Self::chat`] surfaces them.
    /// Transport failures before the stream starts surface as the same typed
    /// [`PulseHiveError::LlmTransport`] errors as `chat`, and so does every
    /// mid-stream body-read failure (classified like `chat()`'s body read).
    /// A cancelled [`LlmConfig::cancel`] token aborts the in-flight request,
    /// every backoff sleep and every read of the streamed body: a server that
    /// answers `200` and then stalls cannot hold the stream past the token.
    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk>> + Send>>> {
        let request = self.build_request(&messages, &tools, config, true)?;
        let (response, attempts) = self.send_request(&request, config).await?;
        let status = response.status();
        let cancel = config.cancel.clone();

        let reads = response
            .bytes_stream()
            .map(move |read| read.map_err(|e| stream_body_error(&e, status, attempts)));

        // A terminating adapter over the raced body reads. The parser state
        // lives inside the unfold, so termination is decided BEFORE any body
        // read is polled: once the parser has emitted Done, the next poll
        // ends the stream outright and a stalled body (or a later
        // cancellation) can neither delay termination nor fabricate an
        // error. A read failure yields exactly one typed error — the
        // underlying body is sticky after an error and would re-report it
        // on every poll — and a body that ends without a [DONE] marker
        // still ends the stream cleanly.
        let stream = futures::stream::unfold(
            (reads, cancel, SseParseState::new()),
            move |(mut reads, cancel, mut parser)| async move {
                if parser.finished {
                    return None;
                }
                let next = match cancel.as_ref() {
                    Some(token) => tokio::select! {
                        biased;
                        _ = token.cancelled() => {
                            Some(Err(cancelled_error(attempts)))
                        }
                        read = reads.next() => read,
                    },
                    None => reads.next().await,
                };
                let read = match next {
                    Some(read) => read,
                    None => return None,
                };
                let chunks = match read {
                    Ok(bytes) => {
                        parser.buffer.extend_from_slice(&bytes);
                        parser.emit_chunks()
                    }
                    Err(e) => {
                        parser.finished = true;
                        vec![Err(e)]
                    }
                };
                Some((chunks, (reads, cancel, parser)))
            },
        )
        .flat_map(futures::stream::iter)
        .boxed();

        Ok(stream)
    }
}

/// A mid-stream body-read failure on a success response, classified like
/// `chat()`'s body read: a timeout is `Timeout`; any other read failure on
/// the streamed body is `Parse` — a success status was already delivered, so
/// nothing parseable arrived.
fn stream_body_error(
    error: &reqwest::Error,
    status: reqwest::StatusCode,
    attempts: u32,
) -> PulseHiveError {
    let kind = if error.is_timeout() {
        LlmErrorKind::Timeout
    } else {
        LlmErrorKind::Parse
    };
    PulseHiveError::llm_transport(
        LlmError::new(kind, error.to_string())
            .with_status(status.as_u16())
            .with_attempts(attempts),
    )
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

/// Returns true for the overload statuses whose `Retry-After` header is
/// honored as retry guidance (and reported on the error).
fn honors_retry_after(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 529)
}

/// The exponential backoff ceiling: an honored `Retry-After` never sleeps
/// longer than this, so a hostile or misconfigured header cannot stall the
/// call for hours.
const RETRY_DELAY_CEILING: Duration = Duration::from_secs(8);

/// The delay honored before the next retry: a 429/529 `Retry-After` (already
/// filtered to those statuses) capped at the backoff ceiling, else backoff.
fn honored_retry_delay(retry_after: Option<Duration>, attempt: u32) -> Duration {
    retry_after
        .map(|delay| delay.min(RETRY_DELAY_CEILING))
        .unwrap_or_else(|| retry_delay(attempt))
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

    #[tokio::test]
    async fn test_mark_started_latches_on_first_poll() {
        let started = Arc::new(AtomicBool::new(false));
        let mut future = std::pin::pin!(MarkStarted {
            future: Box::pin(std::future::pending::<()>()),
            started: Arc::clone(&started),
        });
        assert!(
            !started.load(Ordering::Relaxed),
            "not started before the first poll"
        );

        // One poll of the wrapper polls the inner future; a pending result
        // still counts as started.
        let first =
            std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx))).await;
        assert!(first.is_pending());
        assert!(
            started.load(Ordering::Relaxed),
            "started once the future has been polled"
        );
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

    #[test]
    fn test_honored_retry_delay_capped_at_backoff_ceiling() {
        // A hostile Retry-After never sleeps past the backoff ceiling...
        assert_eq!(
            honored_retry_delay(Some(Duration::from_secs(100_000)), 1),
            RETRY_DELAY_CEILING
        );
        // ...but a short one is honored verbatim, and no header means backoff.
        assert_eq!(
            honored_retry_delay(Some(Duration::from_secs(2)), 1),
            Duration::from_secs(2)
        );
        assert_eq!(honored_retry_delay(None, 1), retry_delay(1));
    }

    #[test]
    fn test_retry_after_honored_only_on_overload_statuses() {
        let four_twenty_nine = reqwest::StatusCode::from_u16(429).expect("429 is a valid status");
        let five_hundred = reqwest::StatusCode::from_u16(500).expect("500 is a valid status");
        assert!(honors_retry_after(four_twenty_nine));
        assert!(honors_retry_after(
            reqwest::StatusCode::from_u16(529).expect("529 is a valid status")
        ));
        assert!(!honors_retry_after(five_hundred));
        assert!(!honors_retry_after(
            reqwest::StatusCode::from_u16(401).expect("401 is a valid status")
        ));
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
