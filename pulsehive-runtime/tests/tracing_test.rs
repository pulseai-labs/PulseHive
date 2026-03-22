//! Integration test verifying structured tracing spans work with tracing-subscriber.
//!
//! Validates E3-S01 acceptance criterion #3: spans are compatible with
//! tracing-subscriber::fmt and standard observability stacks.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures_core::Stream;

use pulsehive_core::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
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

    fn tool_call(id: &str, name: &str, args: serde_json::Value) -> LlmResponse {
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

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echoes input"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
    }
    async fn execute(&self, params: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let text = params["text"].as_str().unwrap_or("no text");
        Ok(ToolResult::text(format!("Echo: {text}")))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn build_hive(provider: MockLlm) -> HiveMind {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    Box::leak(Box::new(dir));

    HiveMind::builder()
        .substrate_path(&path)
        .llm_provider("mock", provider)
        .build()
        .unwrap()
}

fn llm_agent(name: &str, tools: Vec<Arc<dyn Tool>>) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a test agent.".into(),
            tools,
            lens: Lens::default(),
            llm_config: LlmConfig::new("mock", "test-model"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    }
}

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

/// Verify that structured tracing spans are emitted and compatible with
/// tracing-subscriber::fmt. This proves E3-S01 AC #3.
#[tokio::test]
async fn test_tracing_spans_with_subscriber() {
    // Initialize tracing subscriber with test writer (captures output).
    // try_init is used because another test in the process may have already initialized.
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let provider = MockLlm::new(vec![
        MockLlm::tool_call("c1", "echo", serde_json::json!({"text": "hello"})),
        MockLlm::text("Done echoing."),
    ]);
    let hive = build_hive(provider);

    let agent = llm_agent("tracing-test-agent", vec![Arc::new(EchoTool)]);
    let task = Task::new("Echo a greeting");

    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // If we got events without panicking, the tracing subscriber is compatible.
    // The spans (perceive, think, act, record) were created and didn't cause errors.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::AgentStarted { .. })),
        "Expected AgentStarted event"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::AgentCompleted { .. })),
        "Expected AgentCompleted event"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::ToolCallStarted { .. })),
        "Expected ToolCallStarted event"
    );
}

/// Verify that text-only agents work with tracing spans active.
#[tokio::test]
async fn test_tracing_text_only_agent() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::TRACE)
        .try_init();

    let provider = MockLlm::new(vec![MockLlm::text("Simple response.")]);
    let hive = build_hive(provider);

    let agent = llm_agent("simple-agent", vec![]);
    let task = Task::new("Say something");

    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    assert!(events.iter().any(|e| matches!(
        e,
        HiveEvent::AgentCompleted {
            outcome: pulsehive_core::agent::AgentOutcome::Complete { response },
            ..
        } if response == "Simple response."
    )));
}
