//! Agentic loop engine — the Perceive→Think→Act→Record cycle.
//!
//! This module implements the core execution loop for LLM agents. Each agent
//! runs through: perceive substrate → think via LLM → act on tool calls → record experiences.
//!
//! The loop is driven by [`run_agentic_loop`], called from `HiveMind::deploy()`.
//!
//! ## Cancellation (ADR-014)
//!
//! [`LoopContext::cancel`] is the run's cancellation token. The loop checks it
//! before every LLM call and before every tool call: a cancelled token ends
//! the turn as [`AgentOutcome::Cancelled`] carrying the latest assistant text
//! as `partial_response`, with no `LlmCallStarted` or `ToolCallStarted` for
//! work that never began. Each provider call also carries a child of the run
//! token on its `LlmConfig.cancel` — bridged to the agent's own
//! `LlmConfig.cancel` when one is set, so either token aborts the in-flight
//! request — and a provider `LlmTransport` error of kind `Cancelled` maps to
//! the same outcome. Cancellation is cooperative: a tool already executing is
//! always awaited, never force-aborted.

use std::any::Any;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt;
use pulsedb::SubstrateProvider;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use pulsehive_core::agent::{AgentOutcome, ExperienceExtractor, LlmAgentConfig};
use pulsehive_core::approval::{ApprovalHandler, ApprovalResult, PendingAction};
use pulsehive_core::error::PulseHiveError;
use pulsehive_core::event::{EventEmitter, HiveEvent};
use pulsehive_core::ids::CollectiveId;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::{
    LlmConfig, LlmErrorKind, LlmProvider, Message, ToolCall, ToolDefinition,
};
use pulsehive_core::tool::{StreamingTool, Tool, ToolContext, ToolProgress, ToolResult};

use crate::hivemind::Task;
use crate::substrate_ids;

/// Default maximum iterations for the agentic loop.
pub const DEFAULT_MAX_ITERATIONS: usize = 25;

/// Runtime context for the agentic loop, grouping shared resources.
pub struct LoopContext<'a> {
    pub agent_id: String,
    pub task: &'a Task,
    pub provider: Arc<dyn LlmProvider>,
    pub substrate: Arc<dyn SubstrateProvider>,
    pub approval_handler: &'a dyn ApprovalHandler,
    pub event_emitter: EventEmitter,
    pub max_iterations: usize,
    /// Optional embedding provider for computing embeddings before storage.
    pub embedding_provider: Option<Arc<dyn pulsehive_core::embedding::EmbeddingProvider>>,
    /// This run's cancellation token (ADR-014). `think_act_loop` checks it
    /// before each LLM call and each tool call — a cancelled token ends the
    /// turn as `AgentOutcome::Cancelled` with the latest assistant text as
    /// `partial_response` — and each provider call carries a child token on
    /// its `LlmConfig.cancel` so an in-flight request aborts. Each tool
    /// invocation still receives its own child via `ToolContext.cancel`.
    pub cancel: CancellationToken,
}

/// Run the agentic loop for a single LLM agent.
///
/// Executes the Perceive→Think→Act→Record cycle until the LLM produces
/// a final response (no tool calls) or `max_iterations` is reached.
pub async fn run_agentic_loop(config: LlmAgentConfig, ctx: LoopContext<'_>) -> AgentOutcome {
    let LlmAgentConfig {
        system_prompt,
        tools,
        lens,
        llm_config,
        experience_extractor,
        refresh_every_n_tool_calls,
    } = config;

    // Build tool lookup map and definitions for LLM
    let tool_map: HashMap<&str, &dyn Tool> = tools
        .iter()
        .map(|t| (t.name(), t.as_ref() as &dyn Tool))
        .collect();
    let tool_defs: Vec<ToolDefinition> = tools
        .iter()
        .map(|t| ToolDefinition::from_tool(t.as_ref()))
        .collect();

    // 1. PERCEIVE — query substrate through lens
    let context_messages = perceive(
        ctx.substrate.as_ref(),
        &lens,
        ctx.task,
        &ctx.event_emitter,
        &ctx.agent_id,
    )
    .instrument(tracing::info_span!("perceive", agent_id = %ctx.agent_id))
    .await;

    // 2. Build initial conversation
    let mut messages: Vec<Message> = Vec::new();
    messages.push(Message::system(&system_prompt));
    messages.extend(context_messages);
    messages.push(Message::user(&ctx.task.description));

    // 3. THINK → ACT loop (with optional mid-task refresh)
    let outcome = think_act_loop(
        &ctx.agent_id,
        &mut messages,
        &tool_map,
        &tool_defs,
        &llm_config,
        &ctx,
        &lens,
        refresh_every_n_tool_calls,
    )
    .await;

    // 4. RECORD — extract experiences and store in substrate
    record(&messages, &outcome, &ctx, experience_extractor.as_deref())
        .instrument(tracing::info_span!("record", agent_id = %ctx.agent_id))
        .await;

    outcome
}

/// Aborts a caller-token bridge task on drop — the bridge must not outlive
/// the provider call it serves, whichever path the call exits on (early
/// return, error arm, or panic).
struct CancelBridge(Option<tokio::task::JoinHandle<()>>);

impl CancelBridge {
    /// When the agent's `LlmConfig` carries its own `cancel` token, spawn the
    /// task that cancels `call_token` when the caller's token fires, so EITHER
    /// token aborts the in-flight call (ADR-014 A15 — the caller's token is
    /// never dropped or replaced by the loop's). `None` when the config has no
    /// caller token: nothing to bridge. A caller token that is ALREADY
    /// cancelled cancels `call_token` synchronously instead — a spawned task
    /// is only polled after the call is underway, so a fast provider would
    /// inspect a still-live token and complete before the bridge ever ran.
    fn spawn(caller: Option<CancellationToken>, call_token: CancellationToken) -> Self {
        let Some(caller) = caller else {
            return Self(None);
        };
        if caller.is_cancelled() {
            call_token.cancel();
            return Self(None);
        }
        Self(Some(tokio::spawn(async move {
            caller.cancelled().await;
            call_token.cancel();
        })))
    }
}

impl Drop for CancelBridge {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// The core Think→Act loop. Returns when LLM produces a final response or max iterations hit.
///
/// When `refresh_every` is `Some(n)`, re-runs the Perceive phase every `n` tool calls,
/// appending fresh context to the conversation. This enables parallel agents to perceive
/// each other's experiences mid-task.
#[allow(clippy::too_many_arguments)]
async fn think_act_loop(
    agent_id: &str,
    messages: &mut Vec<Message>,
    tool_map: &HashMap<&str, &dyn Tool>,
    tool_defs: &[ToolDefinition],
    llm_config: &LlmConfig,
    ctx: &LoopContext<'_>,
    lens: &Lens,
    refresh_every: Option<usize>,
) -> AgentOutcome {
    let mut tool_calls_since_refresh: usize = 0;
    // Latest assistant text produced this turn — the `partial_response` a
    // `Cancelled` outcome carries; empty until the provider produces any.
    let mut partial_response = String::new();

    for iteration in 1..=ctx.max_iterations {
        // Cancellation checkpoint (ADR-014): a cancelled run token ends the
        // turn before the next LLM call — no `LlmCallStarted`, no request sent.
        if ctx.cancel.is_cancelled() {
            tracing::info!(agent_id = %agent_id, "Run cancelled before LLM call");
            return AgentOutcome::Cancelled { partial_response };
        }

        let think_span = tracing::info_span!(
            "think",
            agent_id = %agent_id,
            iteration,
            model = %llm_config.model,
            message_count = messages.len(),
        );

        // ── THINK: call LLM ──────────────────────────────────────────
        ctx.event_emitter.emit(HiveEvent::LlmCallStarted {
            timestamp_ms: pulsehive_core::event::now_ms(),
            agent_id: agent_id.to_string(),
            model: llm_config.model.clone(),
            message_count: messages.len(),
        });

        // Per-call cancellation (ADR-014 A2): every provider call carries a
        // child of the run token on its `LlmConfig`, so cancelling the run
        // aborts the in-flight request. When the agent definition already
        // carries its own `LlmConfig.cancel`, the bridge cancels the call
        // token when the caller's fires — either token aborts the call (A15).
        // The guard aborts the bridge task on every exit path.
        let mut call_config = llm_config.clone();
        let call_token = ctx.cancel.child_token();
        let _cancel_bridge = CancelBridge::spawn(call_config.cancel.take(), call_token.clone());
        call_config.cancel = Some(call_token);

        let start = Instant::now();
        let response = ctx
            .provider
            .chat(messages.clone(), tool_defs.to_vec(), &call_config)
            .instrument(think_span)
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let (input_tokens, output_tokens) = match &response {
            Ok(r) => (r.usage.input_tokens, r.usage.output_tokens),
            Err(_) => (0, 0),
        };
        ctx.event_emitter.emit(HiveEvent::LlmCallCompleted {
            timestamp_ms: pulsehive_core::event::now_ms(),
            agent_id: agent_id.to_string(),
            model: llm_config.model.clone(),
            duration_ms,
            input_tokens,
            output_tokens,
        });

        let response = match response {
            Ok(r) => r,
            // A provider-side cancellation (ADR-011's `LlmTransport` with kind
            // `Cancelled`) ends the turn as `Cancelled`, not `Error`. The call
            // follows the existing provider-error event path — `LlmCallCompleted`
            // was already emitted above exactly as for any other failure.
            Err(PulseHiveError::LlmTransport(e)) if e.kind == LlmErrorKind::Cancelled => {
                tracing::info!(agent_id = %agent_id, "Provider call cancelled");
                return AgentOutcome::Cancelled { partial_response };
            }
            Err(e) => {
                tracing::error!(agent_id = %agent_id, error = %e, "LLM call failed");
                return AgentOutcome::Error {
                    error: e.to_string(),
                };
            }
        };

        // Track the latest assistant text — a later `Cancelled` carries it as
        // `partial_response`. A `Some("")` (providers emit empty content
        // beside tool calls) is not text: it must not erase the last real
        // partial.
        if let Some(text) = response.content.clone() {
            if !text.is_empty() {
                partial_response = text;
            }
        }

        // ── ACT: handle response ─────────────────────────────────────
        if response.tool_calls.is_empty() {
            let content = response.content.unwrap_or_default();
            tracing::debug!(agent_id = %agent_id, "Final response received");
            messages.push(Message::assistant(&content));
            return AgentOutcome::Complete { response: content };
        }

        tracing::debug!(
            agent_id = %agent_id,
            tool_count = response.tool_calls.len(),
            "Tool calls received"
        );

        messages.push(Message::assistant_with_tool_calls(
            response.tool_calls.clone(),
        ));

        for tool_call in &response.tool_calls {
            // Cancellation checkpoint (ADR-014 A7): a cancelled run token
            // means no further tool call starts — no `ToolCallStarted`, no
            // execute. A tool already running is awaited above, never
            // force-aborted (A3).
            if ctx.cancel.is_cancelled() {
                tracing::info!(
                    agent_id = %agent_id,
                    tool = %tool_call.name,
                    "Run cancelled before tool call"
                );
                return AgentOutcome::Cancelled { partial_response };
            }

            let result = execute_tool_call(
                agent_id,
                tool_call,
                tool_map,
                &ctx.substrate,
                ctx.approval_handler,
                &ctx.event_emitter,
                &ctx.task.collective_id,
                &ctx.cancel,
            )
            .instrument(tracing::info_span!("act", agent_id = %agent_id, tool = %tool_call.name))
            .await;

            messages.push(Message::tool_result(&tool_call.id, result.to_content()));
            tool_calls_since_refresh += 1;
        }

        // ── MID-TASK REFRESH: re-perceive substrate if threshold reached ──
        if let Some(interval) = refresh_every {
            if tool_calls_since_refresh >= interval {
                tracing::info!(
                    agent_id = %agent_id,
                    tool_calls = tool_calls_since_refresh,
                    "Mid-task substrate refresh"
                );
                let refreshed = perceive(
                    ctx.substrate.as_ref(),
                    lens,
                    ctx.task,
                    &ctx.event_emitter,
                    agent_id,
                )
                .instrument(tracing::info_span!("perceive", agent_id = %agent_id, refresh = true))
                .await;
                messages.extend(refreshed);
                tool_calls_since_refresh = 0;
            }
        }
    }

    tracing::warn!(agent_id = %agent_id, max = ctx.max_iterations, "Max iterations reached");
    AgentOutcome::MaxIterationsReached
}

/// Execute a single tool call with approval check.
#[allow(clippy::too_many_arguments)]
async fn execute_tool_call(
    agent_id: &str,
    tool_call: &ToolCall,
    tool_map: &HashMap<&str, &dyn Tool>,
    substrate: &Arc<dyn SubstrateProvider>,
    approval_handler: &dyn ApprovalHandler,
    event_emitter: &EventEmitter,
    collective_id: &CollectiveId,
    cancel: &CancellationToken,
) -> ToolResult {
    let Some(&tool) = tool_map.get(tool_call.name.as_str()) else {
        tracing::warn!(agent_id = %agent_id, tool = %tool_call.name, "Tool not found");
        return ToolResult::error(format!("Tool '{}' not found", tool_call.name));
    };

    // Check approval if required
    if tool.requires_approval() {
        event_emitter.emit(HiveEvent::ToolApprovalRequested {
            timestamp_ms: pulsehive_core::event::now_ms(),
            agent_id: agent_id.to_string(),
            tool_name: tool_call.name.clone(),
            description: format!("Execute {} with {:?}", tool_call.name, tool_call.arguments),
        });

        let action = PendingAction {
            agent_id: agent_id.to_string(),
            tool_name: tool_call.name.clone(),
            params: tool_call.arguments.clone(),
            description: format!("Execute {} tool", tool_call.name),
        };

        let decision = approval_handler.request_approval(&action).await;

        // Cancellation checkpoint on the approval boundary (ADR-014): the
        // approval await can outlive the run — the token may have fired
        // while the handler was deciding, and neither the `Approved` nor
        // the `Modified` path may start the tool afterwards.
        if cancel.is_cancelled() {
            tracing::info!(
                agent_id = %agent_id,
                tool = %tool_call.name,
                "Run cancelled while awaiting tool approval"
            );
            return ToolResult::error(format!(
                "Tool '{}' not executed: run cancelled while awaiting approval",
                tool_call.name
            ));
        }

        match decision {
            Ok(ApprovalResult::Approved) => {} // proceed
            Ok(ApprovalResult::Denied { reason }) => {
                return ToolResult::error(format!("Tool execution denied: {reason}"));
            }
            Ok(ApprovalResult::Modified { new_params }) => {
                // Execute with modified params
                return execute_tool_inner(
                    agent_id,
                    &tool_call.name,
                    new_params,
                    tool,
                    substrate,
                    event_emitter,
                    collective_id,
                    cancel,
                )
                .await;
            }
            Err(e) => {
                return ToolResult::error(format!("Approval handler error: {e}"));
            }
        }
    }

    execute_tool_inner(
        agent_id,
        &tool_call.name,
        tool_call.arguments.clone(),
        tool,
        substrate,
        event_emitter,
        collective_id,
        cancel,
    )
    .await
}

/// Renders a caught panic payload as a message: the `&'static str` /
/// `String` payloads every `panic!` form produces, or a placeholder for
/// anything else.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Execute a tool and emit events.
#[allow(clippy::too_many_arguments)]
async fn execute_tool_inner(
    agent_id: &str,
    tool_name: &str,
    params: serde_json::Value,
    tool: &dyn Tool,
    substrate: &Arc<dyn SubstrateProvider>,
    event_emitter: &EventEmitter,
    collective_id: &CollectiveId,
    cancel: &CancellationToken,
) -> ToolResult {
    let params_str = serde_json::to_string(&params).unwrap_or_default();
    event_emitter.emit(HiveEvent::ToolCallStarted {
        timestamp_ms: pulsehive_core::event::now_ms(),
        agent_id: agent_id.to_string(),
        tool_name: tool_name.to_string(),
        params: params_str,
    });

    // Loop-generated `Started` bookend — emitted for EVERY tool (streaming or
    // not) right after `ToolCallStarted`, so every tool call produces the
    // `Started → … → Completed` envelope.
    event_emitter.emit(HiveEvent::ToolProgress {
        timestamp_ms: pulsehive_core::event::now_ms(),
        agent_id: agent_id.to_string(),
        tool_name: tool_name.to_string(),
        progress: ToolProgress::Started {
            estimated_duration_ms: None,
        },
    });

    let start = Instant::now();
    let context = ToolContext {
        agent_id: agent_id.to_string(),
        collective_id: *collective_id,
        substrate: Arc::clone(substrate),
        event_emitter: event_emitter.clone(),
        // Each invocation gets a child of the run token (ADR-014).
        cancel: cancel.child_token(),
    };

    // Dispatch on the streaming capability probe. Streaming tools get a bounded
    // progress channel plus a forwarder task that pumps each `ToolProgress` into
    // the `HiveEvent` stream; non-streaming tools take the existing path verbatim.
    let exec_result = match tool.as_streaming() {
        Some(streaming) => {
            execute_streaming_body(
                agent_id,
                tool_name,
                params,
                streaming,
                &context,
                event_emitter,
            )
            .await
        }
        None => {
            AssertUnwindSafe(
                tool.execute(params, &context)
                    .instrument(tracing::debug_span!("tool_execute", tool = %tool_name)),
            )
            .catch_unwind()
            .await
        }
    };

    let result = normalize_tool_result(tool_name, exec_result);
    emit_tool_completion(agent_id, tool_name, start, &result, event_emitter);
    result
}

/// Normalizes a caught tool-execution result into a `ToolResult`. A panicked
/// tool becomes an ordinary tool error carrying the panic message (ADR-007:
/// fail loudly, never silently) — bookends and `ToolCallCompleted` are then
/// emitted exactly as for any other failure, and the turn continues.
fn normalize_tool_result(
    tool_name: &str,
    exec_result: Result<Result<ToolResult, PulseHiveError>, Box<dyn Any + Send>>,
) -> ToolResult {
    match exec_result {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            tracing::warn!(tool = %tool_name, error = %e, "Tool execution failed");
            ToolResult::error(e.to_string())
        }
        Err(payload) => {
            let message = panic_message(payload.as_ref());
            tracing::error!(tool = %tool_name, panic = %message, "Tool panicked");
            ToolResult::error(format!("tool '{tool_name}' panicked: {message}"))
        }
    }
}

/// Spawns the progress forwarder for a streaming tool call: drains the tool's
/// progress channel and re-emits each item as a `HiveEvent::ToolProgress`.
/// The `Started`/`Completed` bookends are the loop's alone — a tool that
/// sends them anyway gets them dropped (with a warning) so the call keeps
/// exactly one runtime-emitted pair instead of duplicates.
fn spawn_progress_forwarder(
    agent_id: &str,
    tool_name: &str,
    event_emitter: &EventEmitter,
    mut rx: mpsc::Receiver<ToolProgress>,
) -> tokio::task::JoinHandle<()> {
    // Owns its own emitter clone + labels so it is `Send + 'static` for
    // `tokio::spawn`.
    let forwarder_emitter = event_emitter.clone();
    let forwarder_agent = agent_id.to_string();
    let forwarder_tool = tool_name.to_string();
    tokio::spawn(async move {
        while let Some(progress) = rx.recv().await {
            if matches!(
                progress,
                ToolProgress::Started { .. } | ToolProgress::Completed { .. }
            ) {
                tracing::warn!(
                    tool = %forwarder_tool,
                    "dropping tool-sent progress bookend; Started/Completed are emitted by the loop"
                );
                continue;
            }
            forwarder_emitter.emit(HiveEvent::ToolProgress {
                timestamp_ms: pulsehive_core::event::now_ms(),
                agent_id: forwarder_agent.clone(),
                tool_name: forwarder_tool.clone(),
                progress,
            });
        }
    })
}

/// Runs a streaming tool's body under a panic guard with its progress
/// forwarder: opens the 64-event channel, spawns the bookend-filtering
/// forwarder, awaits `execute_streaming` under `catch_unwind`, then drains
/// the forwarder under a bounded grace. The outer `Err` carries the caught
/// panic payload; `Ok` carries the tool's own `Result`.
async fn execute_streaming_body(
    agent_id: &str,
    tool_name: &str,
    params: serde_json::Value,
    streaming: &dyn StreamingTool,
    context: &ToolContext,
    event_emitter: &EventEmitter,
) -> Result<Result<ToolResult, PulseHiveError>, Box<dyn Any + Send>> {
    let (tx, rx) = mpsc::channel::<ToolProgress>(64);
    let mut forwarder = spawn_progress_forwarder(agent_id, tool_name, event_emitter, rx);

    // Run the streaming body. The `tx` we passed is the only sender clone;
    // it drops when `execute_streaming` returns, closing the channel so the
    // forwarder observes end-of-stream. A panic in the tool body is caught
    // (not propagated): unwinding would kill the spawned agent task and
    // the stream would go silent with no bookends or ToolCallCompleted.
    // On panic the future — and with it `tx` — is dropped, so the
    // forwarder still observes end-of-stream and drains normally.
    let result = AssertUnwindSafe(
        streaming
            .execute_streaming(params, context, tx)
            .instrument(tracing::debug_span!("tool_execute", tool = %tool_name)),
    )
    .catch_unwind()
    .await;

    // The tool has returned; drain the forwarder so every buffered progress
    // event is emitted before the `Completed` bookend. Under the normal
    // contract (the sole `progress_tx` drops when `execute_streaming` returns)
    // the channel closes and the forwarder finishes immediately. But
    // `mpsc::Sender` is `Clone` and tools are third-party code: a tool that
    // clones/leaks the sender into a background task would keep the channel
    // open forever, so an unbounded `forwarder.await` here would hang the whole
    // deployment (no `Completed` / `ToolCallCompleted` / next turn). Bound the
    // wait: give the forwarder a short grace to drain, then stop it — a
    // misbehaving tool must not wedge the agent. (Fuller hardening — scoped
    // sender, backpressure contract, adversarial tests — tracked in the
    // streaming-tool hardening follow-up.)
    const FORWARDER_DRAIN_GRACE: Duration = Duration::from_secs(5);
    if tokio::time::timeout(FORWARDER_DRAIN_GRACE, &mut forwarder)
        .await
        .is_err()
    {
        tracing::warn!(
            tool = %tool_name,
            "streaming tool left a progress sender open after returning; \
             stopping the progress forwarder after the drain grace period"
        );
        forwarder.abort();
    }
    result
}

/// Emits the loop-generated `Completed` bookend and `ToolCallCompleted` for a
/// finished call, reusing the same `start.elapsed()` measurement so the two
/// events' `duration_ms` match.
fn emit_tool_completion(
    agent_id: &str,
    tool_name: &str,
    start: Instant,
    result: &ToolResult,
    event_emitter: &EventEmitter,
) {
    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::debug!(tool = %tool_name, duration_ms, "Tool completed");

    event_emitter.emit(HiveEvent::ToolProgress {
        timestamp_ms: pulsehive_core::event::now_ms(),
        agent_id: agent_id.to_string(),
        tool_name: tool_name.to_string(),
        progress: ToolProgress::Completed { duration_ms },
    });

    let result_preview: String = result.to_content().chars().take(200).collect();
    event_emitter.emit(HiveEvent::ToolCallCompleted {
        timestamp_ms: pulsehive_core::event::now_ms(),
        agent_id: agent_id.to_string(),
        tool_name: tool_name.to_string(),
        duration_ms,
        result_preview,
    });
}

// ── Perceive Phase ───────────────────────────────────────────────────

/// Query the substrate through the agent's lens and assemble budget-aware context.
async fn perceive(
    substrate: &dyn SubstrateProvider,
    lens: &Lens,
    task: &Task,
    event_emitter: &EventEmitter,
    agent_id: &str,
) -> Vec<Message> {
    use crate::perception;
    use pulsehive_core::context::ContextBudget;

    let budget = ContextBudget::from_lens(lens);
    let messages = match perception::assemble_context(substrate, lens, task.collective_id, &budget)
        .await
    {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(agent_id = %agent_id, error = %e, "Perception failed, continuing without context");
            vec![]
        }
    };

    // Count context messages that contain experience data (non-system, non-empty)
    let experience_count = messages
        .iter()
        .filter(|m| matches!(m, pulsehive_core::llm::Message::System { content } if !content.is_empty()))
        .count()
        .max(1)
        .saturating_sub(1); // Subtract the system message itself
    event_emitter.emit(HiveEvent::SubstratePerceived {
        timestamp_ms: pulsehive_core::event::now_ms(),
        agent_id: agent_id.to_string(),
        experience_count,
        insight_count: 0,
    });

    messages
}

// ── Record Phase ─────────────────────────────────────────────────────

/// Extract experiences from the conversation and store in substrate.
async fn record(
    conversation: &[Message],
    outcome: &AgentOutcome,
    ctx: &LoopContext<'_>,
    extractor: Option<&dyn ExperienceExtractor>,
) {
    use crate::experience::DefaultExperienceExtractor;
    use pulsehive_core::agent::ExtractionContext;

    let extraction_ctx = ExtractionContext {
        agent_id: ctx.agent_id.clone(),
        collective_id: ctx.task.collective_id,
        task_description: ctx.task.description.clone(),
    };

    let default_extractor = DefaultExperienceExtractor;
    let extractor: &dyn ExperienceExtractor = extractor.unwrap_or(&default_extractor);

    let experiences = extractor
        .extract(conversation, outcome, &extraction_ctx)
        .await;

    let count = experiences.len();
    for mut exp in experiences {
        // Compute embedding via provider if available and not already set
        if let Some(provider) = &ctx.embedding_provider {
            if exp.embedding.is_none() {
                let start = std::time::Instant::now();
                match provider.embed(&exp.content).await {
                    Ok(embedding) => {
                        let duration_ms = start.elapsed().as_millis() as u64;
                        let dimensions = embedding.len();
                        exp.embedding = Some(embedding);
                        ctx.event_emitter.emit(HiveEvent::EmbeddingComputed {
                            timestamp_ms: pulsehive_core::event::now_ms(),
                            agent_id: ctx.agent_id.clone(),
                            dimensions,
                            duration_ms,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(
                            agent_id = %ctx.agent_id,
                            error = %e,
                            "Failed to compute embedding, storing without"
                        );
                    }
                }
            }
        }

        // Capture metadata before move into store_experience
        let content_preview: String = exp.content.chars().take(200).collect();
        let experience_type = format!("{:?}", exp.experience_type);
        let importance = exp.importance;

        match ctx.substrate.store_experience(exp).await {
            Ok(id) => {
                ctx.event_emitter.emit(HiveEvent::ExperienceRecorded {
                    timestamp_ms: pulsehive_core::event::now_ms(),
                    experience_id: substrate_ids::from_db_experience_id(id),
                    agent_id: ctx.agent_id.clone(),
                    content_preview,
                    experience_type,
                    importance,
                });
            }
            Err(e) => {
                tracing::warn!(
                    agent_id = %ctx.agent_id,
                    error = %e,
                    "Failed to store experience"
                );
            }
        }
    }

    if count > 0 {
        tracing::debug!(agent_id = %ctx.agent_id, count = count, "Recorded experiences");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_core::Stream;
    use pulsehive_core::error::{PulseHiveError, Result};
    use pulsehive_core::llm::{LlmChunk, LlmResponse, TokenUsage};
    use std::pin::Pin;
    use std::sync::Mutex;

    // ── Mock LLM Provider ────────────────────────────────────────────

    /// Mock LLM that returns scripted responses in order.
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
            LlmResponse::text(content)
        }

        fn tool_call_response(id: &str, name: &str, args: serde_json::Value) -> LlmResponse {
            LlmResponse::new(
                None,
                vec![ToolCall {
                    id: id.into(),
                    name: name.into(),
                    arguments: args,
                }],
                TokenUsage::default(),
            )
        }
    }

    #[async_trait]
    impl LlmProvider for MockLlm {
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

    // ── Mock Tool ────────────────────────────────────────────────────

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes input"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn execute(
            &self,
            params: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolResult> {
            let text = params["text"].as_str().unwrap_or("no text");
            Ok(ToolResult::text(format!("Echo: {text}")))
        }
    }

    // ── Helper ───────────────────────────────────────────────────────

    fn test_config(tools: Vec<Arc<dyn Tool>>) -> LlmAgentConfig {
        LlmAgentConfig {
            system_prompt: "You are a test agent.".into(),
            tools,
            lens: pulsehive_core::lens::Lens::default(),
            llm_config: LlmConfig::new("mock", "test-model"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        }
    }

    fn test_task() -> Task {
        Task::new("Test task")
    }

    fn test_substrate() -> Arc<dyn SubstrateProvider> {
        // Use a real PulseDB with tempfile for substrate
        let dir = tempfile::tempdir().unwrap();
        let db =
            pulsedb::PulseDB::open(dir.path().join("test.db"), pulsedb::Config::default()).unwrap();
        // Leak the tempdir so it lives long enough
        let dir = Box::leak(Box::new(dir));
        let _ = dir;
        Arc::new(pulsedb::PulseDBSubstrate::from_db(db))
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_text_only_response() {
        let provider = Arc::new(MockLlm::new(vec![MockLlm::text_response(
            "The answer is 42.",
        )]));
        let config = test_config(vec![]);
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let approval = pulsehive_core::approval::AutoApprove;

        let outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-1".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "The answer is 42.")
        );
    }

    #[tokio::test]
    async fn test_tool_call_then_response() {
        let provider = Arc::new(MockLlm::new(vec![
            MockLlm::tool_call_response("call_1", "echo", serde_json::json!({"text": "hello"})),
            MockLlm::text_response("Echo said: hello"),
        ]));
        let config = test_config(vec![Arc::new(EchoTool)]);
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let approval = pulsehive_core::approval::AutoApprove;

        let outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-1".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        assert!(
            matches!(&outcome, AgentOutcome::Complete { response } if response == "Echo said: hello")
        );
    }

    #[tokio::test]
    async fn test_max_iterations_reached() {
        // LLM always returns tool calls — never gives a final response
        let responses: Vec<LlmResponse> = (0..5)
            .map(|i| {
                MockLlm::tool_call_response(
                    &format!("call_{i}"),
                    "echo",
                    serde_json::json!({"text": "loop"}),
                )
            })
            .collect();

        let provider = Arc::new(MockLlm::new(responses));
        let config = test_config(vec![Arc::new(EchoTool)]);
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let approval = pulsehive_core::approval::AutoApprove;

        let outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-1".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: 3, // Only 3 iterations
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        assert!(matches!(outcome, AgentOutcome::MaxIterationsReached));
    }

    #[tokio::test]
    async fn test_tool_not_found() {
        // LLM calls a tool that doesn't exist, then gives final response
        let provider = Arc::new(MockLlm::new(vec![
            MockLlm::tool_call_response("call_1", "nonexistent_tool", serde_json::json!({})),
            MockLlm::text_response("I couldn't find that tool."),
        ]));
        let config = test_config(vec![]); // No tools registered
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let approval = pulsehive_core::approval::AutoApprove;

        let outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-1".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        // Should complete (LLM recovered from tool-not-found error)
        assert!(matches!(outcome, AgentOutcome::Complete { .. }));
    }

    #[tokio::test]
    async fn test_llm_error_returns_error_outcome() {
        // Provider that always returns error
        let provider = Arc::new(MockLlm::new(vec![])); // Empty = error
        let config = test_config(vec![]);
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let approval = pulsehive_core::approval::AutoApprove;

        let outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-1".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        assert!(matches!(outcome, AgentOutcome::Error { .. }));
    }

    #[tokio::test]
    async fn test_events_emitted_during_loop() {
        let provider = Arc::new(MockLlm::new(vec![
            MockLlm::tool_call_response("call_1", "echo", serde_json::json!({"text": "test"})),
            MockLlm::text_response("Done"),
        ]));
        let config = test_config(vec![Arc::new(EchoTool)]);
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let mut rx = emitter.subscribe();
        let approval = pulsehive_core::approval::AutoApprove;

        let _outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-1".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        // Collect all events
        let mut events = vec![];
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Should have: SubstratePerceived, LlmCallStarted, LlmCallCompleted,
        // ToolCallStarted, ToolCallCompleted, LlmCallStarted, LlmCallCompleted
        assert!(events
            .iter()
            .any(|e| matches!(e, HiveEvent::SubstratePerceived { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, HiveEvent::LlmCallStarted { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, HiveEvent::LlmCallCompleted { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, HiveEvent::ToolCallStarted { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, HiveEvent::ToolCallCompleted { .. })));
    }

    // ── Mid-task refresh tests ───────────────────────────────────────

    fn test_config_with_refresh(
        tools: Vec<Arc<dyn Tool>>,
        refresh: Option<usize>,
    ) -> LlmAgentConfig {
        let mut config = test_config(tools);
        config.refresh_every_n_tool_calls = refresh;
        config
    }

    #[tokio::test]
    async fn test_refresh_disabled_no_extra_perception() {
        // 1 tool call + final response, refresh=None → only 1 SubstratePerceived
        let provider = Arc::new(MockLlm::new(vec![
            MockLlm::tool_call_response("c1", "echo", serde_json::json!({"text": "a"})),
            MockLlm::text_response("Done"),
        ]));
        let config = test_config_with_refresh(vec![Arc::new(EchoTool)], None);
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let mut rx = emitter.subscribe();
        let approval = pulsehive_core::approval::AutoApprove;

        let _outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-no-refresh".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }

        let perceive_count = events
            .iter()
            .filter(|e| matches!(e, HiveEvent::SubstratePerceived { .. }))
            .count();
        assert_eq!(
            perceive_count, 1,
            "With refresh=None, should have exactly 1 SubstratePerceived (initial). Got {perceive_count}"
        );
    }

    #[tokio::test]
    async fn test_refresh_every_1_triggers_after_tool_call() {
        // 2 tool calls + final response, refresh=Some(1) → 1 initial + 2 refresh = 3 SubstratePerceived
        let provider = Arc::new(MockLlm::new(vec![
            MockLlm::tool_call_response("c1", "echo", serde_json::json!({"text": "a"})),
            MockLlm::tool_call_response("c2", "echo", serde_json::json!({"text": "b"})),
            MockLlm::text_response("Done"),
        ]));
        let config = test_config_with_refresh(vec![Arc::new(EchoTool)], Some(1));
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let mut rx = emitter.subscribe();
        let approval = pulsehive_core::approval::AutoApprove;

        let _outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-refresh-1".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }

        let perceive_count = events
            .iter()
            .filter(|e| matches!(e, HiveEvent::SubstratePerceived { .. }))
            .count();
        assert!(
            perceive_count >= 3,
            "With refresh=Some(1) and 2 tool calls, should have >= 3 SubstratePerceived. Got {perceive_count}"
        );
    }

    #[tokio::test]
    async fn test_refresh_not_triggered_below_threshold() {
        // 1 tool call + final response, refresh=Some(10) → only 1 SubstratePerceived (threshold not reached)
        let provider = Arc::new(MockLlm::new(vec![
            MockLlm::tool_call_response("c1", "echo", serde_json::json!({"text": "a"})),
            MockLlm::text_response("Done"),
        ]));
        let config = test_config_with_refresh(vec![Arc::new(EchoTool)], Some(10));
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let mut rx = emitter.subscribe();
        let approval = pulsehive_core::approval::AutoApprove;

        let _outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-high-threshold".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }

        let perceive_count = events
            .iter()
            .filter(|e| matches!(e, HiveEvent::SubstratePerceived { .. }))
            .count();
        assert_eq!(
            perceive_count, 1,
            "With refresh=Some(10) and 1 tool call, should have exactly 1 SubstratePerceived. Got {perceive_count}"
        );
    }

    // ── Streaming tool-progress wiring tests (WI 1.03) ───────────────────

    use pulsehive_core::tool::StreamingTool;

    // Mock streaming tool: pushes a deterministic
    // Progress(0.5) → PartialResult → Progress(1.0) sequence over its channel and
    // never sends `Started`/`Completed` (those are loop bookends). No sleeps, so
    // ordering assertions are deterministic.
    struct ProgressTool;

    #[async_trait]
    impl Tool for ProgressTool {
        fn name(&self) -> &str {
            "progress"
        }
        fn description(&self) -> &str {
            "streaming mock that emits intermediate progress"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolResult> {
            Ok(ToolResult::text("progress-done"))
        }
        fn as_streaming(&self) -> Option<&dyn StreamingTool> {
            Some(self)
        }
    }

    #[async_trait]
    impl StreamingTool for ProgressTool {
        async fn execute_streaming(
            &self,
            _params: serde_json::Value,
            _context: &ToolContext,
            progress_tx: mpsc::Sender<ToolProgress>,
        ) -> Result<ToolResult> {
            let _ = progress_tx
                .send(ToolProgress::Progress {
                    fraction: 0.5,
                    message: None,
                })
                .await;
            let _ = progress_tx
                .send(ToolProgress::PartialResult {
                    payload: serde_json::json!({"n": 1}),
                })
                .await;
            let _ = progress_tx
                .send(ToolProgress::Progress {
                    fraction: 1.0,
                    message: Some("done".into()),
                })
                .await;
            Ok(ToolResult::text("progress-done"))
        }
    }

    // Classify only the tool-call envelope events (ToolCallStarted, the loop's
    // ToolProgress bookends + forwarded intermediate progress, ToolCallCompleted)
    // into short, order-preserving tags. Non-tool events (Llm*, Substrate*) are
    // dropped so the envelope ordering can be asserted directly.
    fn tool_envelope_tags(events: &[HiveEvent]) -> Vec<&'static str> {
        events
            .iter()
            .filter_map(|e| match e {
                HiveEvent::ToolCallStarted { .. } => Some("ToolCallStarted"),
                HiveEvent::ToolCallCompleted { .. } => Some("ToolCallCompleted"),
                HiveEvent::ToolProgress { progress, .. } => Some(match progress {
                    ToolProgress::Started { .. } => "Started",
                    ToolProgress::Progress { .. } => "Progress",
                    ToolProgress::PartialResult { .. } => "PartialResult",
                    ToolProgress::Log { .. } => "Log",
                    ToolProgress::Completed { .. } => "Completed",
                }),
                _ => None,
            })
            .collect()
    }

    // Drive the loop with a single tool call, subscribing BEFORE the run and
    // draining promptly after into a Vec (event count << broadcast cap 256, so no
    // lag drop is possible for this deterministic set).
    async fn run_loop_collect(tools: Vec<Arc<dyn Tool>>) -> Vec<HiveEvent> {
        let provider = Arc::new(MockLlm::new(vec![
            MockLlm::tool_call_response("call_1", tools[0].name(), serde_json::json!({})),
            MockLlm::text_response("Done"),
        ]));
        let config = test_config(tools);
        let task = test_task();
        let substrate = test_substrate();
        let emitter = EventEmitter::default();
        let mut rx = emitter.subscribe();
        let approval = pulsehive_core::approval::AutoApprove;

        let _outcome = run_agentic_loop(
            config,
            LoopContext {
                agent_id: "agent-streaming".into(),
                task: &task,
                provider,
                substrate,
                approval_handler: &approval,
                event_emitter: emitter,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                embedding_provider: None,
                cancel: CancellationToken::new(),
            },
        )
        .await;

        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        events
    }

    #[tokio::test]
    async fn plain_tool_streaming_bookends() {
        // A non-streaming tool must still yield exactly the loop-generated envelope:
        // ToolCallStarted → ToolProgress::Started → ToolProgress::Completed → ToolCallCompleted.
        let events = run_loop_collect(vec![Arc::new(EchoTool)]).await;
        let tags = tool_envelope_tags(&events);
        assert_eq!(
            tags,
            vec![
                "ToolCallStarted",
                "Started",
                "Completed",
                "ToolCallCompleted"
            ],
            "plain tool must produce the exact Started→Completed envelope. Tags: {tags:?}"
        );
    }

    #[tokio::test]
    async fn streaming_tool_progress_ordering() {
        // A streaming tool's intermediate ToolProgress events must appear in send
        // order, bracketed by the loop's Started/Completed bookends and the
        // ToolCall* envelope.
        let events = run_loop_collect(vec![Arc::new(ProgressTool)]).await;
        let tags = tool_envelope_tags(&events);
        assert_eq!(
            tags,
            vec![
                "ToolCallStarted",
                "Started",
                "Progress",
                "PartialResult",
                "Progress",
                "Completed",
                "ToolCallCompleted"
            ],
            "streaming tool must emit intermediate progress in send order between bookends. Tags: {tags:?}"
        );
    }
}
