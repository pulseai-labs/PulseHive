//! End-to-end tests for the perception + recording pipeline.

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
use pulsehive_runtime::hivemind::{HiveMind, Task};

// ── Mock LLM that echoes perceived context ───────────────────────────

/// Every request the provider received, in call order. The provider is moved
/// into the `HiveMind` at build time, so a test that asserts on what an agent
/// perceived keeps this handle alongside it.
type RecordedRequests = Arc<Mutex<Vec<Vec<Message>>>>;

struct EchoContextLlm {
    responses: Mutex<Vec<LlmResponse>>,
    /// `None` for the suites that do not assert on perceived context.
    recorded: Option<RecordedRequests>,
}

impl EchoContextLlm {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            recorded: None,
        }
    }

    /// Like [`EchoContextLlm::new`], but every `chat` request is also recorded.
    fn recording(responses: Vec<LlmResponse>, recorded: RecordedRequests) -> Self {
        Self {
            responses: Mutex::new(responses),
            recorded: Some(recorded),
        }
    }

    fn text(content: &str) -> LlmResponse {
        LlmResponse::text(content)
    }
}

#[async_trait]
impl LlmProvider for EchoContextLlm {
    async fn chat(
        &self,
        messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _config: &LlmConfig,
    ) -> Result<LlmResponse> {
        if let Some(recorded) = &self.recorded {
            recorded.lock().unwrap().push(messages.clone());
        }
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

async fn build_hive(provider: EchoContextLlm) -> (HiveMind, pulsehive_core::ids::CollectiveId) {
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

    (
        hive,
        pulsehive_core::ids::CollectiveId::from_bytes(*cid.as_bytes()),
    )
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
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
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
            refresh_every_n_tool_calls: None,
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

// ── PH-2: char-safe perception truncation ────────────────────────────

/// An experience whose byte 500 falls inside a multi-byte character: 499 ASCII
/// bytes, then `é` (2 bytes, occupying bytes 499 and 500), then a tail. Byte 500
/// is not a character boundary here, so a `[..500]` byte slice panics while a
/// cut floored to the nearest boundary lands on byte 499.
fn multibyte_boundary_experience() -> String {
    format!("{}é{}", "a".repeat(499), "z".repeat(64))
}

#[tokio::test]
async fn perception_survives_multibyte_boundary_at_500_bytes() {
    let recorded: RecordedRequests = Arc::new(Mutex::new(Vec::new()));
    let provider = EchoContextLlm::recording(
        vec![EchoContextLlm::text("Acknowledged.")],
        Arc::clone(&recorded),
    );
    let (hive, cid) = build_hive(provider).await;

    // Keep the fixture honest: it must straddle the 500-byte cut, with `é` the
    // character across it. An ASCII-only fixture would pass without ever
    // reaching the defect.
    let content = multibyte_boundary_experience();
    assert!(
        !content.is_char_boundary(500),
        "fixture must put byte 500 inside a multi-byte character"
    );
    assert_eq!(&content[499..501], "é");

    // Seed the experience the deployed agent will perceive. `get_or_create_collective`
    // is idempotent, so this is the same collective `build_hive` created.
    let db_cid = hive
        .substrate()
        .get_or_create_collective("test")
        .await
        .unwrap();
    hive.record_experience(pulsedb::NewExperience {
        collective_id: db_cid,
        content: content.clone(),
        experience_type: pulsedb::ExperienceType::Generic { category: None },
        embedding: None,
        importance: 0.9,
        confidence: 0.9,
        domain: vec![],
        source_agent: pulsedb::AgentId("seeder".into()),
        source_task: None,
        tags: Default::default(),
        related_files: vec![],
    })
    .await
    .unwrap();

    let agent = AgentDefinition {
        name: "perceiver".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a test agent.".into(),
            tools: vec![],
            lens: Lens::default(),
            llm_config: LlmConfig::new("mock", "test"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    };

    let task = Task::with_collective("Perceive a long experience", cid);
    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // The run completed: building the agent's context did not panic.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::LlmCallCompleted { .. })),
        "Agent should have completed LLM call. Events: {events:?}"
    );

    // The request the provider received carries the experience cut back to byte
    // 499 — the nearest character boundary at or below the 500-byte budget —
    // marked with the same `...` the extractor appends.
    let expected = format!("• You understand that {}...", "a".repeat(499));
    let recorded = recorded.lock().unwrap();
    assert!(
        recorded.iter().flatten().any(|m| match m {
            Message::System { content } => content.contains(&expected),
            _ => false,
        }),
        "Perceived context should carry the char-boundary cut at byte 499. Requests: {recorded:?}"
    );
}
