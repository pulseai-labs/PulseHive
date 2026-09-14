//! r1.s2.w5 — adversarial streaming tools cannot wedge an agent turn
//! (PulseHive #37 G1/G2/G4/G6).
//!
//! Every test drives the real path (`HiveMind::deploy` + `ScriptedProvider`),
//! and every wait is bounded by `BOUND` so a regression fails the test instead
//! of hanging it. No fakes: the provider is the product-owned
//! `ScriptedProvider`, the tool probes are ordinary `Tool`/`StreamingTool`
//! impls.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::LlmConfig;
use pulsehive_core::testing::ScriptedProvider;
use pulsehive_core::tool::{StreamingTool, Tool, ToolContext, ToolProgress, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};

/// Bound on every wait — a hung turn fails the test rather than hanging CI.
const BOUND: Duration = Duration::from_secs(30);

/// Builds a HiveMind on a temp substrate with the scripted provider registered
/// and insight synthesis off (a synthesizer would consume scripted steps).
fn scripted_hive(dir: &tempfile::TempDir, provider: ScriptedProvider) -> HiveMind {
    HiveMind::builder()
        .substrate_path(dir.path().join("streaming-adversarial.db"))
        .llm_provider("scripted", provider)
        .no_insight_synthesizer()
        .build()
        .expect("build HiveMind")
}

/// An LLM agent definition routing to the `scripted` provider.
fn scripted_agent(tools: Vec<Arc<dyn Tool>>, llm_config: LlmConfig) -> AgentDefinition {
    AgentDefinition {
        name: "adversarial-agent".into(),
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

/// `ToolProgress` events for `tool` in a drained set, in arrival order.
fn progress_events<'a>(events: &'a [HiveEvent], tool: &str) -> Vec<&'a ToolProgress> {
    events
        .iter()
        .filter_map(|event| match event {
            HiveEvent::ToolProgress {
                tool_name, progress, ..
            } if tool_name == tool => Some(progress),
            _ => None,
        })
        .collect()
}

/// A streaming tool that floods the progress channel with `events` progress
/// updates, counting how many `send().await` calls fail.
struct FloodTool {
    tool_name: &'static str,
    events: usize,
    send_failures: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for FloodTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Floods the progress channel"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        Ok(ToolResult::text("flood done"))
    }

    fn as_streaming(&self) -> Option<&dyn StreamingTool> {
        Some(self)
    }
}

#[async_trait]
impl StreamingTool for FloodTool {
    async fn execute_streaming(
        &self,
        _params: Value,
        _ctx: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult> {
        for i in 0..self.events {
            if progress_tx
                .send(ToolProgress::Progress {
                    fraction: i as f32 / self.events as f32,
                    message: None,
                })
                .await
                .is_err()
            {
                self.send_failures.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(ToolResult::text("flood done"))
    }
}

#[tokio::test]
async fn flood_of_progress_events_completes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let send_failures = Arc::new(AtomicUsize::new(0));
    let provider = ScriptedProvider::new()
        .then_tool_call("flood", json!({}))
        .then_text("survived the flood");
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![Arc::new(FloodTool {
            tool_name: "flood",
            events: 1_000,
            send_failures: send_failures.clone(),
        })],
        LlmConfig::new("scripted", "test-model"),
    );

    let stream = hive
        .deploy(vec![agent], vec![Task::new("flood the channel")])
        .await
        .expect("deploy agents");
    let events = drain_until_completed(stream).await;

    // Tool->loop delivery is guaranteed with backpressure: a full 64-event
    // buffer makes `send().await` wait, never error — so every one of the
    // 1,000 sends completed. The `deploy()` broadcast downstream is
    // separately lossy (a lagging subscriber's events are dropped), so
    // per-event arrival is asserted only over what the stream yielded.
    assert_eq!(
        send_failures.load(Ordering::SeqCst),
        0,
        "progress sends failed on the tool->loop channel"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, HiveEvent::ToolCallCompleted { tool_name, .. } if tool_name == "flood")
        ),
        "flood's ToolCallCompleted was not emitted"
    );
    match completed_outcome(&events) {
        AgentOutcome::Complete { response } => assert_eq!(response, "survived the flood"),
        other => panic!("expected AgentOutcome::Complete, got {other:?}"),
    }

    // Ordering holds among the progress events the stream actually delivered.
    let fractions: Vec<f32> = progress_events(&events, "flood")
        .into_iter()
        .filter_map(|progress| match progress {
            ToolProgress::Progress { fraction, .. } => Some(*fraction),
            _ => None,
        })
        .collect();
    assert!(
        !fractions.is_empty(),
        "no progress events reached the deploy stream at all"
    );
    assert!(
        fractions.windows(2).all(|w| w[0] <= w[1]),
        "progress events arrived out of order"
    );
}

/// A streaming tool that clones its progress sender into a task that never
/// drops it — the channel stays open after `execute_streaming` returns, so a
/// naive `forwarder.await` would hang the turn forever (G1).
struct LeakySenderTool {
    tool_name: &'static str,
}

#[async_trait]
impl Tool for LeakySenderTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Leaks its progress sender into a background task"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        Ok(ToolResult::text("leaky done"))
    }

    fn as_streaming(&self) -> Option<&dyn StreamingTool> {
        Some(self)
    }
}

#[async_trait]
impl StreamingTool for LeakySenderTool {
    async fn execute_streaming(
        &self,
        _params: Value,
        _ctx: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult> {
        let leaked = progress_tx.clone();
        // The clone is deliberately never dropped: the spawned task pends
        // forever holding it, keeping the channel open past the tool's return.
        tokio::spawn(async move {
            let _leaked = leaked;
            std::future::pending::<()>().await;
        });
        let _ = progress_tx
            .send(ToolProgress::Progress {
                fraction: 0.5,
                message: Some("halfway, about to leak".into()),
            })
            .await;
        Ok(ToolResult::text("leaky done"))
    }
}

#[tokio::test]
async fn leaked_progress_sender_fails_loudly_not_hang() {
    // Real-time test (not paused): the runtime's forwarder drain grace is 5s
    // and `BroadcastStream` self-wakes when the bus is empty, which would keep
    // a paused clock from ever advancing. 30s BOUND comfortably covers the
    // grace.
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new()
        .then_tool_call("leaky", json!({}))
        .then_text("survived the leak");
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![Arc::new(LeakySenderTool { tool_name: "leaky" })],
        LlmConfig::new("scripted", "test-model"),
    );

    let stream = hive
        .deploy(vec![agent], vec![Task::new("leak the sender")])
        .await
        .expect("deploy agents");
    let events = drain_until_completed(stream).await;

    // The bounded drain aborts the still-open forwarder after its grace: the
    // `Completed` bookend and `ToolCallCompleted` are still emitted (the
    // failure is loud, not silent) and the turn completes.
    let progress = progress_events(&events, "leaky");
    assert!(
        progress
            .iter()
            .any(|p| matches!(p, ToolProgress::Completed { .. })),
        "Completed bookend missing after the drain grace"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, HiveEvent::ToolCallCompleted { tool_name, .. } if tool_name == "leaky")
        ),
        "leaky's ToolCallCompleted was not emitted"
    );
    match completed_outcome(&events) {
        AgentOutcome::Complete { response } => assert_eq!(response, "survived the leak"),
        other => panic!("expected AgentOutcome::Complete, got {other:?}"),
    }
}

/// A streaming tool that violates the bookend contract: it sends `Started`
/// and `Completed` itself even though the loop emits those (the forwarder
/// must drop them so a call keeps exactly one runtime-emitted pair).
struct BookendTool {
    tool_name: &'static str,
}

#[async_trait]
impl Tool for BookendTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Sends forbidden Started/Completed bookends"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        Ok(ToolResult::text("bookends done"))
    }

    fn as_streaming(&self) -> Option<&dyn StreamingTool> {
        Some(self)
    }
}

#[async_trait]
impl StreamingTool for BookendTool {
    async fn execute_streaming(
        &self,
        _params: Value,
        _ctx: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult> {
        let _ = progress_tx
            .send(ToolProgress::Started {
                estimated_duration_ms: Some(1),
            })
            .await;
        let _ = progress_tx
            .send(ToolProgress::Progress {
                fraction: 0.5,
                message: None,
            })
            .await;
        let _ = progress_tx
            .send(ToolProgress::Completed { duration_ms: 0 })
            .await;
        Ok(ToolResult::text("bookends done"))
    }
}

#[tokio::test]
async fn forbidden_bookends_are_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new()
        .then_tool_call("bookends", json!({}))
        .then_text("survived the bookends");
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![Arc::new(BookendTool {
            tool_name: "bookends",
        })],
        LlmConfig::new("scripted", "test-model"),
    );

    let stream = hive
        .deploy(vec![agent], vec![Task::new("send forbidden bookends")])
        .await
        .expect("deploy agents");
    let events = drain_until_completed(stream).await;

    // Exactly one Started and one Completed per call — the loop's own pair.
    // The tool-sent duplicates must have been dropped by the forwarder.
    let progress = progress_events(&events, "bookends");
    let started = progress
        .iter()
        .filter(|p| matches!(p, ToolProgress::Started { .. }))
        .count();
    let completed = progress
        .iter()
        .filter(|p| matches!(p, ToolProgress::Completed { .. }))
        .count();
    assert_eq!(started, 1, "expected exactly one Started bookend");
    assert_eq!(completed, 1, "expected exactly one Completed bookend");
    match completed_outcome(&events) {
        AgentOutcome::Complete { response } => assert_eq!(response, "survived the bookends"),
        other => panic!("expected AgentOutcome::Complete, got {other:?}"),
    }
}

/// A streaming tool that emits progress, then fails with an ordinary `Err`.
struct ErrorTool {
    tool_name: &'static str,
}

#[async_trait]
impl Tool for ErrorTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Emits progress then returns an error"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        Err(PulseHiveError::tool("boom"))
    }

    fn as_streaming(&self) -> Option<&dyn StreamingTool> {
        Some(self)
    }
}

#[async_trait]
impl StreamingTool for ErrorTool {
    async fn execute_streaming(
        &self,
        _params: Value,
        _ctx: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult> {
        let _ = progress_tx
            .send(ToolProgress::Progress {
                fraction: 0.5,
                message: Some("about to fail".into()),
            })
            .await;
        Err(PulseHiveError::tool("stream exploded"))
    }
}

#[tokio::test]
async fn error_after_progress_keeps_bookends() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new()
        .then_tool_call("fails", json!({}))
        .then_text("survived the error");
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![Arc::new(ErrorTool { tool_name: "fails" })],
        LlmConfig::new("scripted", "test-model"),
    );

    let stream = hive
        .deploy(vec![agent], vec![Task::new("fail after progress")])
        .await
        .expect("deploy agents");
    let events = drain_until_completed(stream).await;

    // An ordinary tool error keeps the full envelope: the Completed bookend
    // and a ToolCallCompleted carrying the error are still emitted, and the
    // turn continues to its scripted answer.
    let progress = progress_events(&events, "fails");
    assert!(
        progress
            .iter()
            .any(|p| matches!(p, ToolProgress::Completed { .. })),
        "Completed bookend missing after a tool error"
    );
    let completion = events.iter().find_map(|event| match event {
        HiveEvent::ToolCallCompleted {
            tool_name,
            result_preview,
            ..
        } if tool_name == "fails" => Some(result_preview),
        _ => None,
    });
    let preview = completion.expect("fails' ToolCallCompleted was not emitted");
    assert!(
        preview.contains("stream exploded"),
        "ToolCallCompleted preview does not carry the tool error: {preview}"
    );
    match completed_outcome(&events) {
        AgentOutcome::Complete { response } => assert_eq!(response, "survived the error"),
        other => panic!("expected AgentOutcome::Complete, got {other:?}"),
    }
}

/// A streaming tool that emits progress, then panics mid-body (G4).
struct PanicTool {
    tool_name: &'static str,
}

#[async_trait]
impl Tool for PanicTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Emits progress then panics"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        panic!("plain panic");
    }

    fn as_streaming(&self) -> Option<&dyn StreamingTool> {
        Some(self)
    }
}

#[async_trait]
impl StreamingTool for PanicTool {
    async fn execute_streaming(
        &self,
        _params: Value,
        _ctx: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult> {
        let _ = progress_tx
            .send(ToolProgress::Progress {
                fraction: 0.5,
                message: Some("about to panic".into()),
            })
            .await;
        panic!("adversarial panic");
    }
}

#[tokio::test]
async fn panic_after_progress_becomes_tool_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = ScriptedProvider::new()
        .then_tool_call("panicky", json!({}))
        .then_text("survived the panic");
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![Arc::new(PanicTool { tool_name: "panicky" })],
        LlmConfig::new("scripted", "test-model"),
    );

    let stream = hive
        .deploy(vec![agent], vec![Task::new("panic after progress")])
        .await
        .expect("deploy agents");
    let events = drain_until_completed(stream).await;

    // A tool panic becomes an ordinary tool error (ADR-007: fail loudly,
    // never silently): `ToolCallCompleted` carries `tool '<name>' panicked:
    // <message>` and the turn still reaches `AgentCompleted`.
    let completion = events.iter().find_map(|event| match event {
        HiveEvent::ToolCallCompleted {
            tool_name,
            result_preview,
            ..
        } if tool_name == "panicky" => Some(result_preview),
        _ => None,
    });
    let preview = completion.expect("panicky's ToolCallCompleted was not emitted");
    assert!(
        preview.contains("panicked"),
        "ToolCallCompleted preview does not report the panic: {preview}"
    );
    assert!(
        preview.contains("adversarial panic"),
        "ToolCallCompleted preview lost the panic message: {preview}"
    );
    match completed_outcome(&events) {
        AgentOutcome::Complete { response } => assert_eq!(response, "survived the panic"),
        other => panic!("expected AgentOutcome::Complete, got {other:?}"),
    }
}

/// A streaming tool that emits progress and signals its own completion —
/// the side effect a test observes after dropping the `deploy()` stream.
struct SignalTool {
    tool_name: &'static str,
    done: Arc<Notify>,
}

#[async_trait]
impl Tool for SignalTool {
    fn name(&self) -> &str {
        self.tool_name
    }

    fn description(&self) -> &str {
        "Emits progress, then signals completion"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        Ok(ToolResult::text("signal done"))
    }

    fn as_streaming(&self) -> Option<&dyn StreamingTool> {
        Some(self)
    }
}

#[async_trait]
impl StreamingTool for SignalTool {
    async fn execute_streaming(
        &self,
        _params: Value,
        _ctx: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult> {
        let _ = progress_tx
            .send(ToolProgress::Progress {
                fraction: 0.25,
                message: Some("first".into()),
            })
            .await;
        let _ = progress_tx
            .send(ToolProgress::Progress {
                fraction: 1.0,
                message: Some("last".into()),
            })
            .await;
        self.done.notify_one();
        Ok(ToolResult::text("signal done"))
    }
}

#[tokio::test]
async fn dropped_deploy_stream_does_not_wedge_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let done = Arc::new(Notify::new());
    let provider = ScriptedProvider::new()
        .then_tool_call("signaller", json!({}))
        .then_text("finished unseen");
    let provider_probe = provider.clone();
    let hive = scripted_hive(&dir, provider);
    let agent = scripted_agent(
        vec![Arc::new(SignalTool {
            tool_name: "signaller",
            done: done.clone(),
        })],
        LlmConfig::new("scripted", "test-model"),
    );

    let mut stream = hive
        .deploy(vec![agent], vec![Task::new("drop the stream mid-run")])
        .await
        .expect("deploy agents");

    // Consume the stream only until the tool's first progress event arrives,
    // then drop it — the consumer is gone mid-run. `deploy()`'s broadcast is
    // fire-and-forget, so the run must continue unaffected.
    tokio::time::timeout(BOUND, async {
        while let Some(event) = stream.next().await {
            if matches!(
                event,
                HiveEvent::ToolProgress {
                    tool_name,
                    progress: ToolProgress::Progress { .. },
                    ..
                } if tool_name == "signaller"
            ) {
                break;
            }
        }
    })
    .await
    .expect("never saw the first progress event");
    drop(stream);

    // The run finishes on its own: the tool's completion side effect fires,
    // and the second scripted provider call is consumed — the loop reached
    // its terminal LLM call. A wedged or panicked run fails both waits.
    tokio::time::timeout(BOUND, done.notified())
        .await
        .expect("tool never completed after the stream was dropped");
    tokio::time::timeout(BOUND, async {
        while provider_probe.requests().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("run never reached its terminal LLM call after the stream was dropped");
}
