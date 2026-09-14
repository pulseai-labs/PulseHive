//! PulseHive Cancellable Tool Example
//!
//! Demonstrates **task-scoped cooperative cancellation** (ADR-014): a caller
//! owns a `tokio_util::sync::CancellationToken`, attaches it to a `Task` with
//! `Task::with_cancel`, and cancelling it stops that task's run — the
//! in-flight provider call aborts, no further tool call starts, and the turn
//! ends `AgentOutcome::Cancelled { partial_response }` carrying the latest
//! assistant text. A second task then runs on the **same HiveMind** to
//! completion, proving the hive is still usable after a cancellation.
//!
//! The demo tool is a parameter sweep that polls `ToolContext.cancel`
//! between steps and returns its partial results — the cooperative shape a
//! long-running tool takes.
//!
//! Offline by default — no API key, no network: the SDK's scripted test
//! provider ([`ScriptedProvider`], behind `pulsehive-core`'s `testing`
//! feature) scripts a turn that calls the sweep tool, then pends on
//! `then_hang` for the follow-up LLM call until the cancel token fires. The
//! second turn uses a *separate* scripted provider, so its script is
//! unaffected by where in the first turn the cancel landed.
//!
//! When `OPENAI_API_KEY` is set (optional `OPENAI_BASE_URL`, `OPENAI_MODEL`,
//! loaded through `dotenvy`), the example runs both turns against a live
//! OpenAI-compatible endpoint instead.
//!
//! ```bash
//! cargo run -p pulsehive-runtime --example cancellable_tool              # press Enter to stop
//! cargo run -p pulsehive-runtime --example cancellable_tool -- --auto-stop-ms 300
//! ```

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

/// Sweep size: 8 steps × 15ms ≈ 120ms — comfortably under a 300ms
/// `--auto-stop-ms`, so the scripted cancel lands on the hanging follow-up
/// LLM call rather than mid-tool.
const SWEEP_STEPS: usize = 8;
const STEP_MS: u64 = 15;

/// Bound on each event drain — a regression that never completes exits
/// non-zero instead of hanging the caller.
const DRAIN_BOUND: Duration = Duration::from_secs(60);

// ── Cooperative tool ────────────────────────────────────────────────────
// Works the sweep in steps and polls `context.cancel` between them. On a
// mid-sweep cancel it returns the results it already produced — the loop
// still delivers that result as `ToolCallCompleted`, and the run ends
// `Cancelled` at the next checkpoint.

struct ParamSweepTool;

#[async_trait]
impl Tool for ParamSweepTool {
    fn name(&self) -> &str {
        "param_sweep"
    }

    fn description(&self) -> &str {
        "Scores a parameter sweep in steps; returns partial results on cancel"
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, _params: Value, ctx: &ToolContext) -> Result<ToolResult> {
        for step in 1..=SWEEP_STEPS {
            if ctx.cancel.is_cancelled() {
                return Ok(ToolResult::text(format!(
                    "partial: {}/{SWEEP_STEPS} steps scored",
                    step - 1
                )));
            }
            tokio::time::sleep(Duration::from_millis(STEP_MS)).await;
        }
        Ok(ToolResult::text(format!(
            "sweep complete: {SWEEP_STEPS} steps scored"
        )))
    }
}

/// An LLM agent with the sweep tool, routing to the named provider.
fn sweep_agent(provider: &str, model: &str) -> AgentDefinition {
    AgentDefinition {
        name: "sweeper".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "Run the param_sweep tool, then report the results.".into(),
            tools: vec![Arc::new(ParamSweepTool)],
            lens: Lens::default(),
            llm_config: LlmConfig::new(provider, model),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    }
}

/// Drains a deploy stream, printing the tool calls started and returning
/// the terminal `AgentOutcome`. `None` means the stream ended first.
async fn drain_turn(
    mut stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
) -> Option<AgentOutcome> {
    while let Some(event) = stream.next().await {
        match event {
            HiveEvent::ToolCallStarted { tool_name, .. } => {
                println!("tool started: {tool_name}");
            }
            HiveEvent::ToolCallCompleted {
                tool_name,
                result_preview,
                ..
            } => {
                println!("tool completed: {tool_name} -> {result_preview}");
            }
            HiveEvent::AgentCompleted { outcome, .. } => return Some(outcome),
            _ => {}
        }
    }
    None
}

async fn drain_or_die(
    stream: Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
    label: &str,
) -> AgentOutcome {
    match tokio::time::timeout(DRAIN_BOUND, drain_turn(stream)).await {
        Ok(Some(outcome)) => outcome,
        Ok(None) => {
            eprintln!("REGRESSION: {label} stream ended before AgentCompleted");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("REGRESSION: {label} drain timed out before AgentCompleted");
            std::process::exit(1);
        }
    }
}

/// Parses `--auto-stop-ms <n>`: the non-interactive cancel trigger. Without
/// it, pressing Enter cancels the task's token.
fn parse_auto_stop_ms() -> Option<u64> {
    let mut auto_stop_ms = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--auto-stop-ms" {
            auto_stop_ms = args
                .next()
                .map(|v| v.parse().expect("--auto-stop-ms takes a millisecond count"));
        }
    }
    auto_stop_ms
}

/// Builds the HiveMind on a temp substrate and wires provider routing.
/// Offline (default): two ScriptedProviders — turn 1 scripts [assistant
/// text + param_sweep call, then a hang], turn 2 gets its own single-reply
/// script so it is robust to where the cancel landed in turn 1. Live
/// (OPENAI_API_KEY set): one real provider serves both turns. Returns the
/// hive plus each turn's provider name and model.
fn build_hive(dir: &tempfile::TempDir) -> (HiveMind, &'static str, &'static str, String) {
    let builder = HiveMind::builder()
        .substrate_path(dir.path().join("cancellable.db"))
        .no_insight_synthesizer();

    match std::env::var("OPENAI_API_KEY") {
        Ok(key) if !key.is_empty() => {
            let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
            let mut config = pulsehive_openai::OpenAIConfig::new(key, model.clone());
            if let Ok(base_url) = std::env::var("OPENAI_BASE_URL") {
                config = config.with_base_url(base_url);
            }
            let hive = builder
                .llm_provider(
                    "live",
                    pulsehive_openai::OpenAICompatibleProvider::new(config),
                )
                .build()
                .expect("build HiveMind");
            println!("mode: live (OPENAI_API_KEY set, model {model})");
            (hive, "live", "live", model)
        }
        _ => {
            let hive = builder
                .llm_provider(
                    "scripted-a",
                    ScriptedProvider::new()
                        .then_response(LlmResponse::new(
                            Some("sweep draft".to_string()),
                            vec![ToolCall {
                                id: "call_1".into(),
                                name: "param_sweep".into(),
                                arguments: json!({}),
                            }],
                            TokenUsage::default(),
                        ))
                        .then_hang(),
                )
                .llm_provider(
                    "scripted-b",
                    ScriptedProvider::new().then_text("sweep summary"),
                )
                .build()
                .expect("build HiveMind");
            println!("mode: offline scripted (set OPENAI_API_KEY for a live endpoint)");
            (hive, "scripted-a", "scripted-b", "demo".to_string())
        }
    }
}

/// Arms the task-token cancel trigger: `--auto-stop-ms` fires it after n
/// ms on a timer task; otherwise a blocking stdin read cancels on Enter.
fn arm_cancel(token: &CancellationToken, auto_stop_ms: Option<u64>) {
    match auto_stop_ms {
        Some(ms) => {
            println!("auto-stop armed at {ms}ms\n");
            let token = token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                token.cancel();
            });
        }
        None => {
            println!("press Enter to stop the turn\n");
            let token = token.clone();
            tokio::task::spawn_blocking(move || {
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                token.cancel();
            });
        }
    }
}

/// Prints the turn outcome — `outcome: cancelled` plus the partial
/// response for the cancelled path the example demonstrates.
fn print_outcome(outcome: &AgentOutcome) {
    match outcome {
        AgentOutcome::Cancelled { partial_response } => {
            println!("outcome: cancelled");
            println!("partial_response: {partial_response}");
        }
        AgentOutcome::Complete { response } => {
            println!("outcome: complete");
            println!("response: {response}");
        }
        other => println!("outcome: {other:?}"),
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let auto_stop_ms = parse_auto_stop_ms();

    let dir = tempfile::tempdir().expect("create tempdir");
    let (hive, turn1_provider, turn2_provider, model) = build_hive(&dir);

    println!("=== Cancellable Tool Example ===");
    println!(
        "Deploying 'sweeper' — it calls param_sweep ({SWEEP_STEPS} steps of {STEP_MS}ms), \
         then pends on the follow-up LLM call"
    );

    let token = CancellationToken::new();
    let task = Task::new("Run the parameter sweep").with_cancel(token.clone());
    let stream = hive
        .deploy(vec![sweep_agent(turn1_provider, &model)], vec![task])
        .await
        .expect("deploy agents");
    arm_cancel(&token, auto_stop_ms);

    let outcome = drain_or_die(stream, "turn 1").await;
    print_outcome(&outcome);

    // The HiveMind is still usable: a second task runs on it to completion.
    println!("\nRunning the next task on the same HiveMind");
    let stream = hive
        .deploy(
            vec![sweep_agent(turn2_provider, &model)],
            vec![Task::new("Summarize the sweep results")],
        )
        .await
        .expect("deploy second task");
    match drain_or_die(stream, "turn 2").await {
        AgentOutcome::Complete { .. } => println!("next turn: complete"),
        other => {
            eprintln!("REGRESSION: expected the next turn to complete, got {other:?}");
            std::process::exit(1);
        }
    }

    hive.shutdown();

    // Force exit: PulseDB's ONNX runtime holds background threads that prevent a
    // clean Tokio runtime shutdown (same known issue as the other examples).
    std::process::exit(0);
}
