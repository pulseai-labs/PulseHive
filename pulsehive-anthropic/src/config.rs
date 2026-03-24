//! Configuration for the Anthropic Claude API.

/// Configuration for connecting to the Anthropic Messages API.
///
/// # Example
/// ```rust,ignore
/// use pulsehive_anthropic::AnthropicConfig;
///
/// let config = AnthropicConfig::new("sk-ant-...");
/// let config = AnthropicConfig::new("sk-ant-...").with_model("claude-opus-4-6");
/// ```
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    /// Anthropic API key (sent via `x-api-key` header).
    pub api_key: String,
    /// Base URL for the API. Default: `https://api.anthropic.com`
    pub base_url: String,
    /// Default model. Default: `claude-sonnet-4-6`
    pub model: String,
    /// Default max tokens for responses. Default: 4096.
    pub max_tokens: u32,
    /// Request timeout in seconds. Default: 120.
    pub timeout_secs: u64,
    /// Maximum retry attempts for transient errors. Default: 3.
    pub max_retries: u32,
    /// Anthropic API version header. Default: `2023-06-01`
    pub anthropic_version: String,
}

impl AnthropicConfig {
    /// Create a config with the given API key and default settings.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com".into(),
            model: "claude-sonnet-4-6".into(),
            max_tokens: 4096,
            timeout_secs: 120,
            max_retries: 3,
            anthropic_version: "2023-06-01".into(),
        }
    }

    /// Override the model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the base URL (for proxies or testing).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// The full messages endpoint URL.
    pub fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = AnthropicConfig::new("sk-test");
        assert_eq!(config.base_url, "https://api.anthropic.com");
        assert_eq!(config.model, "claude-sonnet-4-6");
        assert_eq!(config.max_tokens, 4096);
        assert_eq!(config.anthropic_version, "2023-06-01");
    }

    #[test]
    fn test_messages_url() {
        let config = AnthropicConfig::new("sk-test");
        assert_eq!(
            config.messages_url(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn test_config_with_model() {
        let config = AnthropicConfig::new("sk-test").with_model("claude-opus-4-6");
        assert_eq!(config.model, "claude-opus-4-6");
    }
}
