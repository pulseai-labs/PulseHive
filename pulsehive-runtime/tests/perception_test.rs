//! End-to-end tests for the perception + recording pipeline.

use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures_core::Stream;

use pulsehive_core::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::*;
use pulsehive_runtime::hivemind::{HiveMind, Task};

// ── Mock LLM that echoes perceived context ───────────────────────────

struct EchoContextLlm {
    responses: Mutex<Vec<LlmResponse>>,
}

impl EchoContextLlm {
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
}

#[async_trait]
impl LlmProvider for EchoContextLlm {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _config: &LlmConfig,
    ) -> Result<LlmResponse> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Err(PulseHiveError::llm("No more responses"))
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
        Err(PulseHiveError::llm("Not used"))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

async fn build_hive(provider: EchoContextLlm) -> (HiveMind, pulsedb::CollectiveId) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    Box::leak(Box::new(dir));

    let hive = HiveMind::builder()
        .substrate_path(&path)
        .llm_provider("mock", provider)
        .build()
        .unwrap();

    let cid = hive
        .substrate()
        .get_or_create_collective("test")
        .await
        .unwrap();

    (hive, cid)
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
                        let is_done = matches!(&e, HiveEvent::ExperienceRecorded { .. } | HiveEvent::AgentCompleted { .. });
                        events.push(e);
                        if is_done { break; }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    events
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_agent_records_experience_after_completion() {
    let provider = EchoContextLlm::new(vec![EchoContextLlm::text("Task completed successfully.")]);
    let (hive, cid) = build_hive(provider).await;

    let agent = AgentDefinition {
        name: "recorder".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a test agent.".into(),
            tools: vec![],
            lens: Lens::default(),
            llm_config: LlmConfig::new("mock", "test"),
            experience_extractor: None, // Uses DefaultExperienceExtractor
        })),
    };

    let task = Task::with_collective("Do something useful", cid);
    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // Should have recorded an experience (comes before AgentCompleted in the pipeline)
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::ExperienceRecorded { .. })),
        "Missing ExperienceRecorded. Events: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::LlmCallCompleted { .. })),
        "Missing LlmCallCompleted. Events: {events:?}"
    );
}

#[tokio::test]
async fn test_empty_substrate_perception_works() {
    // Agent with no prior experiences — should still work
    let provider = EchoContextLlm::new(vec![EchoContextLlm::text("No context needed.")]);
    let (hive, cid) = build_hive(provider).await;

    let agent = AgentDefinition {
        name: "fresh-agent".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a test agent.".into(),
            tools: vec![],
            lens: Lens::default(),
            llm_config: LlmConfig::new("mock", "test"),
            experience_extractor: None,
        })),
    };

    let task = Task::with_collective("Work on empty substrate", cid);
    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // Should have key events — agent processed without errors
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::LlmCallCompleted { .. })),
        "Agent should have completed LLM call. Events: {events:?}"
    );
}
