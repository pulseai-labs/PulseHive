//! Configuration for the Anthropic Claude API.

use std::fmt;

/// Configuration for connecting to the Anthropic Messages API.
///
/// The `api_key` is never rendered by `Debug` — it prints as `<redacted>` —
/// so a config that reaches a log line does not leak the credential.
///
/// # Example
/// ```rust,ignore
/// use pulsehive_anthropic::AnthropicConfig;
///
/// let config = AnthropicConfig::new("sk-ant-...");
/// let config = AnthropicConfig::new("sk-ant-...").with_model("claude-opus-4-6");
/// ```
#[derive(Clone)]
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

    /// The non-secret view [`AnthropicProvider::config`](crate::AnthropicProvider::config)
    /// returns: transport settings only, with no path to the API key.
    pub(crate) fn view(&self) -> AnthropicConfigView {
        AnthropicConfigView {
            base_url: sanitized_base_url(&self.base_url),
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            timeout_secs: self.timeout_secs,
            max_retries: self.max_retries,
            anthropic_version: self.anthropic_version.clone(),
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

/// A non-secret view of an [`AnthropicConfig`]: the transport settings a
/// caller may inspect (endpoint, model, timeout, retry budget), with no
/// field or method that reaches the API key.
#[derive(Debug, Clone)]
pub struct AnthropicConfigView {
    /// Base URL of the API endpoint.
    pub base_url: String,
    /// Default model.
    pub model: String,
    /// Default max tokens for responses.
    pub max_tokens: u32,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum retry attempts for transient errors.
    pub max_retries: u32,
    /// Anthropic API version header.
    pub anthropic_version: String,
}

// The API key is deliberately absent from Debug and base_url renders with
// its userinfo stripped: a config that reaches a log line or an error report
// must not leak either credential.
impl fmt::Debug for AnthropicConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicConfig")
            .field("api_key", &"<redacted>")
            .field("base_url", &sanitized_base_url(&self.base_url))
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_retries", &self.max_retries)
            .field("anthropic_version", &self.anthropic_version)
            .finish()
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

    #[test]
    fn userinfo_in_base_url_is_redacted_from_view_and_debug() {
        let secret = "supersecret";
        let config = AnthropicConfig::new("k")
            .with_base_url(format!("https://user:{secret}@api.example.com"));

        let view = config.view();
        assert_eq!(view.base_url, "https://api.example.com");
        assert!(
            !format!("{view:?}").contains(secret),
            "view Debug leaked the URL credential: {view:?}"
        );
        assert!(
            !format!("{config:?}").contains(secret),
            "config Debug leaked the URL credential"
        );

        // A URL without userinfo passes through untouched.
        let plain = AnthropicConfig::new("k").with_base_url("http://localhost:9999");
        assert_eq!(plain.view().base_url, "http://localhost:9999");

        // A malformed URL (missing a slash) is redacted too: with_base_url
        // accepts it, and redaction must not depend on the URL parsing.
        let malformed =
            AnthropicConfig::new("k").with_base_url("https:/user:secret@api.example.com");
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
            config.messages_url(),
            format!("https://user:{secret}@api.example.com/v1/messages")
        );
    }
}
