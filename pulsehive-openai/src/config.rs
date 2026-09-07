//! Configuration for OpenAI-compatible LLM providers.

use std::fmt;

/// Configuration for connecting to any OpenAI-compatible API.
///
/// Works with OpenAI, GLM (BigModel), vLLM, LM Studio, Ollama, Together, Groq,
/// and any other service exposing the OpenAI chat completions endpoint.
///
/// The `api_key` is never rendered by `Debug` — it prints as `<redacted>` —
/// so a config that reaches a log line does not leak the credential.
///
/// # Example
/// ```
/// use pulsehive_openai::OpenAIConfig;
///
/// // OpenAI (default endpoint)
/// let config = OpenAIConfig::new("sk-...", "gpt-4");
///
/// // GLM-5 via BigModel
/// let config = OpenAIConfig::new("glm-key", "glm-5")
///     .with_base_url("https://open.bigmodel.cn/api/paas/v4");
///
/// // Local Ollama
/// let config = OpenAIConfig::new("unused", "llama3")
///     .with_base_url("http://localhost:11434/v1");
/// ```
#[derive(Clone)]
pub struct OpenAIConfig {
    /// API key for authentication (sent as Bearer token).
    pub api_key: String,
    /// Base URL for the API. Default: `https://api.openai.com/v1`
    pub base_url: String,
    /// Model identifier (e.g., "gpt-4", "glm-5", "llama3").
    pub model: String,
    /// Request timeout in seconds. Default: 60.
    pub timeout_secs: u64,
    /// Maximum retry attempts for transient errors. Default: 3.
    pub max_retries: u32,
}

impl OpenAIConfig {
    /// Creates a config targeting the default OpenAI endpoint.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.openai.com/v1".into(),
            model: model.into(),
            timeout_secs: 60,
            max_retries: 3,
        }
    }

    /// Override the base URL for non-OpenAI endpoints.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Override the request timeout (default: 60 seconds).
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Override the max retry count (default: 3).
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Returns the full chat completions endpoint URL.
    #[allow(dead_code)] // Used in Ticket #13
    pub(crate) fn chat_completions_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}/chat/completions")
    }

    /// The non-secret view [`OpenAICompatibleProvider::config`](crate::OpenAICompatibleProvider::config)
    /// returns: transport settings only, with no path to the API key.
    pub(crate) fn view(&self) -> OpenAIConfigView {
        OpenAIConfigView {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            timeout_secs: self.timeout_secs,
            max_retries: self.max_retries,
        }
    }
}

/// A non-secret view of an [`OpenAIConfig`]: the transport settings a
/// caller may inspect (endpoint, model, timeout, retry budget), with no
/// field or method that reaches the API key.
#[derive(Debug, Clone)]
pub struct OpenAIConfigView {
    /// Base URL of the API endpoint.
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum retry attempts for transient errors.
    pub max_retries: u32,
}

// The API key is deliberately absent from Debug: a config that reaches a log
// line or an error report must not leak the credential.
impl fmt::Debug for OpenAIConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAIConfig")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_retries", &self.max_retries)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = OpenAIConfig::new("sk-test", "gpt-4");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, "gpt-4");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.timeout_secs, 60);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_config_with_base_url() {
        let config =
            OpenAIConfig::new("key", "glm-5").with_base_url("https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(config.base_url, "https://open.bigmodel.cn/api/paas/v4");
    }

    #[test]
    fn test_config_ollama() {
        let config = OpenAIConfig::new("unused", "llama3")
            .with_base_url("http://localhost:11434/v1")
            .with_timeout(120);
        assert_eq!(config.base_url, "http://localhost:11434/v1");
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn test_chat_completions_url() {
        let config = OpenAIConfig::new("k", "m");
        assert_eq!(
            config.chat_completions_url(),
            "https://api.openai.com/v1/chat/completions"
        );

        // Trailing slash should be handled
        let config = config.with_base_url("http://localhost:11434/v1/");
        assert_eq!(
            config.chat_completions_url(),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_config_clone() {
        let config = OpenAIConfig::new("key", "model").with_max_retries(5);
        let cloned = config.clone();
        assert_eq!(cloned.max_retries, 5);
    }
}
