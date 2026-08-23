//! Integration test for the agent-loop streaming wiring (WI 1.03).
//!
//! Drives the full `HiveMind::deploy()` → agentic loop → event stream path with a
//! streaming tool that emits `ToolProgress::Progress` over ~1s, and asserts that a
//! subscriber sees the live progress plus the loop-generated `Started`/`Completed`
//! bookends.
//!
//! Broadcast-drain discipline (audit ②): the `deploy()` stream subscribes BEFORE
//! the agent runs and `collect_events` drains promptly; progress-count assertions
//! use `>=`, never `==`, because the `EventBus` is a lossy `tokio::broadcast`.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures_core::Stream;
use serde_json::Value;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::{
    LlmChunk, LlmConfig, LlmProvider, LlmResponse, Message, TokenUsage, ToolCall, ToolDefinition,
};
use pulsehive_core::tool::{StreamingTool, Tool, ToolContext, ToolProgress, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};
use tokio::sync::mpsc;

// ── Mock LLM Provider ────────────────────────────────────────────────────

/// Scripted LLM: returns one tool call for `sleepy_stream`, then a final text.
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
        Err(PulseHiveError::llm("Streaming not used in loop"))
    }
}

// ── Streaming tool that emits progress every ~200ms for ~1s ───────────────

const PROGRESS_STEPS: usize = 5;
const STEP_MS: u64 = 200;

struct SleepyStreamingTool;

#[async_trait]
impl Tool for SleepyStreamingTool {
    fn name(&self) -> &str {
        "sleepy_stream"
    }
    fn description(&self) -> &str {
        "Long-running tool that reports fractional progress"
    }
    fn parameters(&self) -> Value {
        serde_json::json!({"type": "object"})
    }
    // Non-streaming fallback (never taken here — `as_streaming` returns `Some`).
    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        Ok(ToolResult::text("done (non-streaming path)"))
    }
    fn as_streaming(&self) -> Option<&dyn StreamingTool> {
        Some(self)
    }
}

#[async_trait]
impl StreamingTool for SleepyStreamingTool {
    async fn execute_streaming(
        &self,
        _params: Value,
        _context: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult> {
        for step in 1..=PROGRESS_STEPS {
            tokio::time::sleep(Duration::from_millis(STEP_MS)).await;
            // Dropped receiver is a soft signal: ignore send errors, keep going.
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

// ── Helpers ───────────────────────────────────────────────────────────────

fn build_hive(provider: ScriptedLlm) -> HiveMind {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    // Leak the tempdir so its files outlive the test.
    Box::leak(Box::new(dir));

    HiveMind::builder()
        .substrate_path(&path)
        .llm_provider("mock", provider)
        .build()
        .unwrap()
}

fn llm_agent(name: &str, tools: Vec<Arc<dyn Tool>>) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a test agent.".into(),
            tools,
            lens: Lens::default(),
            llm_config: LlmConfig::new("mock", "test-model"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    }
}

/// Drain the event stream promptly until the agent completes (or timeout).
async fn collect_events(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    timeout: Duration,
) -> Vec<HiveEvent> {
    let mut events = vec![];
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        tokio::select! {
            event = stream.next() => {
                match event {
                    Some(e) => {
                        let is_completed = matches!(&e, HiveEvent::AgentCompleted { .. });
                        events.push(e);
                        if is_completed { break; }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => { break; }
        }
    }

    events
}

// ── Test ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn streaming_tool_progress_reaches_subscriber() {
    let provider = ScriptedLlm::new(vec![
        ScriptedLlm::tool_call("call_1", "sleepy_stream", serde_json::json!({})),
        ScriptedLlm::text("All done."),
    ]);
    let hive = build_hive(provider);

    let agent = llm_agent("streamer", vec![Arc::new(SleepyStreamingTool)]);
    let task = Task::new("Run the long streaming tool");

    let t0 = std::time::Instant::now();
    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(10)).await;
    let elapsed_ms = t0.elapsed().as_millis() as u64;

    // Intermediate progress: at least 4 `Progress` events reached the subscriber.
    let progress_count = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                HiveEvent::ToolProgress {
                    progress: ToolProgress::Progress { .. },
                    ..
                }
            )
        })
        .count();
    assert!(
        progress_count >= 4,
        "expected >= 4 ToolProgress::Progress events, got {progress_count}. Events: {events:?}"
    );

    // Loop-generated `Started` bookend present.
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::ToolProgress {
                progress: ToolProgress::Started { .. },
                ..
            }
        )),
        "missing loop-generated ToolProgress::Started bookend. Events: {events:?}"
    );

    // Loop-generated `Completed` bookend present; capture its duration.
    let duration_ms = events
        .iter()
        .find_map(|e| match e {
            HiveEvent::ToolProgress {
                progress: ToolProgress::Completed { duration_ms },
                ..
            } => Some(*duration_ms),
            _ => None,
        })
        .expect("missing loop-generated ToolProgress::Completed bookend");

    // `Completed.duration_ms` reflects the ~1s of streaming work…
    assert!(
        duration_ms >= 800,
        "tool duration {duration_ms}ms should reflect ~1s of streaming work (>= 800ms)"
    );
    // …and is ≈ the loop's wall-clock elapsed (mocks + deploy overhead are small).
    assert!(
        duration_ms <= elapsed_ms + 50,
        "tool duration {duration_ms}ms cannot meaningfully exceed loop elapsed {elapsed_ms}ms"
    );
    assert!(
        elapsed_ms - duration_ms < 2000,
        "tool duration {duration_ms}ms should be ≈ loop elapsed {elapsed_ms}ms"
    );

    // Sanity: the agent completed successfully through the streaming tool call.
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::AgentCompleted {
                outcome: AgentOutcome::Complete { response },
                ..
            } if response == "All done."
        )),
        "expected AgentCompleted(Complete). Events: {events:?}"
    );
}
