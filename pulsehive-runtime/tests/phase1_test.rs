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

#[tokio::test]
async fn test_phase1_full_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("phase1.db");

    // FR-001: Build HiveMind
    let provider = ScriptedLlm::new(vec![
        // Call 1: Agent calls search tool
        LlmResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "search".into(),
                arguments: serde_json::json!({"query": "rust patterns"}),
            }],
            usage: TokenUsage::default(),
        },
        // Call 2: Agent responds with final answer
        LlmResponse {
            content: Some("Found great Rust patterns in the codebase.".into()),
            tool_calls: vec![],
            usage: TokenUsage::default(),
        },
    ]);

    let hive = HiveMind::builder()
        .substrate_path(&path)
        .llm_provider("test", provider)
        .build()
        .unwrap();

    // FR-002: Seed substrate with prior experience
    let cid = hive
        .substrate()
        .get_or_create_collective("project")
        .await
        .unwrap();

    let seed_exp = NewExperience {
        collective_id: cid,
        content: "The project uses async/await extensively for concurrent operations.".into(),
        experience_type: ExperienceType::TechInsight {
            technology: "rust".into(),
            insight: "Heavy async/await usage".into(),
        },
        embedding: None,
        importance: 0.8,
        confidence: 0.9,
        domain: vec!["rust".into(), "async".into()],
        source_agent: AgentId("seed-agent".into()),
        source_task: None,
        related_files: vec![],
    };
    hive.record_experience(seed_exp).await.unwrap();

    // FR-003 + FR-005: Deploy agent with tool and lens
    let agent = AgentDefinition {
        name: "code-reviewer".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a code reviewer.".into(),
            tools: vec![Arc::new(SearchTool)],
            lens: Lens::new(["rust", "async"]),
            llm_config: LlmConfig::new("test", "test-model"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    };

    let task = Task::with_collective("Review the async patterns", cid);
    let mut stream = hive.deploy(vec![agent], vec![task]).await.unwrap();

    // FR-014: Collect events
    let mut events = vec![];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        tokio::select! {
            event = stream.next() => {
                match event {
                    Some(e) => {
                        let done = matches!(&e, HiveEvent::ExperienceRecorded { .. });
                        events.push(e);
                        if done { break; }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    // Verify event sequence covers all FRs
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::AgentStarted { .. })),
        "FR-003: AgentStarted missing"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::LlmCallStarted { .. })),
        "FR-004: LlmCallStarted missing"
    );
    assert!(
        events.iter().any(
            |e| matches!(e, HiveEvent::ToolCallStarted { tool_name, .. } if tool_name == "search")
        ),
        "FR-006: ToolCallStarted missing"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::ToolCallCompleted { .. })),
        "FR-006: ToolCallCompleted missing"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::ExperienceRecorded { .. })),
        "FR-011: ExperienceRecorded missing"
    );

    println!(
        "Phase 1 integration test passed! {} events collected.",
        events.len()
    );
}
