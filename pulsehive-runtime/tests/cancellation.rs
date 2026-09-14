//! r1.s2.w4 — HiveMind root cancellation and the d15 end-to-end proof
//! (ADR-014).
//!
//! Every test drives the real path (`HiveMind::deploy`, `Task::with_cancel`,
//! `ScriptedProvider`), and every wait is bounded by `BOUND` so a regression
//! fails the test instead of hanging it. No fakes: the provider is the
//! product-owned `ScriptedProvider`, the tools are ordinary `Tool` impls.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive_core::error::Result;
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::{LlmConfig, LlmResponse, TokenUsage, ToolCall};
use pulsehive_core::testing::ScriptedProvider;
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};

/// Bound on every wait — a hung turn fails the test rather than hanging CI.
const BOUND: Duration = Duration::from_secs(30);

/// Builds a HiveMind on a temp substrate with the scripted provider registered
/// and insight synthesis off (a synthesizer would consume scripted steps).
fn scripted_hive(dir: &tempfile::TempDir, provider: ScriptedProvider) -> HiveMind {
    HiveMind::builder()
        .substrate_path(dir.path().join("cancel-hive.db"))
        .llm_provider("scripted", provider)
        .no_insight_synthesizer()
        .build()
        .expect("build HiveMind")
}

/// An LLM agent definition routing to the `scripted` provider.
fn scripted_agent(tools: Vec<Arc<dyn Tool>>, llm_config: LlmConfig) -> AgentDefinition {
    AgentDefinition {
        name: "cancel-agent".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "Work the task.".into(),
            tools,
            lens: Lens::default(),
            llm_config,
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    }
}

/// Drains the deploy stream until `AgentCompleted`, returning every event
/// seen. Bounded — a turn that never completes fails the test.
async fn drain_until_completed(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
) -> Vec<HiveEvent> {
    tokio::time::timeout(BOUND, async move {
        let mut seen = Vec::new();
        while let Some(event) = stream.next().await {
            let done = matches!(event, HiveEvent::AgentCompleted { .. });
            seen.push(event);
            if done {
                break;
            }
        }
        seen
    })
    .await
    .expect("drain timed out before AgentCompleted")
}

/// Like [`drain_until_completed`], but cancels `token` the first time an
/// `LlmCallStarted` event arrives — the in-flight-cancel probe.
async fn drain_cancelling_at_llm_start(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    token: CancellationToken,
) -> Vec<HiveEvent> {
    tokio::time::timeout(BOUND, async move {
        let mut seen = Vec::new();
        let mut fired = false;
        while let Some(event) = stream.next().await {
            if matches!(event, HiveEvent::LlmCallStarted { .. }) && !fired {
                token.cancel();
                fired = true;
            }
            let done = matches!(event, HiveEvent::AgentCompleted { .. });
            seen.push(event);
            if done {
                break;
            }
        }
        seen
    })
    .await
    .expect("drain timed out before AgentCompleted")
}

/// Drains `stream` until both `AgentCompleted` events arrive, cancelling
/// `token` on the first `ToolCallStarted` that belongs to the run started
/// for `cancel_task`. The deploy stream is hive-wide, so one drain sees
/// both siblings' completions; the agent correlation goes through
/// `AgentStarted { agent_id, task_description }`.
async fn drain_siblings_cancelling_tool(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    cancel_task: &str,
    token: CancellationToken,
) -> Vec<HiveEvent> {
    tokio::time::timeout(BOUND, async move {
        let mut seen = Vec::new();
        let mut cancel_agent: Option<String> = None;
        let mut completions = 0usize;
        while let Some(event) = stream.next().await {
            match &event {
                HiveEvent::AgentStarted {
                    agent_id,
                    task_description,
                    ..
                } if task_description == cancel_task => {
                    cancel_agent = Some(agent_id.clone());
                }
                HiveEvent::ToolCallStarted { agent_id, .. }
                    if cancel_agent.as_ref() == Some(agent_id) =>
                {
                    token.cancel();
                }
                HiveEvent::AgentCompleted { .. } => completions += 1,
                _ => {}
            }
            let done = completions == 2;
            seen.push(event);
            if done {
                break;
            }
        }
        seen
    })
    .await
    .expect("drain timed out before both AgentCompleted events")
}

/// The outcome of the first `AgentCompleted` in a drained event set.
fn completed_outcome(events: &[HiveEvent]) -> &AgentOutcome {
    events
        .iter()
        .find_map(|event| match event {
            HiveEvent::AgentCompleted { outcome, .. } => Some(outcome),
            _ => None,
        })
        .expect("no AgentCompleted in drained events")
}

/// A cooperative tool: works in steps and polls `context.cancel` between
/// them, returning the results it already produced — the shape the
/// `cancellable_tool` example demonstrates. `gate` blocks step 1 on the
/// run's cancellation when set, so a test can cancel while a tool is
/// in flight.
struct SweepTool {
    steps: usize,
    step_ms: u64,
}

#[async_trait]
impl Tool for SweepTool {
    fn name(&self) -> &str {
        "sweep"
    }

    fn description(&self) -> &str {
        "Scores parameters in steps; returns partial results on cancel"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, ctx: &ToolContext) -> Result<ToolResult> {
        for step in 1..=self.steps {
            if ctx.cancel.is_cancelled() {
                return Ok(ToolResult::text(format!(
                    "partial: {}/{} steps scored",
                    step - 1,
                    self.steps
                )));
            }
            tokio::time::sleep(Duration::from_millis(self.step_ms)).await;
        }
        Ok(ToolResult::text(format!("all {} steps scored", self.steps)))
    }
}

/// d15: cancelling a task's token aborts the agent's in-flight provider
/// call, starts no further tool call, and returns `Cancelled` carrying the
/// turn's latest assistant text.
#[tokio::test]
async fn cancel_aborts_in_flight_call_and_returns_partial() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Turn: assistant text + a cooperative tool call, then a follow-up LLM
    // call that pends until its cancel token fires.
    let provider = ScriptedProvider::new()
        .then_response(LlmResponse::new(
            Some("working draft".to_string()),
            vec![ToolCall {
                id: "call_1".into(),
                name: "sweep".into(),
                arguments: json!({}),
            }],
            TokenUsage::default(),
        ))
        .then_hang();
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![Arc::new(SweepTool {
            steps: 2,
            step_ms: 5,
        })],
        LlmConfig::new("scripted", "test-model"),
    );

    let token = CancellationToken::new();
    let task = Task::new("cancel mid-turn").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");

    // Cancel when the SECOND LlmCallStarted arrives — the in-flight call is
    // the hanging follow-up after the tool result went back.
    let (events, cancel_at) = tokio::time::timeout(BOUND, {
        let token = token.clone();
        let mut stream = stream;
        async move {
            let mut seen = Vec::new();
            let mut llm_starts = 0usize;
            let mut cancel_at = None;
            while let Some(event) = stream.next().await {
                if matches!(event, HiveEvent::LlmCallStarted { .. }) {
                    llm_starts += 1;
                    if llm_starts == 2 {
                        token.cancel();
                        cancel_at = Some(seen.len());
                    }
                }
                let done = matches!(event, HiveEvent::AgentCompleted { .. });
                seen.push(event);
                if done {
                    break;
                }
            }
            (
                seen,
                cancel_at.expect("second LlmCallStarted never arrived"),
            )
        }
    })
    .await
    .expect("drain timed out before AgentCompleted");

    match completed_outcome(&events) {
        AgentOutcome::Cancelled { partial_response } => assert_eq!(
            partial_response, "working draft",
            "Cancelled must carry the turn's latest assistant text"
        ),
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
    assert!(
        !events[cancel_at..]
            .iter()
            .any(|event| matches!(event, HiveEvent::ToolCallStarted { .. })),
        "ToolCallStarted emitted after the cancel point"
    );
}

/// A cancelled task leaves the HiveMind usable: a later task carrying its
/// own token completes normally on the same hive.
#[tokio::test]
async fn later_task_on_same_hivemind_completes() {
    let dir = tempfile::tempdir().expect("tempdir");
    // One shared queue: task 1's run hangs on the follow-up and is
    // cancelled; task 2's run takes the text reply.
    let provider = ScriptedProvider::new()
        .then_hang()
        .then_text("second turn done");
    let hive = scripted_hive(&dir, provider);
    let agent = || scripted_agent(vec![], LlmConfig::new("scripted", "test-model"));

    let token = CancellationToken::new();
    let stream = hive
        .deploy(
            vec![agent()],
            vec![Task::new("first task").with_cancel(token.clone())],
        )
        .await
        .expect("deploy first task");
    let first = drain_cancelling_at_llm_start(stream, token).await;
    assert!(
        matches!(completed_outcome(&first), AgentOutcome::Cancelled { .. }),
        "first task: expected Cancelled, got {:?}",
        completed_outcome(&first)
    );
    assert!(
        !hive.is_shutdown(),
        "task cancellation must not shut the hive down"
    );

    let stream = hive
        .deploy(
            vec![agent()],
            vec![Task::new("second task").with_cancel(CancellationToken::new())],
        )
        .await
        .expect("deploy second task");
    let second = drain_until_completed(stream).await;
    match completed_outcome(&second) {
        AgentOutcome::Complete { response } => assert_eq!(response, "second turn done"),
        other => panic!("second task: expected Complete, got {other:?}"),
    }
}

/// A gate tool: with `wait_on_cancel` it pends inside `execute` on the
/// run's own `ToolContext.cancel` (released by the task's cancellation);
/// otherwise it returns immediately. Each sibling's agent carries its own
/// instance, so only the cancelled task's run stalls.
struct GateTool {
    wait_on_cancel: bool,
}

#[async_trait]
impl Tool for GateTool {
    fn name(&self) -> &str {
        "gate"
    }

    fn description(&self) -> &str {
        "Passes through, or pends on the run's cancel token when wait_on_cancel is set"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, ctx: &ToolContext) -> Result<ToolResult> {
        if self.wait_on_cancel {
            ctx.cancel.cancelled().await;
            return Ok(ToolResult::text("gate released by cancel"));
        }
        Ok(ToolResult::text("gate passed"))
    }
}

/// Two tasks in flight together on the same hive, each deployed with its
/// own agent and its own `ScriptedProvider` under a distinct provider
/// name — no shared script queue, so neither run can consume the other's
/// steps. Cancelling task A's token ends only A's run; sibling B
/// completes normally.
#[tokio::test]
async fn sibling_task_is_not_cancelled() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Task A's script: a gate call whose tool pends on the run's cancel
    // token, then a hang so a late cancel still ends at a checkpoint.
    // Task B's script: the same gate (instant pass), then the text reply.
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("cancel-hive.db"))
        .llm_provider(
            "scripted-a",
            ScriptedProvider::new()
                .then_tool_call("gate", json!({}))
                .then_hang(),
        )
        .llm_provider(
            "scripted-b",
            ScriptedProvider::new()
                .then_tool_call("gate", json!({}))
                .then_text("sibling done"),
        )
        .no_insight_synthesizer()
        .build()
        .expect("build HiveMind");

    let token_a = CancellationToken::new();
    let agent_a = scripted_agent(
        vec![Arc::new(GateTool {
            wait_on_cancel: true,
        })],
        LlmConfig::new("scripted-a", "test-model"),
    );
    let agent_b = scripted_agent(
        vec![Arc::new(GateTool {
            wait_on_cancel: false,
        })],
        LlmConfig::new("scripted-b", "test-model"),
    );

    let stream = hive
        .deploy(
            vec![agent_a],
            vec![Task::new("sibling A").with_cancel(token_a.clone())],
        )
        .await
        .expect("deploy task A");
    let _sibling = hive
        .deploy(vec![agent_b], vec![Task::new("sibling B")])
        .await
        .expect("deploy task B");

    let events = drain_siblings_cancelling_tool(stream, "sibling A", token_a).await;

    let outcome_of = |description: &str| -> &AgentOutcome {
        events
            .iter()
            .find_map(|event| match event {
                HiveEvent::AgentCompleted {
                    outcome,
                    task_description,
                    ..
                } if task_description == description => Some(outcome),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no AgentCompleted for {description}"))
    };

    match outcome_of("sibling A") {
        AgentOutcome::Cancelled { .. } => {}
        other => panic!("sibling A: expected Cancelled, got {other:?}"),
    }
    match outcome_of("sibling B") {
        AgentOutcome::Complete { response } => assert_eq!(response, "sibling done"),
        other => panic!("sibling B: expected Complete, got {other:?}"),
    }
}

/// `shutdown()` cancels running agents through the HiveMind's internal root
/// token: a run pending on `then_hang` ends as `Cancelled`.
#[tokio::test]
async fn shutdown_cancels_running_agents() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new().then_hang();
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(vec![], LlmConfig::new("scripted", "test-model"));

    let stream = hive
        .deploy(vec![agent], vec![Task::new("shutdown task")])
        .await
        .expect("deploy agents");

    // Once the run is in flight (its provider call pends on `then_hang`),
    // shutdown the hive and drain — the run must end `Cancelled`.
    let events = tokio::time::timeout(BOUND, {
        let hive = &hive;
        let mut stream = stream;
        async move {
            let mut seen = Vec::new();
            let mut fired = false;
            while let Some(event) = stream.next().await {
                if matches!(event, HiveEvent::LlmCallStarted { .. }) && !fired {
                    hive.shutdown();
                    fired = true;
                }
                let done = matches!(event, HiveEvent::AgentCompleted { .. });
                seen.push(event);
                if done {
                    break;
                }
            }
            seen
        }
    })
    .await
    .expect("drain timed out before AgentCompleted");

    assert!(hive.is_shutdown());
    match completed_outcome(&events) {
        AgentOutcome::Cancelled { .. } => {}
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
}

/// Dropping the HiveMind cancels running agents through the same root
/// token: a run pending on `then_hang` ends as `Cancelled`, observed on the
/// event stream that was already open when the hive was dropped — the
/// spawned run holds its own event emitter, so the broadcast channel stays
/// alive until the run finishes.
#[tokio::test]
async fn drop_cancels_running_agents() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new().then_hang();
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(vec![], LlmConfig::new("scripted", "test-model"));

    let stream = hive
        .deploy(vec![agent], vec![Task::new("drop task")])
        .await
        .expect("deploy agents");

    let events = tokio::time::timeout(BOUND, {
        let mut stream = stream;
        let mut hive = Some(hive);
        async move {
            let mut seen = Vec::new();
            let mut fired = false;
            while let Some(event) = stream.next().await {
                if matches!(event, HiveEvent::LlmCallStarted { .. }) && !fired {
                    drop(hive.take());
                    fired = true;
                }
                let done = matches!(event, HiveEvent::AgentCompleted { .. });
                seen.push(event);
                if done {
                    break;
                }
            }
            seen
        }
    })
    .await
    .expect("drain timed out before AgentCompleted");

    match completed_outcome(&events) {
        AgentOutcome::Cancelled { .. } => {}
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
}

/// A substrate whose every method resolves inside a single poll — no
/// internal awaits — so a dispatch driving it can finish within the first
/// `select!` poll. The post-shutdown regression needs exactly that shape:
/// without a synchronous pre-poll cancel, `select!` may poll an
/// immediately-ready dispatch first and let it complete as `Complete`.
struct SyncSubstrate;

#[async_trait]
impl pulsedb::SubstrateProvider for SyncSubstrate {
    async fn store_experience(
        &self,
        _exp: pulsedb::NewExperience,
    ) -> std::result::Result<pulsedb::ExperienceId, pulsedb::PulseDBError> {
        Ok(pulsedb::ExperienceId::new())
    }

    async fn get_experience(
        &self,
        _id: pulsedb::ExperienceId,
    ) -> std::result::Result<Option<pulsedb::Experience>, pulsedb::PulseDBError> {
        Ok(None)
    }

    async fn search_similar(
        &self,
        _collective: pulsedb::CollectiveId,
        _embedding: &[f32],
        _k: usize,
    ) -> std::result::Result<Vec<(pulsedb::Experience, f32)>, pulsedb::PulseDBError> {
        Ok(vec![])
    }

    async fn get_recent(
        &self,
        _collective: pulsedb::CollectiveId,
        _limit: usize,
    ) -> std::result::Result<Vec<pulsedb::Experience>, pulsedb::PulseDBError> {
        Ok(vec![])
    }

    async fn store_relation(
        &self,
        _rel: pulsedb::NewExperienceRelation,
    ) -> std::result::Result<pulsedb::RelationId, pulsedb::PulseDBError> {
        Ok(pulsedb::RelationId::new())
    }

    async fn get_related(
        &self,
        _exp_id: pulsedb::ExperienceId,
    ) -> std::result::Result<
        Vec<(pulsedb::Experience, pulsedb::ExperienceRelation)>,
        pulsedb::PulseDBError,
    > {
        Ok(vec![])
    }

    async fn store_insight(
        &self,
        _insight: pulsedb::NewDerivedInsight,
    ) -> std::result::Result<pulsedb::InsightId, pulsedb::PulseDBError> {
        Ok(pulsedb::InsightId::new())
    }

    async fn get_insights(
        &self,
        _collective: pulsedb::CollectiveId,
        _embedding: &[f32],
        _k: usize,
    ) -> std::result::Result<Vec<(pulsedb::DerivedInsight, f32)>, pulsedb::PulseDBError> {
        Ok(vec![])
    }

    async fn get_activities(
        &self,
        _collective: pulsedb::CollectiveId,
    ) -> std::result::Result<Vec<pulsedb::Activity>, pulsedb::PulseDBError> {
        Ok(vec![])
    }

    async fn get_context_candidates(
        &self,
        _request: pulsedb::ContextRequest,
    ) -> std::result::Result<pulsedb::ContextCandidates, pulsedb::PulseDBError> {
        Ok(pulsedb::ContextCandidates {
            similar_experiences: vec![],
            recent_experiences: vec![],
            insights: vec![],
            relations: vec![],
            active_agents: vec![],
        })
    }

    async fn watch(
        &self,
        _collective: pulsedb::CollectiveId,
    ) -> std::result::Result<
        Pin<Box<dyn Stream<Item = pulsedb::WatchEvent> + Send>>,
        pulsedb::PulseDBError,
    > {
        Ok(Box::pin(futures::stream::empty()))
    }

    async fn create_collective(
        &self,
        _name: &str,
    ) -> std::result::Result<pulsedb::CollectiveId, pulsedb::PulseDBError> {
        Ok(pulsedb::CollectiveId::new())
    }

    async fn get_or_create_collective(
        &self,
        _name: &str,
    ) -> std::result::Result<pulsedb::CollectiveId, pulsedb::PulseDBError> {
        Ok(pulsedb::CollectiveId::new())
    }

    async fn list_collectives(
        &self,
    ) -> std::result::Result<Vec<pulsedb::Collective>, pulsedb::PulseDBError> {
        Ok(vec![])
    }
}

/// r1.s2 review (post-shutdown race): a deployment spawned when the
/// HiveMind root is already cancelled must start cancelled — the run token
/// fires before `dispatch` is ever polled. `select!` picks its first branch
/// at random, so with a one-poll-ready dispatch the missing pre-poll cancel
/// lets roughly half of deploys finish as `Complete`; repeated rounds make
/// the regression deterministic while the fixed contract holds every round.
#[tokio::test]
async fn post_shutdown_deploy_starts_cancelled() {
    let provider = ScriptedProvider::new().then_text("must never be returned");
    let hive = HiveMind::builder()
        .substrate(Box::new(SyncSubstrate))
        .llm_provider("scripted", provider.clone())
        .no_insight_synthesizer()
        .build()
        .expect("build HiveMind");
    hive.shutdown();

    for round in 0..8 {
        let agent = scripted_agent(vec![], LlmConfig::new("scripted", "test-model"));
        let stream = hive
            .deploy(vec![agent], vec![Task::new("post-shutdown")])
            .await
            .expect("deploy post-shutdown");
        let events = drain_until_completed(stream).await;
        match completed_outcome(&events) {
            AgentOutcome::Cancelled { .. } => {}
            other => panic!("round {round}: expected Cancelled, got {other:?}"),
        }
    }
}
