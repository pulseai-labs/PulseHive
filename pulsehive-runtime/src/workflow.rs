//! Workflow execution engine — Sequential, Parallel, Loop agent orchestration.
//!
//! This module provides [`dispatch_agent()`], the central routing function that handles
//! all [`AgentKind`] variants. LLM agents are dispatched to the agentic loop; workflow
//! agents will be dispatched to their respective executors in subsequent tickets.
//!
//! ## Architecture
//!
//! [`WorkflowContext`] carries all shared resources as owned/Arc types (no lifetimes).
//! This is critical for the Parallel executor which needs to `tokio::spawn` child tasks
//! (requiring `'static`). When dispatching to the agentic loop, a temporary
//! [`LoopContext`] is created with borrows scoped to that call.

use std::collections::HashMap;
use std::sync::Arc;

use pulsedb::SubstrateProvider;
use tracing::Instrument;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentKindTag, AgentOutcome, LlmAgentConfig};
use pulsehive_core::approval::ApprovalHandler;
use pulsehive_core::event::{EventBus, HiveEvent};
use pulsehive_core::llm::LlmProvider;

use crate::agentic_loop::{self, LoopContext, DEFAULT_MAX_ITERATIONS};
use crate::hivemind::Task;

/// Owned context for workflow execution.
///
/// All fields are owned or `Arc`-wrapped, making this `Clone`-able for cheap sharing
/// across parallel child tasks. Cloning bumps reference counts — no deep copies.
#[derive(Clone)]
pub(crate) struct WorkflowContext {
    /// The task being executed (description + collective ID).
    pub task: Task,
    /// Named LLM providers registered with HiveMind.
    pub llm_providers: HashMap<String, Arc<dyn LlmProvider>>,
    /// Shared substrate for experience storage and retrieval.
    pub substrate: Arc<dyn SubstrateProvider>,
    /// Handler for tool approval requests.
    pub approval_handler: Arc<dyn ApprovalHandler>,
    /// Event broadcaster for lifecycle and observability events.
    pub event_emitter: EventBus,
}

/// Dispatch an agent to the appropriate executor based on its kind.
///
/// This is the central routing function for all agent types:
/// - `Llm` → agentic loop (Perceive→Think→Act→Record)
/// - `Sequential` / `Parallel` / `Loop` → workflow executors
///
/// Each call emits `AgentStarted` and `AgentCompleted` events, enabling
/// observability at every nesting level.
///
/// Uses `Box::pin` internally for recursive dispatch (Sequential/Parallel/Loop
/// call back into `dispatch_agent`, creating recursive futures that need heap allocation).
pub(crate) fn dispatch_agent(
    agent: AgentDefinition,
    ctx: &WorkflowContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = AgentOutcome> + Send + '_>> {
    let agent_name = agent.name.clone();
    let kind_tag = agent_kind_tag(&agent.kind);
    let span = tracing::info_span!("dispatch_agent", agent_name = %agent_name, kind = ?kind_tag);
    Box::pin(async move {
    let agent_id = uuid::Uuid::now_v7().to_string();

    // Emit lifecycle start event
    ctx.event_emitter.emit(HiveEvent::AgentStarted {
        agent_id: agent_id.clone(),
        name: agent.name.clone(),
        kind: agent_kind_tag(&agent.kind),
    });

    let outcome = match agent.kind {
        AgentKind::Llm(config) => run_llm_agent(&agent_id, *config, ctx).await,
        AgentKind::Sequential(children) => run_sequential(children, ctx).await,
        AgentKind::Parallel(children) => run_parallel(children, ctx).await,
        AgentKind::Loop { agent, max_iterations } => {
            run_loop(*agent, max_iterations, ctx).await
        }
    };

    // Emit lifecycle completion event
    ctx.event_emitter.emit(HiveEvent::AgentCompleted {
        agent_id,
        outcome: outcome.clone(),
    });

    outcome
    }.instrument(span)) // Box::pin
}

/// Execute child agents sequentially — each starts after the previous completes.
///
/// Children share the substrate and collective, so each child's Perceive phase
/// naturally finds experiences recorded by all previous children. This is the
/// "shared consciousness" model — no explicit data passing between agents.
///
/// Returns the last child's outcome. Stops early on error or `MaxIterationsReached`.
/// Empty children list returns `Complete` with empty response.
async fn run_sequential(
    children: Vec<AgentDefinition>,
    ctx: &WorkflowContext,
) -> AgentOutcome {
    tracing::info!(child_count = children.len(), "Sequential workflow started");

    if children.is_empty() {
        return AgentOutcome::Complete {
            response: String::new(),
        };
    }

    let mut last_response = String::new();
    for (i, child) in children.into_iter().enumerate() {
        tracing::info!(child_index = i, child_name = %child.name, "Sequential: running child");
        let outcome = dispatch_agent(child, ctx).await;
        match &outcome {
            AgentOutcome::Complete { response } => {
                last_response = response.clone();
            }
            AgentOutcome::Error { .. } | AgentOutcome::MaxIterationsReached => {
                return outcome;
            }
        }
    }
    AgentOutcome::Complete {
        response: last_response,
    }
}

/// Execute child agents in parallel — all spawned as concurrent Tokio tasks.
///
/// Children share the substrate (via `Arc`) and can perceive each other's
/// experiences as they're written. Each child gets a cloned `WorkflowContext`
/// (cheap: just Arc reference count bumps).
///
/// Returns combined responses on success. If any child errors, reports all
/// errors but still waits for all children to complete (no early cancellation).
async fn run_parallel(
    children: Vec<AgentDefinition>,
    ctx: &WorkflowContext,
) -> AgentOutcome {
    tracing::info!(child_count = children.len(), "Parallel workflow started");

    if children.is_empty() {
        return AgentOutcome::Complete {
            response: String::new(),
        };
    }

    let child_count = children.len();
    tracing::info!(child_count, "Parallel: spawning children");

    let mut join_set = tokio::task::JoinSet::new();
    for child in children {
        let child_ctx = ctx.clone();
        join_set.spawn(async move {
            dispatch_agent(child, &child_ctx).await
        });
    }

    let mut responses = Vec::new();
    let mut errors = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(AgentOutcome::Complete { response }) => {
                responses.push(response);
            }
            Ok(outcome) => {
                errors.push(format!("{outcome:?}"));
            }
            Err(join_err) => {
                errors.push(format!("Task panic: {join_err}"));
            }
        }
    }

    if !errors.is_empty() {
        AgentOutcome::Error {
            error: errors.join("; "),
        }
    } else {
        AgentOutcome::Complete {
            response: responses.join("\n"),
        }
    }
}

/// Completion signal: if a child agent's response contains this string,
/// the loop terminates early. This is a convention — the LLM is instructed
/// to include `[LOOP_DONE]` when it's satisfied with the result.
const LOOP_DONE_SIGNAL: &str = "[LOOP_DONE]";

/// Execute a child agent repeatedly up to `max_iterations` times.
///
/// Each iteration clones the child definition (cheap: Arc bumps for tools)
/// and dispatches it. The loop terminates early if:
/// - The child's response contains `[LOOP_DONE]`
/// - The child returns an error
///
/// Each iteration perceives cumulative experiences from all prior iterations
/// via the shared substrate.
async fn run_loop(
    child: AgentDefinition,
    max_iterations: usize,
    ctx: &WorkflowContext,
) -> AgentOutcome {
    tracing::info!(max_iterations, "Loop workflow started");

    if max_iterations == 0 {
        tracing::warn!("Loop with max_iterations=0, returning immediately");
        return AgentOutcome::Complete {
            response: String::new(),
        };
    }

    let mut last_outcome = AgentOutcome::MaxIterationsReached;
    for i in 0..max_iterations {
        tracing::info!(iteration = i + 1, max = max_iterations, "Loop: starting iteration");
        let outcome = dispatch_agent(child.clone(), ctx).await;

        match &outcome {
            AgentOutcome::Complete { response } if response.contains(LOOP_DONE_SIGNAL) => {
                tracing::info!(iteration = i + 1, "Loop: completion signal received");
                last_outcome = outcome;
                break;
            }
            AgentOutcome::Error { .. } => {
                tracing::warn!(iteration = i + 1, "Loop: child errored, stopping");
                return outcome;
            }
            _ => {
                last_outcome = outcome;
            }
        }
    }
    last_outcome
}

/// Execute an LLM agent through the agentic loop.
///
/// Resolves the named LLM provider from the context, creates a scoped
/// [`LoopContext`] with borrowed fields, and delegates to `run_agentic_loop`.
async fn run_llm_agent(
    agent_id: &str,
    config: LlmAgentConfig,
    ctx: &WorkflowContext,
) -> AgentOutcome {
    // Resolve the LLM provider by name
    let provider_name = &config.llm_config.provider;
    let provider = match ctx.llm_providers.get(provider_name) {
        Some(p) => p.clone(),
        None => {
            return AgentOutcome::Error {
                error: format!(
                    "LLM provider '{}' not registered. Available: {:?}",
                    provider_name,
                    ctx.llm_providers.keys().collect::<Vec<_>>()
                ),
            };
        }
    };

    // Create a scoped LoopContext — borrows from WorkflowContext are local to this call
    agentic_loop::run_agentic_loop(
        config,
        LoopContext {
            agent_id: agent_id.to_string(),
            task: &ctx.task,
            provider,
            substrate: Arc::clone(&ctx.substrate),
            approval_handler: ctx.approval_handler.as_ref(),
            event_emitter: ctx.event_emitter.clone(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
    )
    .await
}

/// Extract a compact kind tag from an agent kind (for event reporting).
fn agent_kind_tag(kind: &AgentKind) -> AgentKindTag {
    match kind {
        AgentKind::Llm(_) => AgentKindTag::Llm,
        AgentKind::Sequential(_) => AgentKindTag::Sequential,
        AgentKind::Parallel(_) => AgentKindTag::Parallel,
        AgentKind::Loop { .. } => AgentKindTag::Loop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use pulsehive_core::lens::Lens;
    use pulsehive_core::llm::*;

    // ── Mock LLM ─────────────────────────────────────────────────────

    struct MockLlm {
        responses: Mutex<Vec<LlmResponse>>,
    }

    impl MockLlm {
        fn new(responses: Vec<LlmResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }

        fn text_response(content: &str) -> LlmResponse {
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
        ) -> pulsehive_core::error::Result<LlmResponse> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Err(pulsehive_core::error::PulseHiveError::llm(
                    "No more scripted responses",
                ))
            } else {
                Ok(responses.remove(0))
            }
        }

        async fn chat_stream(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _config: &LlmConfig,
        ) -> pulsehive_core::error::Result<
            std::pin::Pin<Box<dyn futures_core::Stream<Item = pulsehive_core::error::Result<LlmChunk>> + Send>>,
        > {
            Err(pulsehive_core::error::PulseHiveError::llm(
                "Streaming not used in tests",
            ))
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn test_substrate() -> Arc<dyn SubstrateProvider> {
        let dir = tempfile::tempdir().unwrap();
        let db = pulsedb::PulseDB::open(
            dir.path().join("test.db"),
            pulsedb::Config::default(),
        )
        .unwrap();
        Box::leak(Box::new(dir));
        Arc::new(pulsedb::PulseDBSubstrate::from_db(db))
    }

    async fn test_workflow_ctx(provider: MockLlm) -> WorkflowContext {
        let substrate = test_substrate();
        let collective_id = substrate
            .get_or_create_collective("test-workflow")
            .await
            .unwrap();

        let mut providers: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
        providers.insert("mock".into(), Arc::new(provider));

        WorkflowContext {
            task: Task::with_collective("Test task", collective_id),
            llm_providers: providers,
            substrate,
            approval_handler: Arc::new(pulsehive_core::approval::AutoApprove),
            event_emitter: EventBus::default(),
        }
    }

    fn llm_agent_def(name: &str) -> AgentDefinition {
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

    // ── Tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_dispatch_llm_agent_completes() {
        let provider = MockLlm::new(vec![MockLlm::text_response("Hello from dispatch!")]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = llm_agent_def("test-agent");
        let outcome = dispatch_agent(agent, &ctx).await;

        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "Hello from dispatch!"),
            "Expected Complete, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_dispatch_llm_agent_emits_events() {
        let provider = MockLlm::new(vec![MockLlm::text_response("Done")]);
        let ctx = test_workflow_ctx(provider).await;
        let mut rx = ctx.event_emitter.subscribe();

        let agent = llm_agent_def("evented-agent");
        let _outcome = dispatch_agent(agent, &ctx).await;

        // Collect all events
        let mut events = vec![];
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // First event should be AgentStarted
        assert!(
            matches!(&events[0], HiveEvent::AgentStarted { name, kind, .. }
                if name == "evented-agent" && *kind == AgentKindTag::Llm),
            "Expected AgentStarted, got: {:?}",
            events.first()
        );

        // Last event should be AgentCompleted
        assert!(
            matches!(events.last(), Some(HiveEvent::AgentCompleted { outcome: AgentOutcome::Complete { .. }, .. })),
            "Expected AgentCompleted, got: {:?}",
            events.last()
        );
    }

    #[tokio::test]
    async fn test_dispatch_missing_provider_returns_error() {
        let provider = MockLlm::new(vec![]);
        let ctx = test_workflow_ctx(provider).await;

        // Agent uses a provider name that doesn't exist
        let agent = AgentDefinition {
            name: "bad-provider".into(),
            kind: AgentKind::Llm(Box::new(LlmAgentConfig {
                system_prompt: "test".into(),
                tools: vec![],
                lens: Lens::default(),
                llm_config: LlmConfig::new("nonexistent", "model"),
                experience_extractor: None,
                refresh_every_n_tool_calls: None,
            })),
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Error { error } if error.contains("nonexistent")),
            "Expected provider error, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_sequential_empty_children() {
        let provider = MockLlm::new(vec![]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "seq".into(),
            kind: AgentKind::Sequential(vec![]),
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response.is_empty()),
            "Empty Sequential should return Complete with empty response, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_sequential_two_children_in_order() {
        let provider = MockLlm::new(vec![
            MockLlm::text_response("First done"),
            MockLlm::text_response("Second done"),
        ]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "pipeline".into(),
            kind: AgentKind::Sequential(vec![
                llm_agent_def("step-1"),
                llm_agent_def("step-2"),
            ]),
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "Second done"),
            "Sequential should return last child's response, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_sequential_error_stops_execution() {
        // Only one response — first child gets it, second would fail if reached
        let provider = MockLlm::new(vec![]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "failing-seq".into(),
            kind: AgentKind::Sequential(vec![
                llm_agent_def("will-error"),
                llm_agent_def("should-not-run"),
            ]),
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        // First child errors (no LLM responses), second never runs
        assert!(
            matches!(&outcome, AgentOutcome::Error { .. }),
            "Sequential should stop on first error, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_workflow_context_is_clone() {
        let provider = MockLlm::new(vec![]);
        let ctx = test_workflow_ctx(provider).await;
        let _cloned = ctx.clone(); // Compile-time proof that Clone works
    }

    #[tokio::test]
    async fn test_parallel_empty_children() {
        let provider = MockLlm::new(vec![]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "par".into(),
            kind: AgentKind::Parallel(vec![]),
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response.is_empty()),
            "Empty Parallel should return Complete with empty response, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_parallel_two_children_both_complete() {
        let provider = MockLlm::new(vec![
            MockLlm::text_response("Alpha result"),
            MockLlm::text_response("Beta result"),
        ]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "par".into(),
            kind: AgentKind::Parallel(vec![
                llm_agent_def("alpha"),
                llm_agent_def("beta"),
            ]),
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        match &outcome {
            AgentOutcome::Complete { response } => {
                // Both responses should appear (order may vary due to concurrency)
                assert!(
                    response.contains("result"),
                    "Should contain child responses, got: {response}"
                );
            }
            other => panic!("Expected Complete, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_parallel_one_error_reports_all() {
        // Only one response — one child succeeds, other errors
        let provider = MockLlm::new(vec![
            MockLlm::text_response("I succeeded"),
        ]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "par-err".into(),
            kind: AgentKind::Parallel(vec![
                llm_agent_def("will-succeed"),
                llm_agent_def("will-error"),
            ]),
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Error { .. }),
            "Parallel with one error should return Error, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_loop_zero_iterations() {
        let provider = MockLlm::new(vec![]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "loop-0".into(),
            kind: AgentKind::Loop {
                agent: Box::new(llm_agent_def("child")),
                max_iterations: 0,
            },
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response.is_empty()),
            "Loop with 0 iterations should return Complete empty, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_loop_runs_n_times() {
        let provider = MockLlm::new(vec![
            MockLlm::text_response("Iteration 1"),
            MockLlm::text_response("Iteration 2"),
        ]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "loop-2".into(),
            kind: AgentKind::Loop {
                agent: Box::new(llm_agent_def("worker")),
                max_iterations: 2,
            },
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        // After 2 iterations without [LOOP_DONE], returns MaxIterationsReached
        // But since both completed successfully, last_outcome = last Complete
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "Iteration 2"),
            "Loop should return last iteration's response, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_loop_early_exit_on_done_signal() {
        let provider = MockLlm::new(vec![
            MockLlm::text_response("Still working..."),
            MockLlm::text_response("All done [LOOP_DONE]"),
            MockLlm::text_response("Should not reach this"),
        ]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "loop-done".into(),
            kind: AgentKind::Loop {
                agent: Box::new(llm_agent_def("worker")),
                max_iterations: 5,
            },
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response.contains("[LOOP_DONE]")),
            "Loop should exit on LOOP_DONE signal, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_loop_error_stops() {
        // One response, then error (no more responses)
        let provider = MockLlm::new(vec![
            MockLlm::text_response("First iteration ok"),
        ]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "loop-err".into(),
            kind: AgentKind::Loop {
                agent: Box::new(llm_agent_def("worker")),
                max_iterations: 5,
            },
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Error { .. }),
            "Loop should stop on error, got: {outcome:?}"
        );
    }

    // ── Ticket #46: Sequential event ordering ────────────────────────

    #[tokio::test]
    async fn test_sequential_events_ordered() {
        let provider = MockLlm::new(vec![
            MockLlm::text_response("A done"),
            MockLlm::text_response("B done"),
        ]);
        let ctx = test_workflow_ctx(provider).await;
        let mut rx = ctx.event_emitter.subscribe();

        let agent = AgentDefinition {
            name: "seq-events".into(),
            kind: AgentKind::Sequential(vec![
                llm_agent_def("child-a"),
                llm_agent_def("child-b"),
            ]),
        };

        let _outcome = dispatch_agent(agent, &ctx).await;

        // Collect events
        let mut events = vec![];
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Find AgentStarted events for the LLM children (not the Sequential wrapper)
        let started_names: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                HiveEvent::AgentStarted { name, kind: AgentKindTag::Llm, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();

        // child-a should start before child-b (sequential ordering)
        assert_eq!(
            started_names,
            vec!["child-a", "child-b"],
            "Sequential children should start in order"
        );
    }

    // ── Ticket #47: Parallel event verification ──────────────────────

    #[tokio::test]
    async fn test_parallel_events_for_all_children() {
        let provider = MockLlm::new(vec![
            MockLlm::text_response("Alpha"),
            MockLlm::text_response("Beta"),
        ]);
        let ctx = test_workflow_ctx(provider).await;
        let mut rx = ctx.event_emitter.subscribe();

        let agent = AgentDefinition {
            name: "par-events".into(),
            kind: AgentKind::Parallel(vec![
                llm_agent_def("alpha"),
                llm_agent_def("beta"),
            ]),
        };

        let _outcome = dispatch_agent(agent, &ctx).await;

        let mut events = vec![];
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Both children should have AgentStarted events
        let started_names: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                HiveEvent::AgentStarted { name, kind: AgentKindTag::Llm, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();

        assert!(started_names.contains(&"alpha"), "alpha should have AgentStarted");
        assert!(started_names.contains(&"beta"), "beta should have AgentStarted");

        // Both should have AgentCompleted events
        let completed_count = events
            .iter()
            .filter(|e| matches!(e, HiveEvent::AgentCompleted { outcome: AgentOutcome::Complete { .. }, .. }))
            .count();
        assert!(completed_count >= 2, "Both children should complete, got {completed_count}");
    }

    // ── Ticket #48: Loop additional tests ────────────────────────────

    #[tokio::test]
    async fn test_loop_single_iteration() {
        let provider = MockLlm::new(vec![
            MockLlm::text_response("Only once"),
        ]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "loop-1".into(),
            kind: AgentKind::Loop {
                agent: Box::new(llm_agent_def("worker")),
                max_iterations: 1,
            },
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "Only once"),
            "Loop max=1 should run exactly once, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_loop_all_iterations_complete_returns_last() {
        let provider = MockLlm::new(vec![
            MockLlm::text_response("Iter 1"),
            MockLlm::text_response("Iter 2"),
            MockLlm::text_response("Iter 3"),
        ]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "loop-3".into(),
            kind: AgentKind::Loop {
                agent: Box::new(llm_agent_def("worker")),
                max_iterations: 3,
            },
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        // All 3 completed without [LOOP_DONE], returns last Complete
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "Iter 3"),
            "Loop should return last iteration's response, got: {outcome:?}"
        );
    }

    // ── Ticket #50: Edge cases ───────────────────────────────────────

    #[tokio::test]
    async fn test_single_child_sequential() {
        let provider = MockLlm::new(vec![MockLlm::text_response("Solo")]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "single-seq".into(),
            kind: AgentKind::Sequential(vec![llm_agent_def("only-child")]),
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "Solo"),
            "Single-child Sequential should work like running the child directly, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_single_child_parallel() {
        let provider = MockLlm::new(vec![MockLlm::text_response("Solo parallel")]);
        let ctx = test_workflow_ctx(provider).await;

        let agent = AgentDefinition {
            name: "single-par".into(),
            kind: AgentKind::Parallel(vec![llm_agent_def("only-child")]),
        };

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "Solo parallel"),
            "Single-child Parallel should work, got: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn test_deep_nesting_no_stack_overflow() {
        // 5 levels deep: Sequential(Sequential(Sequential(Sequential(Sequential(Llm)))))
        let provider = MockLlm::new(vec![MockLlm::text_response("Deep!")]);
        let ctx = test_workflow_ctx(provider).await;

        let mut agent = llm_agent_def("leaf");
        for i in 0..5 {
            agent = AgentDefinition {
                name: format!("level-{i}"),
                kind: AgentKind::Sequential(vec![agent]),
            };
        }

        let outcome = dispatch_agent(agent, &ctx).await;
        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "Deep!"),
            "5-level nesting should work without stack overflow, got: {outcome:?}"
        );
    }
}
