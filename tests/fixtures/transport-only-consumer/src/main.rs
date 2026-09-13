use std::sync::Arc;

use pulsehive::prelude::LlmProvider;
use pulsehive::pulsehive_openai::{OpenAICompatibleProvider, OpenAIConfig};

fn main() {
    let provider =
        OpenAICompatibleProvider::new(OpenAIConfig::new("transport-only-fixture", "gpt-4o-mini"));
    let model = provider.config().model;
    let _provider: Arc<dyn LlmProvider> = Arc::new(provider);
    println!("transport-only consumer ready: model={model}");
}
