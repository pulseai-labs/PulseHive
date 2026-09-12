//! Transport-only footprint proof (ADR-013): a consumer that enables only the
//! provider features on the meta crate still constructs both providers and the
//! shared LLM config — the execution engine and its storage layer never enter
//! the build. Runs under `openai,anthropic` and under `--all-features`; makes
//! no network calls.

use std::sync::Arc;

use pulsehive::llm::LlmConfig;
use pulsehive::prelude::LlmProvider;
use pulsehive::pulsehive_anthropic::AnthropicProvider;
use pulsehive::pulsehive_openai::{OpenAICompatibleProvider, OpenAIConfig};

#[test]
fn providers_construct_and_coerce_without_the_execution_engine() {
    let openai: Arc<dyn LlmProvider> = Arc::new(OpenAICompatibleProvider::new(OpenAIConfig::new(
        "sk-test", "gpt-4o",
    )));
    let anthropic: Arc<dyn LlmProvider> = Arc::new(AnthropicProvider::new("sk-ant-test"));

    let config = LlmConfig::new("openai", "gpt-4o");

    let _ = (openai, anthropic, config);
}
