//! Anthropic Claude LLM provider implementation.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;
use reqwest::Client;

use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::llm::*;

use crate::config::AnthropicConfig;
use crate::types::{self, AnthropicTool, AnthropicToolChoice, MessagesRequest, MessagesResponse};

/// Anthropic Claude provider implementing the PulseHive LlmProvider trait.
///
/// Supports Claude Opus, Sonnet, and Haiku models via the Messages API
/// with tool use.
///
/// # Transport behaviour
///
/// Transport failures surface as
/// [`PulseHiveError::LlmTransport`](pulsehive_core::error::PulseHiveError::LlmTransport)
/// carrying an [`LlmError`](pulsehive_core::llm::LlmError). A timeout fails
/// once (`Timeout`) and is never re-sent; connection failures and
/// 429/500/502/503/529 retry within the attempt budget, honouring an
/// integer-seconds `Retry-After` header on 429/529; any other 4xx/5xx fails
/// immediately; a success status with an unreadable or unparseable body is
/// `Parse`, while a body that cannot be read on an error status is a
/// Connect-class failure that retries.
/// `stop_reason` is reported verbatim as `finish_reason`, and `reasoning` is
/// always `None`. [`LlmConfig::timeout_secs`] and [`LlmConfig::max_retries`]
/// override this provider's configured values for a single call, and a
/// cancelled [`LlmConfig::cancel`] token aborts the in-flight request.
///
/// `LlmConfig::reasoning_effort` is **accepted and ignored**: it is never
/// sent on the wire — neither a `reasoning_effort` nor a `thinking`
/// parameter. Mapping it to Anthropic extended thinking is a recorded
/// feature-map entry, not provider parity.
///
/// Streaming (`chat_stream`) is not supported by this provider today; every
/// call returns a not-supported error.
pub struct AnthropicProvider {
    config: AnthropicConfig,
    client: Client,
}

impl AnthropicProvider {
    /// Create a provider with the given API key and default settings.
    pub fn new(api_key: impl Into<String>) -> Self {
        let config = AnthropicConfig::new(api_key);
        Self::with_config(config)
    }

    /// Create a provider with custom configuration.
    pub fn with_config(config: AnthropicConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to build HTTP client");
        Self { config, client }
    }

    /// The provider's configuration.
    pub fn config(&self) -> &AnthropicConfig {
        &self.config
    }

    /// Build the request body for the Messages API.
    fn build_request(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &LlmConfig,
        stream: bool,
    ) -> MessagesRequest {
        let (system, anthropic_messages) = types::convert_messages(messages);
        let anthropic_tools: Vec<AnthropicTool> = tools.iter().map(AnthropicTool::from).collect();
        let model = if config.model == "default" {
            self.config.model.clone()
        } else {
            config.model.clone()
        };
        let tool_choice = config.tool_choice.as_ref().map(AnthropicToolChoice::from);
        if config.reasoning_effort.is_some() {
            tracing::debug!(
                "reasoning_effort is set on LlmConfig but is ignored by the Anthropic \
                 provider: it is not sent on the wire (extended thinking is a recorded \
                 feature-map entry, not a parity item)"
            );
        }

        MessagesRequest {
            model,
            max_tokens: self.config.max_tokens,
            system,
            messages: anthropic_messages,
            tools: anthropic_tools,
            stream: if stream { Some(true) } else { None },
            tool_choice,
        }
    }

    /// Send one Messages API request, applying the per-call overrides and
    /// this crate's retry policy.
    ///
    /// `config.max_retries` wins over `self.config.max_retries`;
    /// `config.timeout_secs` overrides the client timeout for this request;
    /// a cancelled `config.cancel` token aborts the in-flight exchange.
    /// Every `Err` on the transport path is a typed `LlmTransport` error;
    /// `PulseHiveError::llm(..)` remains reserved for request-build
    /// failures, which this path cannot produce.
    async fn send_request(
        &self,
        request_body: &MessagesRequest,
        config: &LlmConfig,
    ) -> Result<LlmResponse> {
        let max_attempts = config.max_retries.unwrap_or(self.config.max_retries) + 1;
        let mut attempts: u32 = 0;

        loop {
            // Cancellation is checked before every attempt, so a token
            // cancelled before the first send returns with attempts == 0.
            if config
                .cancel
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                return Err(Self::cancelled_error(attempts));
            }

            attempts += 1;

            let mut request = self
                .client
                .post(self.config.messages_url())
                .header("x-api-key", &self.config.api_key)
                .header("anthropic-version", &self.config.anthropic_version)
                .header("content-type", "application/json")
                .json(&request_body);
            if let Some(timeout_secs) = config.timeout_secs {
                request = request.timeout(Duration::from_secs(timeout_secs));
            }

            // Phase 1 — send. A timeout here fails once and is never
            // re-sent; any other transport failure (connection refused,
            // reset, ...) retries within the attempt budget.
            let response = match self.race_cancel(request.send(), config, attempts).await? {
                Ok(response) => response,
                Err(e) if e.is_timeout() => return Err(Self::timeout_error(&e, attempts)),
                Err(e) => {
                    if attempts >= max_attempts {
                        return Err(Self::connect_error(&e, attempts));
                    }
                    self.sleep_cancelled(backoff_delay(attempts), config, attempts)
                        .await?;
                    continue;
                }
            };
            let status = response.status();
            let retry_after = parse_retry_after(response.headers());

            // Phase 2 — read the body, still raced against cancellation.
            // The mapping depends on the phase and the status: a timeout
            // fails once whatever the status; a non-timeout read failure is
            // a Parse error on a success status (nothing parseable arrived)
            // and a Connect-class failure that retries within the budget on
            // any other status.
            let body_raw = match self.race_cancel(response.text(), config, attempts).await? {
                Ok(body) => body,
                Err(e) if e.is_timeout() => return Err(Self::timeout_error(&e, attempts)),
                Err(_) if status.is_success() => {
                    // No partial body is recoverable from a dropped read, so
                    // the error carries the status and no body.
                    return Err(PulseHiveError::llm_transport(
                        LlmError::new(LlmErrorKind::Parse, "response body could not be read")
                            .with_status(status.as_u16())
                            .with_attempts(attempts),
                    ));
                }
                Err(e) => {
                    if attempts >= max_attempts {
                        return Err(Self::connect_error(&e, attempts));
                    }
                    self.sleep_cancelled(backoff_delay(attempts), config, attempts)
                        .await?;
                    continue;
                }
            };

            let status_code = status.as_u16();
            if matches!(status_code, 429 | 500 | 502 | 503 | 529) {
                let kind = if status_code == 429 {
                    LlmErrorKind::RateLimited
                } else {
                    LlmErrorKind::ServerError
                };
                if attempts >= max_attempts {
                    let mut error = LlmError::new(kind, format!("Anthropic API error {status}"))
                        .with_status(status_code)
                        .with_attempts(attempts)
                        .with_body(&body_raw);
                    if matches!(status_code, 429 | 529) {
                        if let Some(delay) = retry_after {
                            error = error.with_retry_after(delay);
                        }
                    }
                    return Err(PulseHiveError::llm_transport(error));
                }
                // An integer-seconds Retry-After wins over backoff on
                // 429/529; everything else uses this crate's backoff shape.
                let delay = match retry_after {
                    Some(delay) if matches!(status_code, 429 | 529) => delay,
                    _ => backoff_delay(attempts),
                };
                self.sleep_cancelled(delay, config, attempts).await?;
                continue;
            }

            if status.is_success() {
                let parsed: MessagesResponse = serde_json::from_str(&body_raw).map_err(|e| {
                    PulseHiveError::llm_transport(
                        LlmError::new(
                            LlmErrorKind::Parse,
                            format!("failed to parse response: {e}"),
                        )
                        .with_status(status_code)
                        .with_attempts(attempts)
                        .with_body(&body_raw),
                    )
                })?;
                return match types::convert_response(parsed) {
                    Ok(response) => Ok(response),
                    Err(PulseHiveError::LlmTransport(mut error)) => {
                        error.attempts = attempts;
                        Err(PulseHiveError::LlmTransport(error))
                    }
                    Err(other) => Err(other),
                };
            }

            // Non-retryable failure status: other 5xx stays ServerError,
            // anything else is a ClientError carrying the Anthropic error
            // envelope's message when the body parses as one.
            let kind = if (500..600).contains(&status_code) {
                LlmErrorKind::ServerError
            } else {
                LlmErrorKind::ClientError
            };
            let message = if kind == LlmErrorKind::ClientError {
                serde_json::from_str::<types::AnthropicError>(&body_raw)
                    .map(|envelope| envelope.error.message)
                    .unwrap_or_else(|_| body_raw.clone())
            } else {
                format!("Anthropic API error {status}")
            };
            return Err(PulseHiveError::llm_transport(
                LlmError::new(kind, message)
                    .with_status(status_code)
                    .with_attempts(attempts)
                    .with_body(&body_raw),
            ));
        }
    }

    /// Race a future against the call's cancellation token, if any. Dropping
    /// the future on cancellation aborts the in-flight request.
    async fn race_cancel<F: Future>(
        &self,
        future: F,
        config: &LlmConfig,
        attempts: u32,
    ) -> Result<F::Output> {
        let cancel = config.cancel.as_ref().map(|token| token.cancelled());
        match cancel {
            Some(cancelled) => tokio::select! {
                _ = cancelled => Err(Self::cancelled_error(attempts)),
                output = future => Ok(output),
            },
            None => Ok(future.await),
        }
    }

    /// A backoff sleep raced against cancellation.
    async fn sleep_cancelled(
        &self,
        delay: Duration,
        config: &LlmConfig,
        attempts: u32,
    ) -> Result<()> {
        self.race_cancel(tokio::time::sleep(delay), config, attempts)
            .await
            .map(|_| ())
    }

    /// The typed cancellation error, carrying the sends made so far.
    fn cancelled_error(attempts: u32) -> PulseHiveError {
        PulseHiveError::llm_transport(
            LlmError::new(
                LlmErrorKind::Cancelled,
                "call cancelled by the caller's token",
            )
            .with_attempts(attempts),
        )
    }

    /// The typed, never-retried timeout error. The warn log always names the
    /// timeout, never "connection error" — used by both the send phase and
    /// the body-read phase.
    fn timeout_error(error: &reqwest::Error, attempts: u32) -> PulseHiveError {
        tracing::warn!("anthropic request timed out after {attempts} attempt(s); not retrying");
        PulseHiveError::llm_transport(
            LlmError::new(LlmErrorKind::Timeout, format!("request timed out: {error}"))
                .with_attempts(attempts),
        )
    }

    /// The typed Connect error returned once the attempt budget is spent —
    /// used by the send phase and by a failed body read on a non-success
    /// status.
    fn connect_error(error: &reqwest::Error, attempts: u32) -> PulseHiveError {
        PulseHiveError::llm_transport(
            LlmError::new(LlmErrorKind::Connect, format!("transport failure: {error}"))
                .with_attempts(attempts),
        )
    }
}

/// This crate's own backoff shape, unchanged from 2.0.x: exponential
/// `1 << n` seconds capped at 16s, where `n` counts failed attempts so far.
fn backoff_delay(failed_attempts: u32) -> Duration {
    Duration::from_secs(1u64 << failed_attempts.min(4))
}

/// Parse an integer-seconds `Retry-After` header, when present and integral.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers.get("retry-after")?.to_str().ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<LlmResponse> {
        let request_body = self.build_request(&messages, &tools, config, false);
        self.send_request(&request_body, config).await
    }

    async fn chat_stream(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _config: &LlmConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk>> + Send>>> {
        // Streaming implementation — returns a basic error for now.
        // Full SSE parsing to be implemented in Ticket #74.
        Err(PulseHiveError::llm(
            "Anthropic streaming not yet implemented (coming in Ticket #74)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_construction() {
        let provider = AnthropicProvider::new("sk-test");
        assert_eq!(provider.config.api_key, "sk-test");
        assert_eq!(provider.config.model, "claude-sonnet-4-6");
    }

    #[test]
    fn test_build_request_basic() {
        let provider = AnthropicProvider::new("sk-test");
        let messages = vec![Message::system("You are helpful"), Message::user("Hello")];
        let config = LlmConfig::new("anthropic", "claude-sonnet-4-6");
        let request = provider.build_request(&messages, &[], &config, false);

        assert_eq!(request.model, "claude-sonnet-4-6");
        assert_eq!(request.system, Some("You are helpful".into()));
        assert_eq!(request.messages.len(), 1); // Only user message (system extracted)
        assert!(request.stream.is_none());
    }

    #[test]
    fn test_build_request_with_tools() {
        let provider = AnthropicProvider::new("sk-test");
        let messages = vec![Message::user("Search for rust")];
        let tools = vec![ToolDefinition {
            name: "search".into(),
            description: "Search the web".into(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let config = LlmConfig::new("anthropic", "default");
        let request = provider.build_request(&messages, &tools, &config, false);

        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "search");
        // "default" model should fall back to config.model
        assert_eq!(request.model, "claude-sonnet-4-6");
    }

    #[test]
    fn test_build_request_stream_flag() {
        let provider = AnthropicProvider::new("sk-test");
        let messages = vec![Message::user("Hello")];
        let config = LlmConfig::new("anthropic", "claude-sonnet-4-6");
        let request = provider.build_request(&messages, &[], &config, true);
        assert_eq!(request.stream, Some(true));
    }

    #[test]
    fn test_provider_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<AnthropicProvider>();
    }

    #[tokio::test]
    async fn test_chat_with_invalid_url_returns_error() {
        let mut config =
            AnthropicConfig::new("sk-test").with_base_url("http://localhost:1/invalid");
        // Connection failures now retry within the attempt budget; keep this
        // unit test fast by spending none.
        config.max_retries = 0;
        let provider = AnthropicProvider::with_config(config);

        let result = provider
            .chat(
                vec![Message::user("test")],
                vec![],
                &LlmConfig::new("anthropic", "claude-sonnet-4-6"),
            )
            .await;

        // Transport failures are typed LlmTransport errors (r1.s1.w3): a
        // refused connection classifies as Connect after one attempt.
        match result {
            Err(PulseHiveError::LlmTransport(err)) => {
                assert_eq!(err.kind, LlmErrorKind::Connect);
                assert_eq!(err.attempts, 1);
            }
            other => panic!("expected typed LlmTransport error, got: {other:?}"),
        }
    }
}
