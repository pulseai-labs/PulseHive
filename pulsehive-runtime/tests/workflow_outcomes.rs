//! r1.s2.w3 — workflow cancellation and parallel survivors (#45).
//!
//! Sequential, Parallel and Loop honor the task's cancellation token, and a
//! Parallel stage keeps the responses of children that completed when a
//! sibling fails (`PartialComplete`). Every test drives the real path —
//! `HiveMind::deploy`, `Task::with_cancel`, a distinct `ScriptedProvider`
//! name per Parallel child so script order is deterministic — and every wait
//! is bounded by `BOUND` so a regression fails the test instead of hanging
//! it. No fakes: the providers are the product-owned `ScriptedProvider`, the
//! tool probes are ordinary `Tool` impls.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::LlmConfig;
use pulsehive_core::testing::ScriptedProvider;
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};

/// Bound on every wait — a hung run fails the test rather than hanging CI.
const BOUND: Duration = Duration::from_secs(30);

/// Builds a HiveMind on a temp substrate with each scripted provider
/// registered under its own name (parallel children get deterministic
/// scripts) and insight synthesis off (a synthesizer would consume steps
/// from an arbitrary provider).
fn scripted_hive(
    dir: &tempfile::TempDir,
    providers: Vec<(impl Into<String>, ScriptedProvider)>,
) -> HiveMind {
    let mut builder = HiveMind::builder()
        .substrate_path(dir.path().join("workflow-outcomes.db"))
        .no_insight_synthesizer();
    for (name, provider) in providers {
        builder = builder.llm_provider(name, provider);
    }
    builder.build().expect("build HiveMind")
}

/// An LLM child definition routing to the named provider.
fn llm_child(
    name: &str,
    provider: impl Into<String>,
    tools: Vec<Arc<dyn Tool>>,
) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "Work the task.".into(),
            tools,
            lens: Lens::default(),
            llm_config: LlmConfig::new(provider, "test-model"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    }
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

/// Drains the deploy stream until the named agent emits `AgentCompleted`,
/// returning every event seen. Bounded — a run that never completes fails
/// the test.
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

/// Like [`drain_until_agent_completes`], but cancels `token` when the
/// `at_call`-th `LlmCallStarted` event arrives — the in-flight-cancel probe.
async fn drain_cancelling_at_llm_call(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    agent_name: &str,
    token: CancellationToken,
    at_call: usize,
) -> Vec<HiveEvent> {
    tokio::time::timeout(BOUND, async move {
        let mut seen = Vec::new();
        let mut target_id: Option<String> = None;
        let mut calls = 0usize;
        let mut fired = false;
        while let Some(event) = stream.next().await {
            if let HiveEvent::AgentStarted { agent_id, name, .. } = &event {
                if name == agent_name {
                    target_id = Some(agent_id.clone());
                }
            }
            if matches!(event, HiveEvent::LlmCallStarted { .. }) {
                calls += 1;
                if calls == at_call && !fired {
                    token.cancel();
                    fired = true;
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

/// Drains the deploy stream until every named agent has emitted
/// `AgentCompleted`, returning every event seen. Bounded — a run whose
/// roots never all complete fails the test.
async fn drain_until_all_complete(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    agent_names: &[&str],
) -> Vec<HiveEvent> {
    tokio::time::timeout(BOUND, async move {
        let mut seen = Vec::new();
        let mut ids: HashMap<String, String> = HashMap::new();
        let mut done: std::collections::HashSet<String> = Default::default();
        while let Some(event) = stream.next().await {
            if let HiveEvent::AgentStarted { agent_id, name, .. } = &event {
                if agent_names.contains(&name.as_str()) {
                    ids.insert(name.clone(), agent_id.clone());
                }
            }
            if let HiveEvent::AgentCompleted { agent_id, .. } = &event {
                if ids.values().any(|id| id == agent_id) {
                    done.insert(agent_id.clone());
                }
            }
            let all_done = agent_names
                .iter()
                .all(|n| ids.get(*n).is_some_and(|id| done.contains(id)));
            seen.push(event);
            if all_done {
                break;
            }
        }
        seen
    })
    .await
    .expect("drain timed out before all named agents completed")
}

/// A tool that announces it started, then polls its invocation token every
/// 50 ms for up to 1 s — the cooperative-cancellation probe a Parallel child
/// runs while a test cancels the run token behind it (A3: always awaited,
/// never force-aborted).
struct PollCancelTool {
    started: Arc<Notify>,
    observed: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for PollCancelTool {
    fn name(&self) -> &str {
        "poll_cancel"
    }

    fn description(&self) -> &str {
        "Signals it started, then polls context.cancel for up to 1s"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, ctx: &ToolContext) -> Result<ToolResult> {
        self.started.notify_one();
        for _ in 0..20 {
            if ctx.cancel.is_cancelled() {
                self.observed.store(true, Ordering::SeqCst);
                return Ok(ToolResult::text("observed cancellation"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if ctx.cancel.is_cancelled() {
            self.observed.store(true, Ordering::SeqCst);
        }
        Ok(ToolResult::text("poll window elapsed"))
    }
}

/// The four parallel vectors [`poll_cancel_children`] builds, in name order.
struct PollCancelChildren {
    providers: Vec<(String, ScriptedProvider)>,
    children: Vec<AgentDefinition>,
    started: Vec<Arc<Notify>>,
    observed: Vec<Arc<AtomicBool>>,
}

/// Builds one [`PollCancelTool`] child per name — each polls its token inside
/// the tool, then would finish with a text reply if the cancel never arrived.
fn poll_cancel_children(names: &[&str]) -> PollCancelChildren {
    let mut built = PollCancelChildren {
        providers: Vec::new(),
        children: Vec::new(),
        started: Vec::new(),
        observed: Vec::new(),
    };
    for &name in names {
        let prov_name = format!("prov-{name}");
        built.providers.push((
            prov_name.clone(),
            ScriptedProvider::new()
                .then_tool_call("poll_cancel", json!({}))
                .then_text(format!("{name} done")),
        ));
        let child_started = Arc::new(Notify::new());
        let child_observed = Arc::new(AtomicBool::new(false));
        built.started.push(child_started.clone());
        built.observed.push(child_observed.clone());
        built.children.push(llm_child(
            name,
            prov_name,
            vec![Arc::new(PollCancelTool {
                started: child_started,
                observed: child_observed,
            })],
        ));
    }
    built
}

/// The AC-8 fixture's three roots: a two-child Sequential, a two-child
/// Parallel and a three-iteration Loop, matching the providers `s1`, `s2`,
/// `pa`, `pb` and `looper`.
fn uncancelled_roots() -> Vec<AgentDefinition> {
    vec![
        AgentDefinition {
            name: "wf-seq".into(),
            kind: AgentKind::Sequential(vec![
                llm_child("seq-a", "s1", vec![]),
                llm_child("seq-b", "s2", vec![]),
            ]),
        },
        AgentDefinition {
            name: "wf-par".into(),
            kind: AgentKind::Parallel(vec![
                llm_child("par-a", "pa", vec![]),
                llm_child("par-b", "pb", vec![]),
            ]),
        },
        AgentDefinition {
            name: "wf-loop".into(),
            kind: AgentKind::Loop {
                agent: Box::new(llm_child("loop-child", "looper", vec![])),
                max_iterations: 3,
            },
        },
    ]
}

/// A tool that returns instantly — the filler call a capped child loops on.
struct NoopTool;

#[async_trait]
impl Tool for NoopTool {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        "Returns immediately"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        Ok(ToolResult::text("noop ran"))
    }
}

// ── AC-1 ─────────────────────────────────────────────────────────────

/// Sequential of three children: the token is cancelled while the second
/// runs (its provider call hangs until the per-call token fires); the third
/// never starts; the outcome is `Cancelled` carrying the first child's
/// response (A16).
#[tokio::test]
async fn sequential_stops_before_next_child_on_cancel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p1 = ScriptedProvider::new().then_text("first done");
    let p2 = ScriptedProvider::new().then_hang();
    let p3 = ScriptedProvider::new().then_text("never reached");
    let hive = scripted_hive(&dir, vec![("p1", p1), ("p2", p2), ("p3", p3.clone())]);

    let workflow = AgentDefinition {
        name: "seq-cancel".into(),
        kind: AgentKind::Sequential(vec![
            llm_child("first", "p1", vec![]),
            llm_child("second", "p2", vec![]),
            llm_child("third", "p3", vec![]),
        ]),
    };

    let token = CancellationToken::new();
    let task = Task::new("sequential cancel").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");

    // Cancel when the second child's provider call is in flight — its
    // `then_hang` step resolves as a provider cancellation.
    let events = drain_cancelling_at_llm_call(stream, "seq-cancel", token, 2).await;

    match outcome_of(&events, "seq-cancel") {
        Some(AgentOutcome::Cancelled { partial_response }) => assert_eq!(
            partial_response, "first done",
            "the cancelled Sequential should carry the last completed child's response"
        ),
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
    assert!(
        p3.requests().is_empty(),
        "the third child's provider saw {} request(s) — it must never start",
        p3.requests().len()
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            HiveEvent::AgentStarted { name, .. } if name == "third"
        )),
        "AgentStarted emitted for the third child after cancellation"
    );
}

// ── AC-2 ─────────────────────────────────────────────────────────────

/// A Loop whose child never signals `[LOOP_DONE]`: cancelling while the
/// second iteration's provider call is in flight ends the loop as
/// `Cancelled` carrying the last completed iteration's response (A16),
/// well before `max_iterations`.
#[tokio::test]
async fn loop_stops_at_iteration_top_on_cancel() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Iteration 1 completes; iteration 2's call pends until the run token
    // fires (the loop would need a third step if it kept going).
    let provider = ScriptedProvider::new().then_text("iter 1").then_hang();
    let hive = scripted_hive(&dir, vec![("loop-prov", provider.clone())]);

    let workflow = AgentDefinition {
        name: "loop-cancel".into(),
        kind: AgentKind::Loop {
            agent: Box::new(llm_child("worker", "loop-prov", vec![])),
            max_iterations: 10,
        },
    };

    let token = CancellationToken::new();
    let task = Task::new("loop cancel").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");

    // The second LlmCallStarted is iteration 2's in-flight call.
    let events = drain_cancelling_at_llm_call(stream, "loop-cancel", token, 2).await;

    match outcome_of(&events, "loop-cancel") {
        Some(AgentOutcome::Cancelled { partial_response }) => assert_eq!(
            partial_response, "iter 1",
            "the cancelled Loop should carry the last completed iteration's response"
        ),
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
    assert_eq!(
        provider.requests().len(),
        2,
        "the loop should stop at the cancel, not run out its 10 iterations"
    );
}

// ── AC-3 ─────────────────────────────────────────────────────────────

/// Four Parallel children, each running a tool that polls `context.cancel`
/// every 50 ms for up to 1 s. Cancelling the task token at ~200 ms must
/// return `Cancelled` within 1 s of the cancel, every child must have
/// observed its token, and every child must still emit its own
/// `AgentCompleted` — an aborted `JoinSet` task would never emit one
/// (D5/A3: cooperative drain, never `abort_all`).
#[tokio::test]
async fn parallel_cancel_drains_cooperatively() {
    let dir = tempfile::tempdir().expect("tempdir");

    let names = ["w1", "w2", "w3", "w4"];
    let probes = poll_cancel_children(&names);
    let hive = scripted_hive(&dir, probes.providers);

    let workflow = AgentDefinition {
        name: "par-cancel".into(),
        kind: AgentKind::Parallel(probes.children),
    };

    let token = CancellationToken::new();
    let task = Task::new("parallel cancel").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");

    // Wait until all four tool bodies are inside their poll loop, then
    // cancel around the 200 ms mark of their 1 s window.
    for notify in &probes.started {
        tokio::time::timeout(BOUND, notify.notified())
            .await
            .expect("a poll tool never started");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let cancel_at = Instant::now();
    token.cancel();

    let events = tokio::time::timeout(BOUND, drain_until_agent_completes(stream, "par-cancel"))
        .await
        .expect("drain timed out before the parallel's AgentCompleted");
    let elapsed = cancel_at.elapsed();

    assert!(
        elapsed < Duration::from_secs(1),
        "cancelled Parallel took {elapsed:?} to complete — the drain must beat the 1s poll window"
    );
    match outcome_of(&events, "par-cancel") {
        Some(AgentOutcome::Cancelled { .. }) => {}
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
    for (name, flag) in names.iter().zip(&probes.observed) {
        assert!(
            flag.load(Ordering::SeqCst),
            "child {name} never observed its cancellation token"
        );
    }
    for name in names {
        match outcome_of(&events, name) {
            Some(AgentOutcome::Cancelled { .. }) => {}
            other => panic!(
                "child {name} did not complete as Cancelled — a JoinSet abort would leave no AgentCompleted (got {other:?})"
            ),
        }
    }
}

// ── AC-4 ─────────────────────────────────────────────────────────────

/// Children a and c complete while b's provider errors: the Parallel returns
/// `PartialComplete` carrying a's and c's responses plus one error naming b
/// (#45 — the survivors are no longer discarded).
#[tokio::test]
async fn parallel_with_failed_child_returns_partial_complete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pa = ScriptedProvider::new().then_text("alpha response");
    let pb = ScriptedProvider::new().then_error(PulseHiveError::llm("b exploded"));
    let pc = ScriptedProvider::new().then_text("gamma response");
    let hive = scripted_hive(&dir, vec![("pa", pa), ("pb", pb), ("pc", pc)]);

    let workflow = AgentDefinition {
        name: "par-partial".into(),
        kind: AgentKind::Parallel(vec![
            llm_child("a", "pa", vec![]),
            llm_child("b", "pb", vec![]),
            llm_child("c", "pc", vec![]),
        ]),
    };
    let task = Task::new("parallel partial");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "par-partial").await;

    match outcome_of(&events, "par-partial") {
        Some(AgentOutcome::PartialComplete { responses, errors }) => {
            assert_eq!(
                responses.len(),
                2,
                "both survivors' responses kept: {responses:?}"
            );
            assert!(responses.iter().any(|r| r == "alpha response"));
            assert!(responses.iter().any(|r| r == "gamma response"));
            assert_eq!(
                errors.len(),
                1,
                "one error for the one failed child: {errors:?}"
            );
            assert!(
                errors[0].starts_with("b: ") && errors[0].contains("b exploded"),
                "the error must name the failed child, got {:?}",
                errors[0]
            );
        }
        other => panic!("expected AgentOutcome::PartialComplete, got {other:?}"),
    }
}

// ── AC-5 ─────────────────────────────────────────────────────────────

/// `Sequential([Parallel([ok, failing]), critic])`: the Parallel ends
/// `PartialComplete`, which is progress — the critic still runs, perceives
/// the survivor's recorded work, and the sequence completes.
#[tokio::test]
async fn sequential_continues_past_partial_parallel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ok = ScriptedProvider::new().then_text("survivor text");
    let bad = ScriptedProvider::new().then_error(PulseHiveError::llm("bad child"));
    let critic = ScriptedProvider::new().then_text("critique done");
    let hive = scripted_hive(
        &dir,
        vec![("ok", ok), ("bad", bad), ("critic", critic.clone())],
    );

    let workflow = AgentDefinition {
        name: "seq-partial".into(),
        kind: AgentKind::Sequential(vec![
            AgentDefinition {
                name: "par-stage".into(),
                kind: AgentKind::Parallel(vec![
                    llm_child("survivor", "ok", vec![]),
                    llm_child("failing", "bad", vec![]),
                ]),
            },
            llm_child("critic", "critic", vec![]),
        ]),
    };
    let task = Task::new("sequential past partial");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "seq-partial").await;

    match outcome_of(&events, "seq-partial") {
        Some(AgentOutcome::Complete { response }) => assert_eq!(
            response, "critique done",
            "the Sequential should finish with the critic's response"
        ),
        other => panic!("expected AgentOutcome::Complete, got {other:?}"),
    }
    match outcome_of(&events, "par-stage") {
        Some(AgentOutcome::PartialComplete { .. }) => {}
        other => panic!("expected the inner Parallel to be PartialComplete, got {other:?}"),
    }

    // The critic's provider request must carry the survivor's text — the
    // survivor's Record phase wrote it into the shared collective before the
    // critic's Perceive ran.
    let requests = critic.requests();
    assert!(
        !requests.is_empty(),
        "the critic's provider saw no request — it never ran"
    );
    let saw_survivor_text = requests.iter().any(|req| {
        req.messages
            .iter()
            .any(|m| serde_json::to_string(m).is_ok_and(|s| s.contains("survivor text")))
    });
    assert!(
        saw_survivor_text,
        "the critic's request did not carry the survivor's work: {:?}",
        requests[0].messages
    );
}

// ── AC-6 ─────────────────────────────────────────────────────────────

/// A child that hits the agentic-loop cap (`DEFAULT_MAX_ITERATIONS`) is
/// reported in `errors` as `<agent>: max iterations reached`, not in
/// `responses` — the survivor still lands as `PartialComplete`.
#[tokio::test]
async fn parallel_counts_capped_child_as_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ok = ScriptedProvider::new().then_text("fine result");
    // 25 iterations × one scripted tool call each, plus headroom — the real
    // cap is the only producer of MaxIterationsReached (LlmAgentConfig
    // exposes no smaller one).
    let mut capped = ScriptedProvider::new();
    for _ in 0..26 {
        capped = capped.then_tool_call("noop", json!({}));
    }
    let hive = scripted_hive(&dir, vec![("ok", ok), ("capped", capped)]);

    let workflow = AgentDefinition {
        name: "par-capped".into(),
        kind: AgentKind::Parallel(vec![
            llm_child("survivor", "ok", vec![]),
            llm_child("capped", "capped", vec![Arc::new(NoopTool)]),
        ]),
    };
    let task = Task::new("parallel capped");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "par-capped").await;

    match outcome_of(&events, "par-capped") {
        Some(AgentOutcome::PartialComplete { responses, errors }) => {
            assert_eq!(responses, &["fine result".to_string()]);
            assert_eq!(
                errors,
                &["capped: max iterations reached".to_string()],
                "the capped child must be reported as '<agent>: max iterations reached'"
            );
        }
        other => panic!("expected AgentOutcome::PartialComplete, got {other:?}"),
    }
}

// ── AC-7 ─────────────────────────────────────────────────────────────

/// `Parallel([Parallel([ok, failing]), ok])`: the inner `PartialComplete`
/// flattens into the parent (A17) — one `PartialComplete` carrying both
/// survivors' responses and the inner child's named error, never a nested
/// outcome stringified into an error.
#[tokio::test]
async fn nested_partial_complete_merges_into_parent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p1 = ScriptedProvider::new().then_text("inner ok");
    let p2 = ScriptedProvider::new().then_error(PulseHiveError::llm("inner boom"));
    let p3 = ScriptedProvider::new().then_text("outer ok");
    let hive = scripted_hive(&dir, vec![("p1", p1), ("p2", p2), ("p3", p3)]);

    let workflow = AgentDefinition {
        name: "par-nested".into(),
        kind: AgentKind::Parallel(vec![
            AgentDefinition {
                name: "inner-par".into(),
                kind: AgentKind::Parallel(vec![
                    llm_child("inner-ok", "p1", vec![]),
                    llm_child("inner-bad", "p2", vec![]),
                ]),
            },
            llm_child("outer-ok", "p3", vec![]),
        ]),
    };
    let task = Task::new("nested partial");
    let stream = hive
        .deploy(vec![workflow], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_agent_completes(stream, "par-nested").await;

    match outcome_of(&events, "par-nested") {
        Some(AgentOutcome::PartialComplete { responses, errors }) => {
            assert_eq!(responses.len(), 2, "both survivors kept: {responses:?}");
            assert!(responses.iter().any(|r| r == "inner ok"));
            assert!(responses.iter().any(|r| r == "outer ok"));
            assert_eq!(errors.len(), 1, "one flattened inner error: {errors:?}");
            assert!(
                errors[0].starts_with("inner-bad: ") && errors[0].contains("inner boom"),
                "the flattened error must keep the inner child's name, got {:?}",
                errors[0]
            );
        }
        other => panic!("expected AgentOutcome::PartialComplete, got {other:?}"),
    }
}

// ── AC-8 ─────────────────────────────────────────────────────────────

/// Tokens that never fire: an all-complete Parallel returns `Complete` with
/// responses joined by newline, and Sequential and Loop return what they
/// returned before this work — last child's response, last iteration's
/// outcome.
#[tokio::test]
async fn uncancelled_workflows_are_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let hive = scripted_hive(
        &dir,
        vec![
            ("s1", ScriptedProvider::new().then_text("seq one")),
            ("s2", ScriptedProvider::new().then_text("seq two")),
            ("pa", ScriptedProvider::new().then_text("par alpha")),
            ("pb", ScriptedProvider::new().then_text("par beta")),
            (
                "looper",
                ScriptedProvider::new()
                    .then_text("iter 1")
                    .then_text("iter 2")
                    .then_text("iter 3"),
            ),
        ],
    );

    // A token that never fires, attached to a single task all three roots share.
    let token = CancellationToken::new();
    let task = Task::new("uncancelled run").with_cancel(token);
    let stream = hive
        .deploy(uncancelled_roots(), vec![task])
        .await
        .expect("deploy agents");

    // Drain until each named root has emitted its AgentCompleted.
    let events = drain_until_all_complete(stream, &["wf-seq", "wf-par", "wf-loop"]).await;

    match outcome_of(&events, "wf-par") {
        Some(AgentOutcome::Complete { response }) => {
            let lines: Vec<&str> = response.split('\n').collect();
            assert_eq!(lines.len(), 2, "responses joined by newline: {response:?}");
            assert!(lines.contains(&"par alpha") && lines.contains(&"par beta"));
        }
        other => panic!("expected wf-par Complete, got {other:?}"),
    }
    match outcome_of(&events, "wf-seq") {
        Some(AgentOutcome::Complete { response }) => {
            assert_eq!(
                response, "seq two",
                "Sequential returns the last child's response"
            )
        }
        other => panic!("expected wf-seq Complete, got {other:?}"),
    }
    match outcome_of(&events, "wf-loop") {
        Some(AgentOutcome::Complete { response }) => {
            assert_eq!(
                response, "iter 3",
                "Loop returns the last iteration's outcome"
            )
        }
        other => panic!("expected wf-loop Complete, got {other:?}"),
    }
}
