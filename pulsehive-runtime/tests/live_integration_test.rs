//! Live integration tests for the full PulseHive agentic pipeline.
//!
//! These tests deploy real agents with a real LLM (GLM via OpenAI-compatible API).
//! Run with: `cargo test -p pulsehive-runtime --test live_integration_test -- --ignored`
//!
//! Requires `.env` file at repo root with PULSEHIVE_API_KEY, PULSEHIVE_BASE_URL, PULSEHIVE_MODEL.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use pulsehive_core::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::LlmConfig;
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};
use pulsehive_openai::{OpenAICompatibleProvider, OpenAIConfig};
use pulsehive_runtime::hivemind::{HiveMind, Task};

fn setup_provider() -> OpenAICompatibleProvider {
    dotenvy::from_filename("../.env")
        .or_else(|_| dotenvy::dotenv())
        .ok();
    let api_key = std::env::var("PULSEHIVE_API_KEY").expect("Set PULSEHIVE_API_KEY in .env");
    let base_url = std::env::var("PULSEHIVE_BASE_URL").expect("Set PULSEHIVE_BASE_URL in .env");
    let model = std::env::var("PULSEHIVE_MODEL").expect("Set PULSEHIVE_MODEL in .env");

    OpenAICompatibleProvider::new(OpenAIConfig::new(&api_key, &model).with_base_url(&base_url))
}

fn model_name() -> String {
    std::env::var("PULSEHIVE_MODEL").unwrap_or_else(|_| "GLM-4.7".into())
}

/// A simple tool that returns a fixed response.
struct GetTimeTool;

#[async_trait]
impl Tool for GetTimeTool {
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
        _params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> pulsehive_core::error::Result<ToolResult> {
        Ok(ToolResult::text("2026-03-26T10:00:00Z"))
    }
}

#[tokio::test]
#[ignore]
async fn live_single_agent_with_tool() {
    let provider = setup_provider();
    let model = model_name();
    let dir = tempfile::tempdir().unwrap();

    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("live_test.db"))
        .llm_provider("openai", provider)
        .no_relationship_detector()
        .no_insight_synthesizer()
        .build()
        .unwrap();

    let agent = AgentDefinition {
        name: "time-agent".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You have a get_time tool. When asked for the time, use it. Then report the time to the user.".into(),
            tools: vec![Arc::new(GetTimeTool)],
            lens: Lens::new(["general"]),
            llm_config: LlmConfig::new("openai", &model),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    };

    let mut stream = hive
        .deploy(vec![agent], vec![Task::new("What time is it?")])
        .await
        .unwrap();

    let mut events = vec![];
    while let Some(event) = stream.next().await {
        println!("  {:?}", event);
        events.push(event);
        if matches!(events.last(), Some(HiveEvent::AgentCompleted { .. })) {
            break;
        }
    }

    hive.shutdown();

    // Verify we got the expected event flow
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::AgentStarted { .. })),
        "Should have AgentStarted event"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiveEvent::LlmCallStarted { .. })),
        "Should have LlmCallStarted event"
    );

    // Check final outcome
    let completed = events
        .iter()
        .find(|e| matches!(e, HiveEvent::AgentCompleted { .. }));
    assert!(completed.is_some(), "Should have AgentCompleted event");
    match completed.unwrap() {
        HiveEvent::AgentCompleted { outcome, .. } => {
            println!("Agent outcome: {:?}", outcome);
            assert!(
                matches!(
                    outcome,
                    pulsehive_core::agent::AgentOutcome::Complete { .. }
                ),
                "Agent should complete successfully, got: {:?}",
                outcome
            );
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
#[ignore]
async fn live_sequential_two_agents() {
    let provider = setup_provider();
    let model = model_name();
    let dir = tempfile::tempdir().unwrap();

    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("live_seq.db"))
        .llm_provider("openai", provider)
        .no_relationship_detector()
        .no_insight_synthesizer()
        .build()
        .unwrap();

    let config = LlmConfig::new("openai", &model);

    let pipeline = AgentDefinition {
        name: "pipeline".into(),
        kind: AgentKind::Sequential(vec![
            AgentDefinition {
                name: "researcher".into(),
                kind: AgentKind::Llm(Box::new(LlmAgentConfig {
                    system_prompt: "You are a research agent. Provide 3 key facts about Rust programming language. Be concise.".into(),
                    tools: vec![],
                    lens: Lens::new(["research"]),
                    llm_config: config.clone(),
                    experience_extractor: None,
                    refresh_every_n_tool_calls: None,
                })),
            },
            AgentDefinition {
                name: "summarizer".into(),
                kind: AgentKind::Llm(Box::new(LlmAgentConfig {
                    system_prompt: "You are a summarizer. Summarize the research findings available in your context into one sentence.".into(),
                    tools: vec![],
                    lens: Lens::new(["research", "summary"]),
                    llm_config: config,
                    experience_extractor: None,
                    refresh_every_n_tool_calls: None,
                })),
            },
        ]),
    };

    let mut stream = hive
        .deploy(vec![pipeline], vec![Task::new("Research Rust programming")])
        .await
        .unwrap();

    let mut agent_starts = 0;
    let mut agent_completes = 0;
    while let Some(event) = stream.next().await {
        match &event {
            HiveEvent::AgentStarted { name, .. } => {
                agent_starts += 1;
                println!("  Started: {name}");
            }
            HiveEvent::AgentCompleted { outcome, .. } => {
                agent_completes += 1;
                println!("  Completed: {:?}", outcome);
            }
            _ => {}
        }
        // Break after pipeline (parent) completes — it's the last AgentCompleted
        if agent_completes >= 3 {
            // 3 = researcher + summarizer + pipeline
            break;
        }
    }

    hive.shutdown();

    // Should have 3 starts (pipeline + researcher + summarizer)
    assert!(
        agent_starts >= 3,
        "Expected 3+ agent starts, got {agent_starts}"
    );
    assert!(
        agent_completes >= 3,
        "Expected 3+ agent completions, got {agent_completes}"
    );
}
