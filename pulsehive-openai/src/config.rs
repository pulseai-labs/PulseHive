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
            base_url: sanitized_base_url(&self.base_url),
            model: self.model.clone(),
            timeout_secs: self.timeout_secs,
            max_retries: self.max_retries,
        }
    }
}

/// `base_url` with any URL userinfo (`https://user:pass@host`) removed, so a
/// credential embedded in the endpoint never reaches a log line through the
/// view or `Debug`. The provider keeps dialing the original URL.
///
/// The input is treated as opaque, not parsed: a malformed URL (say
/// `https:/user:pass@host`, missing a slash) is still redacted — redaction
/// must not depend on the URL being well-formed, since the same string is
/// accepted by `with_base_url` and only rejected later at request build.
fn sanitized_base_url(base_url: &str) -> String {
    // Skip the scheme separator: the first ':' and any run of '/'s after it
    // (handles both "://" and the single-slash malformed spelling).
    let authority_start = match base_url.find(':') {
        Some(colon) => {
            colon
                + 1
                + base_url[colon + 1..]
                    .chars()
                    .take_while(|&c| c == '/')
                    .count()
        }
        None => 0,
    };
    let authority = &base_url[authority_start..];
    let authority_end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
    let (authority, rest) = authority.split_at(authority_end);
    match authority.rfind('@') {
        Some(at) => format!(
            "{}{}{}",
            &base_url[..authority_start],
            &authority[at + 1..],
            rest
        ),
        None => base_url.to_string(),
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

// The API key is deliberately absent from Debug and base_url renders with
// its userinfo stripped: a config that reaches a log line or an error report
// must not leak either credential.
impl fmt::Debug for OpenAIConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAIConfig")
            .field("api_key", &"<redacted>")
            .field("base_url", &sanitized_base_url(&self.base_url))
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

    #[test]
    fn userinfo_in_base_url_is_redacted_from_view_and_debug() {
        let secret = "supersecret";
        let config = OpenAIConfig::new("k", "m")
            .with_base_url(format!("https://user:{secret}@api.example.com/v1"));

        let view = config.view();
        assert_eq!(view.base_url, "https://api.example.com/v1");
        assert!(
            !format!("{view:?}").contains(secret),
            "view Debug leaked the URL credential: {view:?}"
        );
        assert!(
            !format!("{config:?}").contains(secret),
            "config Debug leaked the URL credential"
        );

        // A URL without userinfo passes through untouched.
        let plain = OpenAIConfig::new("k", "m").with_base_url("http://localhost:11434/v1");
        assert_eq!(plain.view().base_url, "http://localhost:11434/v1");

        // A malformed URL (missing a slash) is redacted too: with_base_url
        // accepts it, and redaction must not depend on the URL parsing.
        let malformed =
            OpenAIConfig::new("k", "m").with_base_url("https:/user:secret@api.example.com");
        assert!(
            !malformed.view().base_url.contains("secret"),
            "view leaked the credential of a malformed URL: {}",
            malformed.view().base_url
        );
        assert!(
            !format!("{malformed:?}").contains("secret"),
            "Debug leaked the credential of a malformed URL"
        );
        assert_eq!(malformed.view().base_url, "https:/api.example.com");

        // Transport still dials the original URL, credentials included.
        assert_eq!(
            config.chat_completions_url(),
            format!("https://user:{secret}@api.example.com/v1/chat/completions")
        );
    }
}
