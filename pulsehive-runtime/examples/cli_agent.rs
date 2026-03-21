//! PulseHive CLI Agent Example
//!
//! Demonstrates deploying a single agent with a tool. Run with:
//! ```bash
//! cargo run -p pulsehive-runtime --example cli_agent
//! ```

use std::pin::Pin;
use std::sync::Arc;

use futures::StreamExt;
use pulsehive_core::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::*;
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};

// A simple tool that returns the current time
struct GetTime;

#[async_trait::async_trait]
impl Tool for GetTime {
    fn name(&self) -> &str {
        "get_time"
    }
    fn description(&self) -> &str {
        "Returns the current UTC time"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    async fn execute(
        &self,
        _p: serde_json::Value,
        _c: &ToolContext,
    ) -> pulsehive_core::error::Result<ToolResult> {
        Ok(ToolResult::text("Current time: 2026-03-21T12:00:00Z"))
    }
}

// Mock LLM that returns a simple response (no API key needed)
struct MockLlm;

#[async_trait::async_trait]
impl LlmProvider for MockLlm {
    async fn chat(
        &self,
        _m: Vec<Message>,
        _t: Vec<ToolDefinition>,
        _c: &LlmConfig,
    ) -> pulsehive_core::error::Result<LlmResponse> {
        Ok(LlmResponse {
            content: Some("Hello from PulseHive!".into()),
            tool_calls: vec![],
            usage: TokenUsage::default(),
        })
    }
    async fn chat_stream(
        &self,
        _m: Vec<Message>,
        _t: Vec<ToolDefinition>,
        _c: &LlmConfig,
    ) -> pulsehive_core::error::Result<
        Pin<Box<dyn futures_core::Stream<Item = pulsehive_core::error::Result<LlmChunk>> + Send>>,
    > {
        Err(pulsehive_core::error::PulseHiveError::llm("Use chat()"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let hive = HiveMind::builder()
        .substrate_path("my_agent.db")
        .llm_provider("mock", MockLlm)
        .build()?;

    let agent = AgentDefinition {
        name: "assistant".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a helpful assistant.".into(),
            tools: vec![Arc::new(GetTime)],
            lens: Lens::new(["general"]),
            llm_config: LlmConfig::new("mock", "demo"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    };

    let mut stream = hive
        .deploy(vec![agent], vec![Task::new("Say hello")])
        .await?;
    while let Some(event) = stream.next().await {
        println!("{event:?}");
    }
    Ok(())
}
