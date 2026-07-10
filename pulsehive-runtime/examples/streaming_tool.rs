//! PulseHive Streaming Tool Example
//!
//! Demonstrates a long-running [`StreamingTool`] that reports **live progress**
//! through the agent loop instead of a frozen wait. A scripted (in-file) LLM
//! provider requests the streaming tool once; the tool emits fractional
//! `ToolProgress::Progress` events roughly once per second for ~5s. The agent
//! loop (v2.1.0) forwards each one as a `HiveEvent::ToolProgress` and brackets
//! them with loop-generated `Started` / `Completed` bookends.
//!
//! Hermetic: no API key and no network — the scripted provider drives the real
//! `perceive → think → act → record` loop via `HiveMind::deploy()`. The event
//! stream is drained **concurrently** with the tool run (the agent executes as a
//! spawned Tokio task), so a burst of progress isn't lapped by the lossy
//! broadcast `EventBus`.
//!
//! This example is a **self-asserting gate**: it counts the `ToolProgress`
//! events it observes and exits non-zero unless it sees >= 5 of them ending with
//! the loop-generated `Completed` bookend. Run with:
//! ```bash
//! cargo run -p pulsehive-runtime --example streaming_tool
//! ```

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures_core::Stream;
use serde_json::Value;

use pulsehive_core::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::{
    LlmChunk, LlmConfig, LlmProvider, LlmResponse, Message, TokenUsage, ToolCall, ToolDefinition,
};
use pulsehive_core::tool::{StreamingTool, Tool, ToolContext, ToolProgress, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};
use tokio::sync::mpsc;

/// Number of `Progress` events the streaming tool emits. `>= 5` satisfies the
/// slice's `auto:` demo criterion with margin against the lossy broadcast.
const PROGRESS_STEPS: usize = 6;

/// Delay between progress emissions. `< 1000ms` keeps the rate at `>= 1
/// Progress` per second across ~5s of simulated work.
const STEP_MS: u64 = 800;

// ── Scripted LLM ────────────────────────────────────────────────────────
// Requests the streaming tool once, then returns a final text answer. No API
// key, no network — same in-file provider pattern as `custom_tool.rs`.

struct ScriptedLlm {
    responses: Mutex<Vec<LlmResponse>>,
}

impl ScriptedLlm {
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

    fn tool_call(id: &str, name: &str, args: Value) -> LlmResponse {
        LlmResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: name.into(),
                arguments: args,
            }],
            usage: TokenUsage::default(),
        }
    }
}

#[async_trait]
impl LlmProvider for ScriptedLlm {
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
        Err(PulseHiveError::llm(
            "LLM token streaming is not used in this example",
        ))
    }
}

// ── Streaming tool ──────────────────────────────────────────────────────
// Emits a `Progress` event every ~800ms for ~5s. The `Started` / `Completed`
// bookends are emitted by the agent loop, not here.

struct ProgressStreamTool;

#[async_trait]
impl Tool for ProgressStreamTool {
    fn name(&self) -> &str {
        "progress_stream"
    }

    fn description(&self) -> &str {
        "Long-running tool that reports live fractional progress"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({ "type": "object" })
    }

    // Non-streaming fallback (never taken here — `as_streaming` returns `Some`).
    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        Ok(ToolResult::text("done (non-streaming fallback)"))
    }

    fn as_streaming(&self) -> Option<&dyn StreamingTool> {
        Some(self)
    }
}

#[async_trait]
impl StreamingTool for ProgressStreamTool {
    async fn execute_streaming(
        &self,
        _params: Value,
        _context: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult> {
        for step in 1..=PROGRESS_STEPS {
            tokio::time::sleep(Duration::from_millis(STEP_MS)).await;
            // A dropped receiver is a soft signal: ignore send errors and keep
            // computing, returning the final result regardless.
            let _ = progress_tx
                .send(ToolProgress::Progress {
                    fraction: step as f32 / PROGRESS_STEPS as f32,
                    message: Some(format!("step {step}/{PROGRESS_STEPS}")),
                })
                .await;
        }
        Ok(ToolResult::text("stream complete"))
    }
}

#[tokio::main]
async fn main() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("streaming.db"))
        .llm_provider(
            "mock",
            ScriptedLlm::new(vec![
                ScriptedLlm::tool_call("call_1", "progress_stream", serde_json::json!({})),
                ScriptedLlm::text("Streaming tool finished."),
            ]),
        )
        .build()
        .expect("build HiveMind");

    let agent = AgentDefinition {
        name: "streamer".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You run a long streaming tool and report its progress.".into(),
            tools: vec![Arc::new(ProgressStreamTool)],
            lens: Lens::default(),
            llm_config: LlmConfig::new("mock", "demo"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    };

    println!("=== Streaming Tool Example ===");
    println!(
        "Deploying agent 'streamer' with a streaming tool (emits {PROGRESS_STEPS} progress events \
         over ~5s)\n"
    );

    let mut stream = hive
        .deploy(vec![agent], vec![Task::new("Run the streaming tool")])
        .await
        .expect("deploy agents");

    // Drain the event stream CONCURRENTLY with the tool run: `deploy()` spawns
    // the agent as a Tokio task and returns immediately, so polling the returned
    // stream observes progress live (not collect-after). Bounded by a timeout so
    // a regression that never completes can't hang CI.
    let drain = async {
        let mut total = 0usize;
        let mut progress = 0usize;
        // Track the LAST ToolProgress kind so the gate can require the envelope to
        // actually *end* with Completed (not merely to have seen a Completed
        // somewhere). `None` until the first ToolProgress arrives.
        let mut last_kind: Option<&'static str> = None;

        while let Some(event) = stream.next().await {
            match event {
                HiveEvent::ToolProgress {
                    tool_name,
                    progress: payload,
                    ..
                } => {
                    total += 1;
                    match payload {
                        ToolProgress::Started { .. } => {
                            last_kind = Some("started");
                            println!("  [{tool_name}] ToolProgress::Started");
                        }
                        ToolProgress::Progress { fraction, message } => {
                            progress += 1;
                            last_kind = Some("progress");
                            let label = message.unwrap_or_default();
                            println!(
                                "  [{tool_name}] ToolProgress::Progress {:.0}% {label}",
                                fraction * 100.0
                            );
                        }
                        ToolProgress::Completed { duration_ms } => {
                            last_kind = Some("completed");
                            println!("  [{tool_name}] ToolProgress::Completed ({duration_ms}ms)");
                        }
                        other => {
                            last_kind = Some("other");
                            println!("  [{tool_name}] {other:?}");
                        }
                    }
                }
                // `HiveEvent` is `#[non_exhaustive]` (v2.1.0), so an external
                // exhaustive match needs this catch-all arm.
                HiveEvent::AgentCompleted { .. } => break,
                _ => {}
            }
        }

        (total, progress, last_kind)
    };

    let (total, progress, last_kind) =
        match tokio::time::timeout(Duration::from_secs(60), drain).await {
            Ok(counts) => counts,
            Err(_) => {
                eprintln!("REGRESSION: event drain timed out before AgentCompleted");
                std::process::exit(1);
            }
        };

    println!(
        "\nObserved {total} ToolProgress events ({progress} Progress), last_kind={last_kind:?}"
    );

    // Self-assert — this makes `cargo run --example streaming_tool` a real gate.
    // Assert on the *Progress* count (the user-visible live updates), not the total
    // (which Started/Completed bookends would pad to 5), and require the envelope to
    // END with the loop-generated Completed bookend — so a regression to fewer live
    // updates, or a Completed that isn't terminal, fails loudly.
    if progress < 5 || last_kind != Some("completed") {
        eprintln!(
            "REGRESSION: expected >= 5 live Progress updates ending with Completed, \
             got total={total} progress={progress} last_kind={last_kind:?}"
        );
        std::process::exit(1);
    }

    println!("SELF-ASSERT PASSED: >= 5 live Progress updates observed, ending with Completed.");

    hive.shutdown();

    // Force exit: PulseDB's ONNX runtime holds background threads that prevent a
    // clean Tokio runtime shutdown (same known issue as the custom_tool example).
    // NOTE: because this bypasses destructors, this example gates *event
    // observation* (the streaming envelope reaching a subscriber), NOT task-lifecycle
    // correctness — it is not evidence that the forwarder task, exporter, or agent
    // tasks shut down cleanly. Lifecycle/leak checks belong in the runtime test suite.
    std::process::exit(0);
}
