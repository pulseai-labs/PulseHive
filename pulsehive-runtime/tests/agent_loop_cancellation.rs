//! r1.s2.w2 — the agent loop honors its run token (ADR-014).
//!
//! Every test drives the real path (`HiveMind::deploy`, `Task::with_cancel`,
//! `ScriptedProvider`), and every wait is bounded by `BOUND` so a regression
//! fails the test instead of hanging it. No fakes: the provider is the
//! product-owned `ScriptedProvider`, the tool probes are ordinary `Tool` impls.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive_core::approval::{ApprovalHandler, ApprovalResult, PendingAction};
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
        .substrate_path(dir.path().join("cancel-loop.db"))
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

/// A tool that announces it started, then blocks until the test releases it —
/// the probe a test cancels the run token behind (A3: it is always awaited,
/// never force-aborted).
struct GateTool {
    tool_name: &'static str,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl Tool for GateTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Signals it started, then waits for release"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(ToolResult::text(format!("{} done", self.tool_name)))
    }
}

/// A tool that records whether its body ever ran.
struct SpyTool {
    tool_name: &'static str,
    executed: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for SpyTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Records that it executed"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        self.executed.store(true, Ordering::SeqCst);
        Ok(ToolResult::text(format!("{} ran", self.tool_name)))
    }
}

/// A tool that requires approval and records whether its body ever ran —
/// the probe for the approval-boundary cancellation check.
struct ApprovalTool {
    tool_name: &'static str,
    executed: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for ApprovalTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Requires approval; records that it executed"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    fn requires_approval(&self) -> bool {
        true
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        self.executed.store(true, Ordering::SeqCst);
        Ok(ToolResult::text(format!("{} ran", self.tool_name)))
    }
}

/// An approval handler that blocks until the test releases it, then returns
/// the scripted decision — the probe for "cancelled while awaiting approval".
struct GateApproval {
    release: Arc<Notify>,
    result: ApprovalResult,
}

#[async_trait]
impl ApprovalHandler for GateApproval {
    async fn request_approval(&self, _action: &PendingAction) -> Result<ApprovalResult> {
        self.release.notified().await;
        Ok(self.result.clone())
    }
}

/// Drains the deploy stream until `AgentCompleted`; on the first
/// `ToolApprovalRequested` it cancels `token` and releases the gated
/// approval handler — the cancel lands while `request_approval` is awaited.
async fn drain_cancelling_at_approval(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    token: CancellationToken,
    release: Arc<Notify>,
) -> Vec<HiveEvent> {
    tokio::time::timeout(BOUND, async move {
        let mut seen = Vec::new();
        let mut fired = false;
        while let Some(event) = stream.next().await {
            if matches!(event, HiveEvent::ToolApprovalRequested { .. }) && !fired {
                token.cancel();
                release.notify_one();
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

#[tokio::test]
async fn pre_cancelled_turn_makes_no_provider_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A scripted step that must never be consumed — if the turn reaches the
    // provider at all, the request log below records it.
    let provider = ScriptedProvider::new().then_text("should never be read");
    let hive = scripted_hive(&dir, provider.clone());
    let agent = scripted_agent(vec![], LlmConfig::new("scripted", "test-model"));

    let token = CancellationToken::new();
    token.cancel();
    let task = Task::new("pre-cancelled task").with_cancel(token);

    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_until_completed(stream).await;

    match completed_outcome(&events) {
        AgentOutcome::Cancelled { partial_response } => assert_eq!(
            partial_response, "",
            "a turn cancelled before any LLM call has no assistant text"
        ),
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
    assert!(
        provider.requests().is_empty(),
        "provider saw {} request(s) on a pre-cancelled turn",
        provider.requests().len()
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, HiveEvent::LlmCallStarted { .. })),
        "LlmCallStarted emitted on a pre-cancelled turn"
    );
}

#[tokio::test]
async fn in_flight_provider_call_aborts_to_cancelled() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The provider pends until the call's cancel token fires — the turn ends
    // only if the run token reaches the call as `LlmConfig.cancel`.
    let provider = ScriptedProvider::new().then_hang();
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(vec![], LlmConfig::new("scripted", "test-model"));

    let token = CancellationToken::new();
    let task = Task::new("in-flight cancel").with_cancel(token.clone());

    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_cancelling_at_llm_start(stream, token).await;

    assert!(
        matches!(completed_outcome(&events), AgentOutcome::Cancelled { .. }),
        "expected AgentOutcome::Cancelled, got {:?}",
        completed_outcome(&events)
    );
}

#[tokio::test]
async fn no_tool_call_starts_after_cancel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a_started = Arc::new(Notify::new());
    let a_release = Arc::new(Notify::new());
    let b_executed = Arc::new(AtomicBool::new(false));

    // One response carrying two tool calls: A runs, B must never start once
    // the run token fires mid-A. `then_response` because `then_tool_call`
    // scripts single-call replies only.
    let provider = ScriptedProvider::new().then_response(LlmResponse::new(
        None,
        vec![
            ToolCall {
                id: "call_a".into(),
                name: "tool_a".into(),
                arguments: json!({}),
            },
            ToolCall {
                id: "call_b".into(),
                name: "tool_b".into(),
                arguments: json!({}),
            },
        ],
        TokenUsage::default(),
    ));
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![
            Arc::new(GateTool {
                tool_name: "tool_a",
                started: a_started.clone(),
                release: a_release.clone(),
            }),
            Arc::new(SpyTool {
                tool_name: "tool_b",
                executed: b_executed.clone(),
            }),
        ],
        LlmConfig::new("scripted", "test-model"),
    );

    let token = CancellationToken::new();
    let task = Task::new("two tool calls").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");

    // Cancel while A executes, then release it — A3: the running tool is
    // awaited and its result still reaches ToolCallCompleted.
    tokio::time::timeout(BOUND, a_started.notified())
        .await
        .expect("tool A never started");
    token.cancel();
    a_release.notify_one();

    let events = drain_until_completed(stream).await;

    assert!(
        matches!(completed_outcome(&events), AgentOutcome::Cancelled { .. }),
        "expected AgentOutcome::Cancelled, got {:?}",
        completed_outcome(&events)
    );
    assert!(
        events.iter().any(
            |event| matches!(event, HiveEvent::ToolCallCompleted { tool_name, .. } if tool_name == "tool_a")
        ),
        "tool A's ToolCallCompleted was not emitted"
    );
    assert!(
        !events.iter().any(
            |event| matches!(event, HiveEvent::ToolCallStarted { tool_name, .. } if tool_name == "tool_b")
        ),
        "ToolCallStarted emitted for tool B after cancellation"
    );
    assert!(
        !b_executed.load(Ordering::SeqCst),
        "tool B's body ran after cancellation"
    );
}

#[tokio::test]
async fn partial_response_is_latest_assistant_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let a_started = Arc::new(Notify::new());
    let a_release = Arc::new(Notify::new());

    // A response carrying text AND a tool call: the text is the partial the
    // cancelled turn carries back.
    let provider = ScriptedProvider::new().then_response(LlmResponse::new(
        Some("partial draft answer".to_string()),
        vec![ToolCall {
            id: "call_a".into(),
            name: "tool_a".into(),
            arguments: json!({}),
        }],
        TokenUsage::default(),
    ));
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![Arc::new(GateTool {
            tool_name: "tool_a",
            started: a_started.clone(),
            release: a_release.clone(),
        })],
        LlmConfig::new("scripted", "test-model"),
    );

    let token = CancellationToken::new();
    let task = Task::new("partial text").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");

    // Cancel during the tool call; the assistant text was already produced.
    tokio::time::timeout(BOUND, a_started.notified())
        .await
        .expect("tool never started");
    token.cancel();
    a_release.notify_one();

    let events = drain_until_completed(stream).await;
    match completed_outcome(&events) {
        AgentOutcome::Cancelled { partial_response } => {
            assert_eq!(partial_response, "partial draft answer")
        }
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
}

#[tokio::test]
async fn caller_llm_config_token_still_cancels() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The agent definition's own LlmConfig.cancel must still abort the call
    // (A15) — the loop's per-call token must not drop or replace it.
    let caller_token = CancellationToken::new();
    let provider = ScriptedProvider::new().then_hang();
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![],
        LlmConfig::new("scripted", "test-model").with_cancel(caller_token.clone()),
    );

    // The task carries a token too — it never fires; only the caller's does.
    let task_token = CancellationToken::new();
    let task = Task::new("caller config token").with_cancel(task_token);

    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_cancelling_at_llm_start(stream, caller_token).await;

    assert!(
        matches!(completed_outcome(&events), AgentOutcome::Cancelled { .. }),
        "expected AgentOutcome::Cancelled, got {:?}",
        completed_outcome(&events)
    );
}

#[tokio::test]
async fn uncancelled_turn_is_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    // One shared queue serves both runs identically: a tool call, then text.
    let provider = ScriptedProvider::new()
        .then_tool_call("echo", json!({"text": "hi"}))
        .then_text("done")
        .then_tool_call("echo", json!({"text": "hi"}))
        .then_text("done");
    let hive = scripted_hive(&dir, provider);
    let agent = || {
        scripted_agent(
            vec![Arc::new(SpyTool {
                tool_name: "echo",
                executed: Arc::new(AtomicBool::new(false)),
            })],
            LlmConfig::new("scripted", "test-model"),
        )
    };

    // Run 1: the task carries a token that never fires.
    let token = CancellationToken::new();
    let stream = hive
        .deploy(
            vec![agent()],
            vec![Task::new("with token").with_cancel(token)],
        )
        .await
        .expect("deploy with-token run");
    let with_token = drain_until_completed(stream).await;

    // Run 2: no token at all.
    let stream = hive
        .deploy(vec![agent()], vec![Task::new("no token")])
        .await
        .expect("deploy no-token run");
    let without_token = drain_until_completed(stream).await;

    for (label, events) in [
        ("with token", &with_token),
        ("without token", &without_token),
    ] {
        match completed_outcome(events) {
            AgentOutcome::Complete { response } => assert_eq!(response, "done"),
            other => panic!("{label}: expected Complete, got {other:?}"),
        }
    }

    // The same turn envelope, event variant for event variant.
    // `WatchNotification` is filtered out: it is forwarded asynchronously from
    // the substrate watch, not part of the turn's own event sequence.
    let kinds = |events: &[HiveEvent]| -> Vec<String> {
        events
            .iter()
            .filter(|event| !matches!(event, HiveEvent::WatchNotification { .. }))
            .map(|event| {
                serde_json::to_value(event).expect("event serializes")["type"]
                    .as_str()
                    .expect("event has a type tag")
                    .to_string()
            })
            .collect()
    };
    assert_eq!(
        kinds(&with_token),
        kinds(&without_token),
        "event sequence differs between token-carrying and tokenless runs"
    );
}

/// r1.s2 review (approval boundary): a run cancelled while
/// `request_approval` is awaited must not let an `Approved` decision start
/// the tool — no `ToolCallStarted`, no tool body.
#[tokio::test]
async fn cancel_during_approval_blocks_approved_tool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executed = Arc::new(AtomicBool::new(false));
    let release = Arc::new(Notify::new());

    let provider = ScriptedProvider::new().then_tool_call("guarded", json!({}));
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("cancel-loop.db"))
        .llm_provider("scripted", provider)
        .approval_handler(GateApproval {
            release: release.clone(),
            result: ApprovalResult::Approved,
        })
        .no_insight_synthesizer()
        .build()
        .expect("build HiveMind");
    let agent = scripted_agent(
        vec![Arc::new(ApprovalTool {
            tool_name: "guarded",
            executed: executed.clone(),
        })],
        LlmConfig::new("scripted", "test-model"),
    );

    let token = CancellationToken::new();
    let task = Task::new("approved but cancelled").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_cancelling_at_approval(stream, token, release).await;

    assert!(
        matches!(completed_outcome(&events), AgentOutcome::Cancelled { .. }),
        "expected AgentOutcome::Cancelled, got {:?}",
        completed_outcome(&events)
    );
    assert!(
        !events.iter().any(
            |event| matches!(event, HiveEvent::ToolCallStarted { tool_name, .. } if tool_name == "guarded")
        ),
        "ToolCallStarted emitted for a tool whose approval raced a cancel"
    );
    assert!(
        !executed.load(Ordering::SeqCst),
        "approved tool body ran after the run was cancelled"
    );
}

/// Same boundary, `Modified` path: an approval carrying rewritten params
/// must not reach `execute_tool_inner` after the run token fired.
#[tokio::test]
async fn cancel_during_approval_blocks_modified_tool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executed = Arc::new(AtomicBool::new(false));
    let release = Arc::new(Notify::new());

    let provider = ScriptedProvider::new().then_tool_call("guarded", json!({}));
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("cancel-loop.db"))
        .llm_provider("scripted", provider)
        .approval_handler(GateApproval {
            release: release.clone(),
            result: ApprovalResult::Modified {
                new_params: json!({"rewritten": true}),
            },
        })
        .no_insight_synthesizer()
        .build()
        .expect("build HiveMind");
    let agent = scripted_agent(
        vec![Arc::new(ApprovalTool {
            tool_name: "guarded",
            executed: executed.clone(),
        })],
        LlmConfig::new("scripted", "test-model"),
    );

    let token = CancellationToken::new();
    let task = Task::new("modified but cancelled").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");
    let events = drain_cancelling_at_approval(stream, token, release).await;

    assert!(
        matches!(completed_outcome(&events), AgentOutcome::Cancelled { .. }),
        "expected AgentOutcome::Cancelled, got {:?}",
        completed_outcome(&events)
    );
    assert!(
        !events.iter().any(
            |event| matches!(event, HiveEvent::ToolCallStarted { tool_name, .. } if tool_name == "guarded")
        ),
        "ToolCallStarted emitted for a tool whose modified approval raced a cancel"
    );
    assert!(
        !executed.load(Ordering::SeqCst),
        "modified-approval tool body ran after the run was cancelled"
    );
}

/// r1.s2 review (provider pre-cancel): an agent definition whose
/// `LlmConfig.cancel` is already cancelled must cancel the per-call token
/// synchronously — a spawned bridge is only polled after the call is
/// underway, so a fast provider would observe a still-live token and
/// complete. The provider's own request log proves what it saw at call
/// start.
#[tokio::test]
async fn pre_cancelled_definition_token_cancels_call_synchronously() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A ready scripted reply: without the synchronous cancel the call
    // completes as `Complete` before the bridge task is ever polled.
    let provider = ScriptedProvider::new().then_text("must not be returned");
    let hive = scripted_hive(&dir, provider.clone());
    let caller_token = CancellationToken::new();
    caller_token.cancel();
    let agent = scripted_agent(
        vec![],
        LlmConfig::new("scripted", "test-model").with_cancel(caller_token),
    );

    // The task carries no token — only the definition's token is fired.
    let stream = hive
        .deploy(vec![agent], vec![Task::new("pre-cancelled config")])
        .await
        .expect("deploy agents");
    let events = drain_until_completed(stream).await;

    assert!(
        matches!(completed_outcome(&events), AgentOutcome::Cancelled { .. }),
        "expected AgentOutcome::Cancelled, got {:?}",
        completed_outcome(&events)
    );
    let requests = provider.requests();
    assert_eq!(
        requests.len(),
        1,
        "provider saw {} requests",
        requests.len()
    );
    assert!(
        requests[0]
            .config
            .cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled),
        "call token was still live when the provider inspected it"
    );
}

/// r1.s2 review (partial_response correctness): a later tool-call response
/// carrying `content: Some("")` — the shape providers emit for a tool-only
/// turn — must not erase the last real assistant text; the cancelled run
/// still reports it.
#[tokio::test]
async fn empty_tool_call_content_preserves_partial_response() {
    let dir = tempfile::tempdir().expect("tempdir");
    let b_started = Arc::new(Notify::new());
    let b_release = Arc::new(Notify::new());

    // Turn: text + instant tool call, then an empty-content tool call the
    // test cancels behind.
    let provider = ScriptedProvider::new()
        .then_response(LlmResponse::new(
            Some("real draft".to_string()),
            vec![ToolCall {
                id: "call_a".into(),
                name: "tool_a".into(),
                arguments: json!({}),
            }],
            TokenUsage::default(),
        ))
        .then_response(LlmResponse::new(
            Some(String::new()),
            vec![ToolCall {
                id: "call_b".into(),
                name: "tool_b".into(),
                arguments: json!({}),
            }],
            TokenUsage::default(),
        ));
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![
            Arc::new(SpyTool {
                tool_name: "tool_a",
                executed: Arc::new(AtomicBool::new(false)),
            }),
            Arc::new(GateTool {
                tool_name: "tool_b",
                started: b_started.clone(),
                release: b_release.clone(),
            }),
        ],
        LlmConfig::new("scripted", "test-model"),
    );

    let token = CancellationToken::new();
    let task = Task::new("empty content turn").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");

    // Cancel while tool B is in flight — after the empty-content response
    // was folded into partial_response.
    tokio::time::timeout(BOUND, b_started.notified())
        .await
        .expect("tool B never started");
    token.cancel();
    b_release.notify_one();

    let events = drain_until_completed(stream).await;
    match completed_outcome(&events) {
        AgentOutcome::Cancelled { partial_response } => assert_eq!(
            partial_response, "real draft",
            "an empty tool-call response must not erase the last assistant text"
        ),
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
}

/// A tool that cancels the run token inside its own body — the probe for
/// "cancelled during the final iteration's last tool call".
struct CancellingTool {
    tool_name: &'static str,
    token: CancellationToken,
}

#[async_trait]
impl Tool for CancellingTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Cancels the run token mid-execution"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        self.token.cancel();
        Ok(ToolResult::text(format!(
            "{} cancelled the run",
            self.tool_name
        )))
    }
}

/// r1.s2 review (max-iterations boundary): cancellation landing during the
/// last tool call of the final iteration must still return `Cancelled`
/// with the turn's partial response — not `MaxIterationsReached`. Drives
/// `run_agentic_loop` directly so `max_iterations = 1` puts the only tool
/// call on the final iteration (the same path `LoopContext` exposes).
#[tokio::test]
async fn cancel_during_final_tool_returns_cancelled_not_max_iterations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = pulsedb::PulseDB::open(dir.path().join("cap.db"), pulsedb::Config::default())
        .expect("open temp substrate");
    let substrate: Arc<dyn pulsedb::SubstrateProvider> =
        Arc::new(pulsedb::PulseDBSubstrate::from_db(db));

    let token = CancellationToken::new();
    // One iteration: the provider answers with text plus a tool call; the
    // tool cancels the run token while executing; the loop then reaches
    // the iteration cap.
    let provider = ScriptedProvider::new().then_response(LlmResponse::new(
        Some("turn draft".to_string()),
        vec![ToolCall {
            id: "call_1".into(),
            name: "selfcancel".into(),
            arguments: json!({}),
        }],
        TokenUsage::default(),
    ));

    let config = LlmAgentConfig {
        system_prompt: "Work the task.".into(),
        tools: vec![Arc::new(CancellingTool {
            tool_name: "selfcancel",
            token: token.clone(),
        })],
        lens: Lens::default(),
        llm_config: LlmConfig::new("scripted", "test-model"),
        experience_extractor: None,
        refresh_every_n_tool_calls: None,
    };
    let task = Task::new("cancel during the final tool");
    let outcome = pulsehive_runtime::agentic_loop::run_agentic_loop(
        config,
        pulsehive_runtime::agentic_loop::LoopContext {
            agent_id: "cap-agent".into(),
            task: &task,
            provider: Arc::new(provider),
            substrate,
            approval_handler: &pulsehive_core::approval::AutoApprove,
            event_emitter: pulsehive_core::event::EventEmitter::default(),
            max_iterations: 1,
            embedding_provider: None,
            cancel: token,
        },
    )
    .await;

    match outcome {
        AgentOutcome::Cancelled { partial_response } => assert_eq!(
            partial_response, "turn draft",
            "the cancelled run should carry the turn's partial response"
        ),
        other => panic!("expected AgentOutcome::Cancelled, got {other:?}"),
    }
}

/// An approval handler whose decision never arrives — the probe proving the
/// approval await races the run token. Without the `select!`, a turn
/// cancelled while `request_approval` is pending waits on the handler
/// forever and emits no terminal `AgentCompleted`.
struct NeverApprove;

#[async_trait]
impl ApprovalHandler for NeverApprove {
    async fn request_approval(&self, _action: &PendingAction) -> Result<ApprovalResult> {
        std::future::pending::<Result<ApprovalResult>>().await
    }
}

/// r1.s2 review (P1 approval race): cancelling while the approval handler
/// stays pending must end the turn — the pending approval future is
/// dropped and the run completes as `Cancelled`, still emitting
/// `AgentCompleted`, with no tool body ever starting.
#[tokio::test]
async fn cancel_ends_turn_while_approval_never_resolves() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executed = Arc::new(AtomicBool::new(false));

    let provider = ScriptedProvider::new().then_tool_call("guarded", json!({}));
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("cancel-loop.db"))
        .llm_provider("scripted", provider)
        .approval_handler(NeverApprove)
        .no_insight_synthesizer()
        .build()
        .expect("build HiveMind");
    let agent = scripted_agent(
        vec![Arc::new(ApprovalTool {
            tool_name: "guarded",
            executed: executed.clone(),
        })],
        LlmConfig::new("scripted", "test-model"),
    );

    let token = CancellationToken::new();
    let task = Task::new("cancel a pending approval").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");
    // The handler is never released — only the run token can end the turn.
    let events = drain_cancelling_at_approval(stream, token, Arc::new(Notify::new())).await;

    assert!(
        matches!(completed_outcome(&events), AgentOutcome::Cancelled { .. }),
        "expected AgentOutcome::Cancelled, got {:?}",
        completed_outcome(&events)
    );
    assert!(
        !events.iter().any(
            |event| matches!(event, HiveEvent::ToolCallStarted { tool_name, .. } if tool_name == "guarded")
        ),
        "ToolCallStarted emitted for a tool whose approval never resolved"
    );
    assert!(
        !executed.load(Ordering::SeqCst),
        "tool body ran after the run was cancelled mid-approval"
    );
}
