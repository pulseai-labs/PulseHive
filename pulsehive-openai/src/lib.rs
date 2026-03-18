//! OpenAI-compatible LLM provider for PulseHive.
//!
//! Works with any OpenAI-compatible API: OpenAI, GLM, vLLM, LM Studio, Ollama,
//! Together, Groq, and others.
//!
//! # Example
//! ```rust,ignore
//! use pulsehive_openai::{OpenAIConfig, OpenAICompatibleProvider};
//!
//! let provider = OpenAICompatibleProvider::new(
//!     OpenAIConfig::new("sk-...", "gpt-4")
//! );
//! ```

mod config;
mod provider;
pub(crate) mod types;

pub use config::OpenAIConfig;
pub use provider::OpenAICompatibleProvider;
