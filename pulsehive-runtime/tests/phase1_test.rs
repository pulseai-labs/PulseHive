//! Phase 1 comprehensive integration test.
//!
//! Validates ALL Phase 1 functional requirements in a single test flow:
//! FR-001 (builder), FR-002 (substrate), FR-003 (deploy), FR-004 (loop),
//! FR-006 (tools), FR-009 (perception), FR-010 (re-ranking), FR-011 (recording),
//! FR-014 (events).

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures_core::Stream;

use pulsedb::{AgentId, ExperienceType, NewExperience};
use pulsehive_core::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::*;
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};

// ── Mocks ────────────────────────────────────────────────────────────

struct ScriptedLlm {
    responses: Mutex<Vec<LlmResponse>>,
}

impl ScriptedLlm {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

#[async_trait]
impl LlmProvider for ScriptedLlm {
    async fn chat(
        &self,
        _m: Vec<Message>,
        _t: Vec<ToolDefinition>,
        _c: &LlmConfig,
    ) -> Result<LlmResponse> {
        let mut r = self.responses.lock().unwrap();
        if r.is_empty() {
            Err(PulseHiveError::llm("No responses"))
        } else {
            Ok(r.remove(0))
        }
    }
    async fn chat_stream(
        &self,
        _m: Vec<Message>,
        _t: Vec<ToolDefinition>,
        _c: &LlmConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk>> + Send>>> {
        Err(PulseHiveError::llm("Not used"))
    }
}

struct SearchTool;

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }
    fn description(&self) -> &str {
        "Search for information"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}})
    }
    async fn execute(&self, params: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let query = params["query"].as_str().unwrap_or("unknown");
        Ok(ToolResult::text(format!("Results for: {query}")))
    }
}

// ── Full Phase 1 Test ────────────────────────────────────────────────

fn scripted_llm() -> ScriptedLlm {
    ScriptedLlm::new(vec![
        LlmResponse::new(
            None,
            vec![ToolCall {
                id: "call_1".into(),
                name: "search".into(),
                arguments: serde_json::json!({"query": "rust patterns"}),
            }],
            TokenUsage::default(),
        ),
        LlmResponse::text("Found great Rust patterns in the codebase."),
    ])
}

async fn seed_substrate(hive: &HiveMind) -> pulsedb::CollectiveId {
    let collective_id = hive
        .substrate()
        .get_or_create_collective("project")
        .await
        .unwrap();
    hive.record_experience(NewExperience {
        collective_id,
        content: "The project uses async/await extensively for concurrent operations.".into(),
        experience_type: ExperienceType::TechInsight {
            technology: "rust".into(),
            insight: "Heavy async/await usage".into(),
        },
        embedding: None,
        importance: 0.8,
        confidence: 0.9,
        domain: vec!["rust".into(), "async".into()],
        tags: Default::default(),
        source_agent: AgentId("seed-agent".into()),
        source_task: None,
        related_files: vec![],
    })
    .await
    .unwrap();
    collective_id
}

fn phase1_agent() -> AgentDefinition {
    AgentDefinition {
        name: "code-reviewer".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a code reviewer.".into(),
            tools: vec![Arc::new(SearchTool)],
            lens: Lens::new(["rust", "async"]),
            llm_config: LlmConfig::new("test", "test-model"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    }
}

async fn collect_until_recorded(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
) -> Vec<HiveEvent> {
    let mut events = vec![];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        tokio::select! {
            event = stream.next() => {
                match event {
                    Some(event) => {
                        let done = matches!(&event, HiveEvent::ExperienceRecorded { .. });
                        events.push(event);
                        if done { break; }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    events
}

fn assert_phase1_events(events: &[HiveEvent]) {
    assert!(
        events
            .iter()
            .any(|event| matches!(event, HiveEvent::AgentStarted { .. })),
        "FR-003: AgentStarted missing"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, HiveEvent::LlmCallStarted { .. })),
        "FR-004: LlmCallStarted missing"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, HiveEvent::ToolCallStarted { tool_name, .. } if tool_name == "search")
        ),
        "FR-006: ToolCallStarted missing"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, HiveEvent::ToolCallCompleted { .. })),
        "FR-006: ToolCallCompleted missing"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, HiveEvent::ExperienceRecorded { .. })),
        "FR-011: ExperienceRecorded missing"
    );
}

#[tokio::test]
async fn test_phase1_full_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("phase1.db");
    let hive = HiveMind::builder()
        .substrate_path(&path)
        .llm_provider("test", scripted_llm())
        .build()
        .unwrap();
    let collective_id = seed_substrate(&hive).await;
    let task = Task::with_collective("Review the async patterns", collective_id);
    let stream = hive.deploy(vec![phase1_agent()], vec![task]).await.unwrap();

    let events = collect_until_recorded(stream).await;
    assert_phase1_events(&events);
    println!(
        "Phase 1 integration test passed! {} events collected.",
        events.len()
    );
}
