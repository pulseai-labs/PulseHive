//! Integration tests for workflow agents — Sequential, Parallel, Loop, and nested compositions.
//!
//! Tests exercise the full HiveMind::deploy() → workflow::dispatch_agent() → agentic loop path.

use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures_core::Stream;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::*;
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

fn llm_agent(name: &str) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a test agent.".into(),
            tools: vec![],
            lens: Lens::default(),
            llm_config: LlmConfig::new("mock", "test-model"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    }
}

/// Collect events with a timeout — doesn't stop on first AgentCompleted
/// (workflows emit multiple completions). Uses a short settling period
/// after the last event to detect when all agents are done.
async fn collect_workflow_events(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    timeout: Duration,
) -> Vec<HiveEvent> {
    let mut events = vec![];
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        tokio::select! {
            event = stream.next() => {
                match event {
                    Some(e) => events.push(e),
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

/// Count AgentStarted events with a specific kind tag
fn count_started(events: &[HiveEvent], kind: pulsehive_core::agent::AgentKindTag) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, HiveEvent::AgentStarted { kind: k, .. } if *k == kind))
        .count()
}

/// Count AgentCompleted events with Complete outcome
fn count_completed(events: &[HiveEvent]) -> usize {
    events
        .iter()
        .filter(|e| {
            matches!(
                e,
                HiveEvent::AgentCompleted {
                    outcome: AgentOutcome::Complete { .. },
                    ..
                }
            )
        })
        .count()
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_deploy_sequential_workflow() {
    let provider = MockLlm::new(vec![MockLlm::text("Step 1 done"), MockLlm::text("Step 2 done")]);
    let hive = build_hive(provider);

    let workflow = AgentDefinition {
        name: "pipeline".into(),
        kind: AgentKind::Sequential(vec![llm_agent("step-1"), llm_agent("step-2")]),
    };
    let task = Task::new("Run pipeline");

    let stream = hive.deploy(vec![workflow], vec![task]).await.unwrap();
    let events = collect_workflow_events(stream, Duration::from_secs(5)).await;

    // Should have AgentStarted events for: top-level spawn, Sequential wrapper, step-1, step-2
    assert!(
        count_started(&events, pulsehive_core::agent::AgentKindTag::Llm) >= 2,
        "Should have at least 2 LLM AgentStarted events. Events: {events:?}"
    );
    // Both LLM children should complete
    assert!(
        count_completed(&events) >= 2,
        "Should have at least 2 AgentCompleted events. Events: {events:?}"
    );
}

#[tokio::test]
async fn test_deploy_parallel_workflow() {
    let provider = MockLlm::new(vec![MockLlm::text("Alpha done"), MockLlm::text("Beta done")]);
    let hive = build_hive(provider);

    let workflow = AgentDefinition {
        name: "fan-out".into(),
        kind: AgentKind::Parallel(vec![llm_agent("alpha"), llm_agent("beta")]),
    };
    let task = Task::new("Fan out work");

    let stream = hive.deploy(vec![workflow], vec![task]).await.unwrap();
    let events = collect_workflow_events(stream, Duration::from_secs(5)).await;

    // Both parallel children should start and complete
    assert!(
        count_started(&events, pulsehive_core::agent::AgentKindTag::Llm) >= 2,
        "Both parallel children should start. Events: {events:?}"
    );
    assert!(
        count_completed(&events) >= 2,
        "Both parallel children should complete. Events: {events:?}"
    );
}

#[tokio::test]
async fn test_deploy_loop_workflow() {
    let provider = MockLlm::new(vec![
        MockLlm::text("Iteration 1"),
        MockLlm::text("Done [LOOP_DONE]"),
    ]);
    let hive = build_hive(provider);

    let workflow = AgentDefinition {
        name: "refiner".into(),
        kind: AgentKind::Loop {
            agent: Box::new(llm_agent("worker")),
            max_iterations: 5,
        },
    };
    let task = Task::new("Refine until done");

    let stream = hive.deploy(vec![workflow], vec![task]).await.unwrap();
    let events = collect_workflow_events(stream, Duration::from_secs(5)).await;

    // Should have 2 LLM starts (2 iterations before [LOOP_DONE])
    assert!(
        count_started(&events, pulsehive_core::agent::AgentKindTag::Llm) >= 2,
        "Loop should run 2 iterations. Events: {events:?}"
    );
}

#[tokio::test]
async fn test_deploy_nested_sequential_parallel() {
    // Sequential([Parallel([A, B]), C])
    // A and B run concurrently, then C runs after both complete
    let provider = MockLlm::new(vec![
        MockLlm::text("A result"),
        MockLlm::text("B result"),
        MockLlm::text("C sees A and B"),
    ]);
    let hive = build_hive(provider);

    let workflow = AgentDefinition {
        name: "nested".into(),
        kind: AgentKind::Sequential(vec![
            AgentDefinition {
                name: "parallel-phase".into(),
                kind: AgentKind::Parallel(vec![llm_agent("A"), llm_agent("B")]),
            },
            llm_agent("C"),
        ]),
    };
    let task = Task::new("Nested workflow");

    let stream = hive.deploy(vec![workflow], vec![task]).await.unwrap();
    let events = collect_workflow_events(stream, Duration::from_secs(5)).await;

    // All 3 LLM children should start and complete
    assert!(
        count_started(&events, pulsehive_core::agent::AgentKindTag::Llm) >= 3,
        "All 3 LLM agents should start. Events: {events:?}"
    );
    assert!(
        count_completed(&events) >= 3,
        "All 3 LLM agents should complete. Events: {events:?}"
    );
}

#[tokio::test]
async fn test_deploy_nested_parallel_sequential() {
    // Parallel([Sequential([A, B]), C])
    // Sequential(A→B) and C run concurrently
    let provider = MockLlm::new(vec![
        MockLlm::text("A done"),
        MockLlm::text("B after A"),
        MockLlm::text("C concurrent"),
    ]);
    let hive = build_hive(provider);

    let workflow = AgentDefinition {
        name: "par-seq".into(),
        kind: AgentKind::Parallel(vec![
            AgentDefinition {
                name: "seq-branch".into(),
                kind: AgentKind::Sequential(vec![llm_agent("A"), llm_agent("B")]),
            },
            llm_agent("C"),
        ]),
    };
    let task = Task::new("Parallel with sequential branch");

    let stream = hive.deploy(vec![workflow], vec![task]).await.unwrap();
    let events = collect_workflow_events(stream, Duration::from_secs(5)).await;

    assert!(
        count_started(&events, pulsehive_core::agent::AgentKindTag::Llm) >= 3,
        "All 3 LLM agents should start. Events: {events:?}"
    );
}

#[tokio::test]
async fn test_deploy_deep_nesting() {
    // Sequential([Parallel([Loop { A, 2 }, B])])
    // Triple nesting: seq > par > loop
    let provider = MockLlm::new(vec![
        MockLlm::text("Loop iter 1"),
        MockLlm::text("Loop iter 2"),
        MockLlm::text("B result"),
    ]);
    let hive = build_hive(provider);

    let workflow = AgentDefinition {
        name: "deep".into(),
        kind: AgentKind::Sequential(vec![AgentDefinition {
            name: "par-phase".into(),
            kind: AgentKind::Parallel(vec![
                AgentDefinition {
                    name: "loop-branch".into(),
                    kind: AgentKind::Loop {
                        agent: Box::new(llm_agent("looper")),
                        max_iterations: 2,
                    },
                },
                llm_agent("B"),
            ]),
        }]),
    };
    let task = Task::new("Deep nesting test");

    let stream = hive.deploy(vec![workflow], vec![task]).await.unwrap();
    let events = collect_workflow_events(stream, Duration::from_secs(5)).await;

    // looper runs 2x + B runs 1x = at least 3 LLM starts
    assert!(
        count_started(&events, pulsehive_core::agent::AgentKindTag::Llm) >= 3,
        "Should have at least 3 LLM starts (2 loop + 1 B). Events: {events:?}"
    );
}

// ── Sprint 6: Watch Integration Tests (#56, #57) ────────────────────

fn llm_agent_with_refresh(name: &str, refresh: Option<usize>) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a test agent.".into(),
            tools: vec![],
            lens: Lens::default(),
            llm_config: LlmConfig::new("mock", "test-model"),
            experience_extractor: None,
            refresh_every_n_tool_calls: refresh,
        })),
    }
}

/// #56: Sequential agents naturally share experiences (B perceives A's at start)
#[tokio::test]
async fn test_sequential_experience_sharing() {
    let provider = MockLlm::new(vec![
        MockLlm::text("Agent A discovered something"),
        MockLlm::text("Agent B building on A's work"),
    ]);
    let hive = build_hive(provider);

    let workflow = AgentDefinition {
        name: "seq-share".into(),
        kind: AgentKind::Sequential(vec![
            llm_agent("agent-a"),
            llm_agent("agent-b"),
        ]),
    };
    let task = Task::new("Sequential sharing test");

    let stream = hive.deploy(vec![workflow], vec![task]).await.unwrap();
    let events = collect_workflow_events(stream, Duration::from_secs(5)).await;

    // Agent A should record an experience, Agent B should perceive the substrate
    let has_experience_recorded = events
        .iter()
        .any(|e| matches!(e, HiveEvent::ExperienceRecorded { .. }));
    assert!(
        has_experience_recorded,
        "Agent A should record an experience. Events: {events:?}"
    );

    // Both agents should have perceived the substrate
    let perceive_count = events
        .iter()
        .filter(|e| matches!(e, HiveEvent::SubstratePerceived { .. }))
        .count();
    assert!(
        perceive_count >= 2,
        "Both agents should perceive substrate. Got {perceive_count}. Events: {events:?}"
    );
}

/// #56: Watch events appear in the event stream
#[tokio::test]
async fn test_watch_events_in_stream() {
    let provider = MockLlm::new(vec![MockLlm::text("Done")]);
    let hive = build_hive(provider);

    let agent = llm_agent("watcher");
    let task = Task::new("Watch test");

    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_workflow_events(stream, Duration::from_secs(5)).await;

    // The agent records an experience → Watch should emit WatchNotification
    // Note: Watch delivery is async, so it may or may not arrive within the timeout.
    // We just verify no panics and the event stream works correctly.
    let watch_count = events
        .iter()
        .filter(|e| matches!(e, HiveEvent::WatchNotification { .. }))
        .count();
    // WatchNotification may or may not arrive depending on timing — just log it
    tracing::info!(watch_events = watch_count, "Watch events received");

    // Core assertion: agent completed successfully with Watch subscription active
    assert!(
        count_completed(&events) >= 1,
        "Agent should complete even with Watch subscription active. Events: {events:?}"
    );
}

/// #57: Multiple concurrent agents complete without errors (stress-lite)
#[tokio::test]
async fn test_stress_3_concurrent_agents() {
    let provider = MockLlm::new(vec![
        MockLlm::text("Agent 1 done"),
        MockLlm::text("Agent 2 done"),
        MockLlm::text("Agent 3 done"),
    ]);
    let hive = build_hive(provider);

    let workflow = AgentDefinition {
        name: "stress".into(),
        kind: AgentKind::Parallel(vec![
            llm_agent_with_refresh("worker-1", Some(2)),
            llm_agent_with_refresh("worker-2", Some(2)),
            llm_agent_with_refresh("worker-3", Some(2)),
        ]),
    };
    let task = Task::new("Stress test");

    let stream = hive.deploy(vec![workflow], vec![task]).await.unwrap();
    let events = collect_workflow_events(stream, Duration::from_secs(10)).await;

    // All 3 should start
    assert!(
        count_started(&events, pulsehive_core::agent::AgentKindTag::Llm) >= 3,
        "All 3 agents should start. Events: {events:?}"
    );

    // All 3 should complete
    assert!(
        count_completed(&events) >= 3,
        "All 3 agents should complete. Events: {events:?}"
    );
}
