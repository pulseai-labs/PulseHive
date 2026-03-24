//! Anthropic Claude LLM provider implementation.

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;
use reqwest::Client;

use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::llm::*;

use crate::config::AnthropicConfig;
use crate::types::{self, AnthropicTool, MessagesRequest, MessagesResponse};

/// Anthropic Claude provider implementing the PulseHive LlmProvider trait.
///
/// Supports Claude Opus, Sonnet, and Haiku models via the Messages API
/// with tool use and streaming.
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

        MessagesRequest {
            model,
            max_tokens: self.config.max_tokens,
            system,
            messages: anthropic_messages,
            tools: anthropic_tools,
            stream: if stream { Some(true) } else { None },
        }
    }
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

        let mut last_error = None;
        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << attempt.min(4));
                tokio::time::sleep(delay).await;
            }

            let response = self
                .client
                .post(self.config.messages_url())
                .header("x-api-key", &self.config.api_key)
                .header("anthropic-version", &self.config.anthropic_version)
                .header("content-type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| PulseHiveError::llm(format!("HTTP request failed: {e}")))?;

            let status = response.status();

            // Retry on rate limit (429) or overloaded (529)
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.as_u16() == 529 {
                last_error = Some(PulseHiveError::llm(format!(
                    "Anthropic API rate limited ({}), attempt {}/{}",
                    status,
                    attempt + 1,
                    self.config.max_retries + 1
                )));
                continue;
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(PulseHiveError::llm(format!(
                    "Anthropic API error {}: {}",
                    status, body
                )));
            }

            let body = response
                .json::<MessagesResponse>()
                .await
                .map_err(|e| PulseHiveError::llm(format!("Failed to parse response: {e}")))?;

            return Ok(types::convert_response(body));
        }

        Err(last_error.unwrap_or_else(|| PulseHiveError::llm("Max retries exceeded")))
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
        let config = AnthropicConfig::new("sk-test").with_base_url("http://localhost:1/invalid");
        let provider = AnthropicProvider::with_config(config);

        let result = provider
            .chat(
                vec![Message::user("test")],
                vec![],
                &LlmConfig::new("anthropic", "claude-sonnet-4-6"),
            )
            .await;

        assert!(result.is_err());
    }
}
