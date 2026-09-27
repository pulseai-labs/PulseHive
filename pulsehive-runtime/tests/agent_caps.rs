//! r2.s5.w3 — per-agent execution budgets (pulse-guard PH-8).
//!
//! A consumer can bound one LLM agent's work without touching any other: an
//! agent's own iteration cap and a cap on the tool calls it actually executes.
//! A tripped tool-call cap ends the turn `AgentOutcome::ToolCallCapReached`
//! naming the limit, and a parent workflow names it. An agent that sets neither
//! cap behaves exactly as in 3.0.0.
//!
//! Every test drives the real path — `HiveMind::deploy`, the product-owned
//! `ScriptedProvider`, an ordinary `Tool` impl that counts its own runs — and
//! every wait is bounded by `BOUND`, so a regression fails the test instead of
//! hanging it. The counting tool is the ground truth for L5.2 ("a call counts
//! once it reaches the tool body"): assertions are on bodies that ran and on
//! `ToolCallStarted` events, never on the loop's own bookkeeping.
//!
//! The four negative controls the spec names (`## 5. Acceptance criteria →
//! Verification notes`) each have a test here that fails when the control is
//! applied: `tool_call_cap_trips_before_the_next_call_within_one_response`
//! (cap check moved after execution), `unknown_tool_does_not_count_against_the_cap`
//! (unknown tools counted), `cancel_wins_over_the_tool_call_cap` (cancel and cap
//! check order swapped), and `final_answer_at_the_cap_completes` (a final answer
//! refused at the cap).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Bound on every wait — a hung run fails the test rather than hanging CI.
const BOUND: Duration = Duration::from_secs(30);

/// Builds a HiveMind on a temp substrate with each scripted provider
/// registered under its own name (Parallel children get deterministic scripts)
/// and insight synthesis off (a synthesizer would consume steps from an
/// arbitrary provider).
fn scripted_hive(
    dir: &tempfile::TempDir,
    providers: Vec<(impl Into<String>, ScriptedProvider)>,
) -> HiveMind {
    let mut builder = HiveMind::builder()
        .substrate_path(dir.path().join("agent-caps.db"))
        .no_insight_synthesizer();
    for (name, provider) in providers {
        builder = builder.llm_provider(name, provider);
    }
    builder.build().expect("build HiveMind")
}

/// An LLM child definition carrying the given tool set and its own execution
/// budgets (L3 — the caps ride on `LlmConfig`, which also names the provider
/// the child routes to).
fn llm_child(name: &str, tools: Vec<Arc<dyn Tool>>, llm_config: LlmConfig) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
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

/// A tool that counts the runs of its body — L5.2's ground truth — and, when
/// armed with a token, cancels the run from inside that body.
struct CountingTool {
    name: String,
    runs: Arc<AtomicUsize>,
    cancel_on_run: Option<CancellationToken>,
}

impl CountingTool {
    fn new(name: &str, runs: &Arc<AtomicUsize>) -> Self {
        Self {
            name: name.into(),
            runs: Arc::clone(runs),
            cancel_on_run: None,
        }
    }

    /// A counting tool that cancels `token` when its body runs — the
    /// `cancel_wins_over_the_tool_call_cap` probe: by the time the NEXT
    /// requested call's checkpoint runs, the run token has fired.
    fn cancelling(name: &str, runs: &Arc<AtomicUsize>, token: CancellationToken) -> Self {
        Self {
            name: name.into(),
            runs: Arc::clone(runs),
            cancel_on_run: Some(token),
        }
    }
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Counts how many times its body ran"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        if let Some(token) = &self.cancel_on_run {
            token.cancel();
        }
        Ok(ToolResult::text(format!("{} ran", self.name)))
    }
}

/// One response requesting `count` calls of the named tool — the
/// multi-call-in-one-response shape L5.1's per-call check exists for.
fn response_requesting(name: &str, count: usize) -> LlmResponse {
    LlmResponse::new(
        None,
        (1..=count)
            .map(|i| ToolCall {
                id: format!("call_{i}"),
                name: name.into(),
                arguments: json!({}),
            })
            .collect(),
        TokenUsage::default(),
    )
    .with_finish_reason("tool_calls")
}

/// The outcome a named agent completed with — `AgentStarted` carries the
/// name, `AgentCompleted` carries the same generated `agent_id`.
fn outcome_of<'a>(events: &'a [HiveEvent], agent_name: &str) -> Option<&'a AgentOutcome> {
    let ids: HashMap<&str, &str> = events
        .iter()
        .filter_map(|event| match event {
            HiveEvent::AgentStarted { agent_id, name, .. } => {
                Some((name.as_str(), agent_id.as_str()))
            }
            _ => None,
        })
        .collect();
    let target = ids.get(agent_name)?;
    events.iter().find_map(|event| match event {
        HiveEvent::AgentCompleted {
            agent_id, outcome, ..
        } if agent_id == target => Some(outcome),
        _ => None,
    })
}

/// How many `ToolCallStarted` events the named agent emitted — L5.1's
/// "the unstarted calls get no `ToolCallStarted`".
fn tool_call_starts(events: &[HiveEvent], agent_name: &str) -> usize {
    let ids: HashMap<&str, &str> = events
        .iter()
        .filter_map(|event| match event {
            HiveEvent::AgentStarted { agent_id, name, .. } => {
                Some((name.as_str(), agent_id.as_str()))
            }
            _ => None,
        })
        .collect();
    let Some(target) = ids.get(agent_name) else {
        return 0;
    };
    events
        .iter()
        .filter(|event| matches!(event, HiveEvent::ToolCallStarted { agent_id, .. } if agent_id == target))
        .count()
}

/// Drains the deploy stream until the named agent emits `AgentCompleted`,
/// returning every event seen. Bounded — a run that never completes fails the
/// test.
async fn drain_until_agent_completes(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    agent_name: &str,
) -> Vec<HiveEvent> {
    tokio::time::timeout(BOUND, async move {
        let mut seen = Vec::new();
        let mut target_id: Option<String> = None;
        while let Some(event) = stream.next().await {
            if let HiveEvent::AgentStarted { agent_id, name, .. } = &event {
                if name == agent_name {
                    target_id = Some(agent_id.clone());
                }
            }
            let done = matches!(&event, HiveEvent::AgentCompleted { agent_id, .. }
                if Some(agent_id) == target_id.as_ref());
            seen.push(event);
            if done {
                break;
            }
        }
        seen
    })
    .await
    .expect("drain timed out before the agent's AgentCompleted")
}

/// L5.1 — the cap is checked before each invocation, including the second and
/// later calls in one response. The third requested call must not start.
#[tokio::test]
async fn tool_call_cap_trips_before_the_next_call_within_one_response() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new().then_response(response_requesting("count", 3));
    let hive = scripted_hive(&dir, vec![("prov", provider)]);
    let runs = Arc::new(AtomicUsize::new(0));
    let workflow = llm_child(
        "capped",
        vec![Arc::new(CountingTool::new("count", &runs))],
        LlmConfig::new("prov", "test-model").with_max_tool_calls(2),
    );

    let task = Task::new("one response requesting more calls than the cap");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "capped").await;

    match outcome_of(&events, "capped") {
        Some(AgentOutcome::ToolCallCapReached { limit }) => assert_eq!(
            *limit, 2,
            "the outcome must carry the configured cap, not the count that ran"
        ),
        other => panic!("expected AgentOutcome::ToolCallCapReached, got {other:?}"),
    }
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "exactly the cap's worth of tool bodies must run"
    );
    assert_eq!(
        tool_call_starts(&events, "capped"),
        2,
        "the two started calls get ToolCallStarted and the third must not"
    );
}

/// L5.3 — `Some(0)` is literal: the first requested call trips the cap, and no
/// body runs.
#[tokio::test]
async fn tool_call_cap_zero_trips_on_the_first_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new().then_response(response_requesting("count", 1));
    let hive = scripted_hive(&dir, vec![("prov", provider)]);
    let runs = Arc::new(AtomicUsize::new(0));
    let workflow = llm_child(
        "capped",
        vec![Arc::new(CountingTool::new("count", &runs))],
        LlmConfig::new("prov", "test-model").with_max_tool_calls(0),
    );

    let task = Task::new("a zero tool-call cap");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "capped").await;

    match outcome_of(&events, "capped") {
        Some(AgentOutcome::ToolCallCapReached { limit }) => assert_eq!(*limit, 0),
        other => panic!("expected AgentOutcome::ToolCallCapReached {{ limit: 0 }}, got {other:?}"),
    }
    assert_eq!(runs.load(Ordering::SeqCst), 0, "no body may run at a zero cap");
    assert_eq!(
        tool_call_starts(&events, "capped"),
        0,
        "an unstarted call gets no ToolCallStarted"
    );
}

/// L5.2 — a call counts once it reaches the tool body, so an unknown tool
/// consumes the model's request without consuming the budget. Counting it
/// would block the real call beside it and stop the turn one call early.
#[tokio::test]
async fn unknown_tool_does_not_count_against_the_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new()
        .then_response(LlmResponse::new(
            None,
            vec![
                ToolCall {
                    id: "call_1".into(),
                    name: "no_such_tool".into(),
                    arguments: json!({}),
                },
                ToolCall {
                    id: "call_2".into(),
                    name: "count".into(),
                    arguments: json!({}),
                },
            ],
            TokenUsage::default(),
        ))
        .then_response(response_requesting("count", 1));
    let hive = scripted_hive(&dir, vec![("prov", provider)]);
    let runs = Arc::new(AtomicUsize::new(0));
    let workflow = llm_child(
        "capped",
        vec![Arc::new(CountingTool::new("count", &runs))],
        LlmConfig::new("prov", "test-model").with_max_tool_calls(1),
    );

    let task = Task::new("an unknown tool beside a real one");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "capped").await;

    match outcome_of(&events, "capped") {
        Some(AgentOutcome::ToolCallCapReached { limit }) => assert_eq!(
            *limit, 1,
            "the real call must run, and only the NEXT requested call may trip the cap"
        ),
        other => panic!("expected AgentOutcome::ToolCallCapReached, got {other:?}"),
    }
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the real call ran; the unknown tool did not consume the budget"
    );
    assert_eq!(tool_call_starts(&events, "capped"), 1);
}

/// L5.5 — the cap bounds work, not the ability to answer with what the work
/// produced: a final response with no tool calls ends `Complete` even at
/// `executed == limit`.
#[tokio::test]
async fn final_answer_at_the_cap_completes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new()
        .then_response(response_requesting("count", 1))
        .then_text("answered at the cap");
    let hive = scripted_hive(&dir, vec![("prov", provider)]);
    let runs = Arc::new(AtomicUsize::new(0));
    let workflow = llm_child(
        "capped",
        vec![Arc::new(CountingTool::new("count", &runs))],
        LlmConfig::new("prov", "test-model").with_max_tool_calls(1),
    );

    let task = Task::new("a final answer exactly at the cap");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "capped").await;

    match outcome_of(&events, "capped") {
        Some(AgentOutcome::Complete { response }) => assert_eq!(response, "answered at the cap"),
        other => panic!("expected AgentOutcome::Complete at the cap, got {other:?}"),
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

/// L3/L5.3 — `max_iterations: Some(k)` bounds the loop itself: exactly k
/// provider calls, then `MaxIterationsReached`.
#[tokio::test]
async fn iteration_cap_stops_after_k_llm_calls() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new()
        .then_response(response_requesting("count", 1))
        .then_response(response_requesting("count", 1))
        .then_response(response_requesting("count", 1));
    let hive = scripted_hive(&dir, vec![("prov", provider.clone())]);
    let runs = Arc::new(AtomicUsize::new(0));
    let workflow = llm_child(
        "capped",
        vec![Arc::new(CountingTool::new("count", &runs))],
        LlmConfig::new("prov", "test-model").with_max_iterations(2),
    );

    let task = Task::new("an iteration cap of two");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "capped").await;

    assert!(
        matches!(
            outcome_of(&events, "capped"),
            Some(AgentOutcome::MaxIterationsReached)
        ),
        "expected AgentOutcome::MaxIterationsReached, got {:?}",
        outcome_of(&events, "capped")
    );
    assert_eq!(
        provider.requests().len(),
        2,
        "exactly k provider calls for an iteration cap of k"
    );
}

/// L5.3 — `Some(0)` is literal for the iteration cap too: `MaxIterationsReached`
/// with no LLM call at all.
#[tokio::test]
async fn iteration_cap_zero_makes_no_llm_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new().then_text("never served");
    let hive = scripted_hive(&dir, vec![("prov", provider.clone())]);
    let workflow = llm_child(
        "capped",
        vec![],
        LlmConfig::new("prov", "test-model").with_max_iterations(0),
    );

    let task = Task::new("an iteration cap of zero");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "capped").await;

    assert!(
        matches!(
            outcome_of(&events, "capped"),
            Some(AgentOutcome::MaxIterationsReached)
        ),
        "expected AgentOutcome::MaxIterationsReached, got {:?}",
        outcome_of(&events, "capped")
    );
    assert!(
        provider.requests().is_empty(),
        "a zero iteration cap must make no provider call"
    );
}

/// L5.5 — cancellation wins over a cap. The first body cancels the run, so the
/// second requested call meets the cancellation checkpoint (ADR-014 A7) with
/// `executed == limit` behind it: `Cancelled`, not `ToolCallCapReached`.
#[tokio::test]
async fn cancel_wins_over_the_tool_call_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new().then_response(response_requesting("count", 2));
    let hive = scripted_hive(&dir, vec![("prov", provider)]);
    let runs = Arc::new(AtomicUsize::new(0));
    let token = CancellationToken::new();
    let workflow = llm_child(
        "capped",
        vec![Arc::new(CountingTool::cancelling("count", &runs, token.clone()))],
        LlmConfig::new("prov", "test-model").with_max_tool_calls(1),
    );

    let task = Task::new("cancel from inside the capped body").with_cancel(token);
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "capped").await;

    assert!(
        matches!(
            outcome_of(&events, "capped"),
            Some(AgentOutcome::Cancelled { .. })
        ),
        "a run token cancelled inside the first body outranks the cap \
         reached by that same body — expected AgentOutcome::Cancelled, got {:?}",
        outcome_of(&events, "capped")
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "only the first body ran; the second call stopped at the checkpoint"
    );
}

/// L4 with w1's L2 — a capped child beside an uncapped sibling: the sibling
/// runs past both of the capped child's thresholds and completes, the composite
/// is `PartialComplete`, and the capped child is named with its limit.
#[tokio::test]
async fn uncapped_sibling_runs_past_both_thresholds() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The capped child trips on the second call of its single response.
    let capped = ScriptedProvider::new().then_response(response_requesting("count", 2));
    // The sibling executes three calls across three iterations (past the
    // capped child's tool-call limit AND its iteration cap) and then answers.
    let uncapped = ScriptedProvider::new()
        .then_response(response_requesting("count", 1))
        .then_response(response_requesting("count", 1))
        .then_response(response_requesting("count", 1))
        .then_text("sibling done");
    let hive = scripted_hive(&dir, vec![("capped-prov", capped), ("uncapped-prov", uncapped)]);
    let capped_runs = Arc::new(AtomicUsize::new(0));
    let uncapped_runs = Arc::new(AtomicUsize::new(0));

    let workflow = AgentDefinition {
        name: "par-caps".into(),
        kind: AgentKind::Parallel(vec![
            llm_child(
                "capped",
                vec![Arc::new(CountingTool::new("count", &capped_runs))],
                LlmConfig::new("capped-prov", "test-model")
                    .with_max_iterations(2)
                    .with_max_tool_calls(1),
            ),
            llm_child(
                "uncapped",
                vec![Arc::new(CountingTool::new("count", &uncapped_runs))],
                LlmConfig::new("uncapped-prov", "test-model"),
            ),
        ]),
    };
    let task = Task::new("a capped child beside an uncapped sibling");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "par-caps").await;

    assert_eq!(
        capped_runs.load(Ordering::SeqCst),
        1,
        "the capped child runs its cap's worth and no more"
    );
    assert_eq!(
        uncapped_runs.load(Ordering::SeqCst),
        3,
        "the uncapped sibling's budgets are its own — past both of the capped \
         child's thresholds"
    );

    match outcome_of(&events, "par-caps") {
        Some(AgentOutcome::PartialComplete { responses, errors }) => {
            assert_eq!(
                responses,
                &["sibling done".to_string()],
                "the sibling's response survives: {responses:?}"
            );
            assert_eq!(errors.len(), 1, "one error, naming the capped child: {errors:?}");
            assert_eq!(
                errors[0], "capped: tool call cap reached (limit 1)",
                "the composite names the capped child and the limit it hit"
            );
        }
        other => panic!("expected AgentOutcome::PartialComplete, got {other:?}"),
    }
    assert!(
        matches!(
            outcome_of(&events, "uncapped"),
            Some(AgentOutcome::Complete { .. })
        ),
        "the uncapped sibling completes: {:?}",
        outcome_of(&events, "uncapped")
    );
}

/// L5.4 — counters are per dispatch. A `Loop` whose child is capped at one
/// call executes one call per iteration: a counter shared across dispatches
/// would block the second iteration's first call and end the loop on a cap
/// trip instead of completing.
#[tokio::test]
async fn loop_redispatch_resets_the_counter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new()
        .then_response(response_requesting("count", 1))
        .then_text("iter text")
        .then_response(response_requesting("count", 1))
        .then_text("iter text")
        .then_response(response_requesting("count", 1))
        .then_text("iter text");
    let hive = scripted_hive(&dir, vec![("loop-prov", provider.clone())]);
    let runs = Arc::new(AtomicUsize::new(0));

    let workflow = AgentDefinition {
        name: "loop-caps".into(),
        kind: AgentKind::Loop {
            agent: Box::new(llm_child(
                "worker",
                vec![Arc::new(CountingTool::new("count", &runs))],
                LlmConfig::new("loop-prov", "test-model").with_max_tool_calls(1),
            )),
            max_iterations: 3,
        },
    };
    let task = Task::new("a loop re-dispatching a capped child");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "loop-caps").await;

    match outcome_of(&events, "loop-caps") {
        Some(AgentOutcome::Complete { response }) => assert_eq!(
            response, "iter text",
            "each iteration completes after its one executed call"
        ),
        other => panic!("expected the Loop to complete, got {other:?}"),
    }
    assert_eq!(
        runs.load(Ordering::SeqCst),
        3,
        "one executed call per dispatch — the counter starts at zero each time (L5.4)"
    );
    assert_eq!(
        provider.requests().len(),
        6,
        "two provider calls per dispatch, three dispatches"
    );
}
