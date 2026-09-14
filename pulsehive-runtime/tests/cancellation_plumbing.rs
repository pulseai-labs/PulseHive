//! r1.s2.w1 — cancellation plumbing, behavior-neutral.
//!
//! Proves the token chain lands end to end before any behavior changes:
//! `Task::with_cancel` stores the caller's token, each spawned run gets a
//! child of it, and the `ToolContext.cancel` a tool receives is a child of
//! that run token — so cancelling the task token is observed inside a
//! running tool body. The final `AgentOutcome` is deliberately not
//! asserted: w2 changes it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use pulsehive_core::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
use pulsehive_core::error::Result;
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::LlmConfig;
use pulsehive_core::testing::ScriptedProvider;
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};

/// Tool that reports it started, then waits on its invocation token.
///
/// `started` fires once `execute` is running; the tool then waits for
/// `context.cancel.cancelled()` under a bound and records the outcome in
/// `observed` — true only if the cancellation actually arrived.
struct CancelProbeTool {
    started: Arc<Notify>,
    observed: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for CancelProbeTool {
    fn name(&self) -> &str {
        "cancel_probe"
    }

    fn description(&self) -> &str {
        "Signals it started, then awaits its cancellation token"
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, _params: Value, ctx: &ToolContext) -> Result<ToolResult> {
        self.started.notify_one();
        match tokio::time::timeout(Duration::from_secs(5), ctx.cancel.cancelled()).await {
            Ok(()) => {
                self.observed.store(true, Ordering::SeqCst);
                Ok(ToolResult::text("cancelled"))
            }
            Err(_) => Ok(ToolResult::text("not cancelled")),
        }
    }
}

#[tokio::test]
async fn task_cancel_token_reaches_tool_context() {
    let dir = tempfile::tempdir().expect("tempdir");
    let started = Arc::new(Notify::new());
    let observed = Arc::new(AtomicBool::new(false));

    // One scripted tool call, then a final answer so the turn can end.
    let provider = ScriptedProvider::new()
        .then_tool_call("cancel_probe", json!({}))
        .then_text("done");

    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("cancel-plumbing.db"))
        .llm_provider("scripted", provider)
        .no_insight_synthesizer()
        .build()
        .expect("build HiveMind");

    let agent = AgentDefinition {
        name: "probe-agent".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "Call the probe tool.".into(),
            tools: vec![Arc::new(CancelProbeTool {
                started: started.clone(),
                observed: observed.clone(),
            })],
            lens: Lens::default(),
            llm_config: LlmConfig::new("scripted", "test-model"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    };

    let token = CancellationToken::new();
    let task = Task::new("run the probe").with_cancel(token.clone());

    let mut stream = hive
        .deploy(vec![agent], vec![task])
        .await
        .expect("deploy agents");

    // Cancel only once the tool body is actually awaiting its token —
    // the assertion below proves the signal arrived mid-execution.
    tokio::time::timeout(Duration::from_secs(30), started.notified())
        .await
        .expect("probe tool never started");
    token.cancel();

    // Drain until the run finishes. The outcome variant is w2's business;
    // here it only matters that the run ends.
    let _ = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = stream.next().await {
            if matches!(event, HiveEvent::AgentCompleted { .. }) {
                break;
            }
        }
    })
    .await;

    assert!(
        observed.load(Ordering::SeqCst),
        "tool did not observe the task's cancellation"
    );
}
