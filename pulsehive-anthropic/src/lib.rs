//! Anthropic Claude LLM provider for PulseHive.
//!
//! Supports Claude Opus, Sonnet, and Haiku models via the Messages API
//! with tool use support.
//!
//! # Example
//! ```rust,ignore
//! use pulsehive_anthropic::{AnthropicProvider, AnthropicConfig};
//!
//! let provider = AnthropicProvider::new("sk-ant-...");
//! // Or with custom config:
//! let provider = AnthropicProvider::with_config(
//!     AnthropicConfig::new("sk-ant-...").with_model("claude-opus-4-6")
//! );
//! ```

pub mod config;
pub mod provider;
pub mod types;

pub use config::{AnthropicConfig, AnthropicConfigView};
pub use provider::AnthropicProvider;
