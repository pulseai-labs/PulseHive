//! Integration tests for the full PulseHive deploy pipeline.
//!
//! Tests the complete flow: HiveMind → deploy() → agentic loop → event stream.

use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures_core::Stream;
use serde_json::Value;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::*;
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};

// ── Mock LLM Provider ────────────────────────────────────────────────

struct MockLlm {
    responses: Mutex<Vec<LlmResponse>>,
}

impl MockLlm {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }

    fn text(content: &str) -> LlmResponse {
        LlmResponse {
            content: Some(content.into()),
            tool_calls: vec![],
            usage: TokenUsage::default(),
        }
    }

    fn tool_call(id: &str, name: &str, args: Value) -> LlmResponse {
        LlmResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: name.into(),
                arguments: args,
            }],
            usage: TokenUsage::default(),
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlm {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _config: &LlmConfig,
    ) -> Result<LlmResponse> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Err(PulseHiveError::llm("No more scripted responses"))
        } else {
            Ok(responses.remove(0))
        }
    }

    async fn chat_stream(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _config: &LlmConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk>> + Send>>> {
        Err(PulseHiveError::llm("Not used in tests"))
    }
}

// ── Mock Tool ────────────────────────────────────────────────────────

struct SearchTool;

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }
    fn description(&self) -> &str {
        "Search for information"
    }
    fn parameters(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}})
    }
    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let query = params["query"].as_str().unwrap_or("unknown");
        Ok(ToolResult::text(format!("Results for: {query}")))
    }
}

// ── Helper ───────────────────────────────────────────────────────────

fn build_hive(provider: MockLlm) -> HiveMind {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    // Leak tempdir so it outlives the test
    Box::leak(Box::new(dir));

    HiveMind::builder()
        .substrate_path(&path)
        .llm_provider("mock", provider)
        .build()
        .unwrap()
}

fn llm_agent(name: &str, tools: Vec<Box<dyn Tool>>) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a test agent.".into(),
            tools,
            lens: Lens::default(),
            llm_config: LlmConfig::new("mock", "test-model"),
            experience_extractor: None,
        })),
    }
}

/// Collect events from stream with a timeout.
async fn collect_events(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    timeout: Duration,
) -> Vec<HiveEvent> {
    let mut events = vec![];
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        tokio::select! {
            event = stream.next() => {
                match event {
                    Some(e) => {
                        let is_completed = matches!(&e, HiveEvent::AgentCompleted { .. });
                        events.push(e);
                        if is_completed { break; }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                break;
            }
        }
    }

    events
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_deploy_single_agent_text_only() {
    let provider = MockLlm::new(vec![MockLlm::text("The answer is 42.")]);
    let hive = build_hive(provider);

    let agent = llm_agent("answerer", vec![]);
    let task = Task::new("What is the meaning of life?");

    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // Should have AgentStarted and AgentCompleted
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::AgentStarted { name, .. } if name == "answerer")),
        "Missing AgentStarted event. Events: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::AgentCompleted { outcome: AgentOutcome::Complete { response }, .. }
            if response == "The answer is 42."
        )),
        "Missing AgentCompleted event. Events: {events:?}"
    );
}

#[tokio::test]
async fn test_deploy_agent_with_tool_call() {
    let provider = MockLlm::new(vec![
        MockLlm::tool_call("call_1", "search", serde_json::json!({"query": "rust"})),
        MockLlm::text("Found info about Rust."),
    ]);
    let hive = build_hive(provider);

    let agent = llm_agent("researcher", vec![Box::new(SearchTool)]);
    let task = Task::new("Research Rust programming");

    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // Verify full event sequence
    assert!(events
        .iter()
        .any(|e| matches!(e, HiveEvent::AgentStarted { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, HiveEvent::LlmCallStarted { .. })));
    assert!(events.iter().any(
        |e| matches!(e, HiveEvent::ToolCallStarted { tool_name, .. } if tool_name == "search")
    ));
    assert!(events.iter().any(
        |e| matches!(e, HiveEvent::ToolCallCompleted { tool_name, .. } if tool_name == "search")
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        HiveEvent::AgentCompleted {
            outcome: AgentOutcome::Complete { response },
            ..
        } if response == "Found info about Rust."
    )));
}

#[tokio::test]
async fn test_deploy_missing_provider_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");

    // Build without registering any providers
    let hive = HiveMind::builder().substrate_path(&path).build().unwrap();

    let agent = llm_agent("test", vec![]);
    let task = Task::new("Do something");

    let result = hive.deploy(vec![agent], vec![task]).await;
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(
        err.to_string().contains("not registered"),
        "Expected 'not registered' error, got: {err}"
    );
}

#[tokio::test]
async fn test_deploy_empty_agents_returns_empty_stream() {
    let provider = MockLlm::new(vec![]);
    let hive = build_hive(provider);

    let mut stream = hive.deploy(vec![], vec![]).await.unwrap();
    assert!(stream.next().await.is_none());
}
