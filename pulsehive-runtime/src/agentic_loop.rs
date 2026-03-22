//! Agentic loop engine — the Perceive→Think→Act→Record cycle.
//!
//! This module implements the core execution loop for LLM agents. Each agent
//! runs through: perceive substrate → think via LLM → act on tool calls → record experiences.
//!
//! The loop is driven by [`run_agentic_loop`], called from `HiveMind::deploy()`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use pulsedb::SubstrateProvider;

use pulsehive_core::agent::{AgentOutcome, ExperienceExtractor, LlmAgentConfig};
use pulsehive_core::approval::{ApprovalHandler, ApprovalResult, PendingAction};
use pulsehive_core::event::{EventEmitter, HiveEvent};
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::{LlmConfig, LlmProvider, Message, ToolCall, ToolDefinition};
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};

use crate::hivemind::Task;

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
    tracing::info!(agent_id = %ctx.agent_id, "Perceive phase");
    let context_messages = perceive(
        ctx.substrate.as_ref(),
        &lens,
        ctx.task,
        &ctx.event_emitter,
        &ctx.agent_id,
    )
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
    tracing::info!(agent_id = %ctx.agent_id, "Record phase");
    record(&messages, &outcome, &ctx, experience_extractor.as_deref()).await;

    outcome
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

    for iteration in 1..=ctx.max_iterations {
        tracing::info!(agent_id = %agent_id, iteration = iteration, model = %llm_config.model, "Think phase");

        // ── THINK: call LLM ──────────────────────────────────────────
        ctx.event_emitter.emit(HiveEvent::LlmCallStarted {
            agent_id: agent_id.to_string(),
            model: llm_config.model.clone(),
            message_count: messages.len(),
        });

        let start = Instant::now();
        let response = ctx
            .provider
            .chat(messages.clone(), tool_defs.to_vec(), llm_config)
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        ctx.event_emitter.emit(HiveEvent::LlmCallCompleted {
            agent_id: agent_id.to_string(),
            model: llm_config.model.clone(),
            duration_ms,
        });

        let response = match response {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(agent_id = %agent_id, error = %e, "LLM call failed");
                return AgentOutcome::Error {
                    error: e.to_string(),
                };
            }
        };

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
            tracing::info!(agent_id = %agent_id, tool = %tool_call.name, "Act phase");
            let result = execute_tool_call(
                agent_id,
                tool_call,
                tool_map,
                &ctx.substrate,
                ctx.approval_handler,
                &ctx.event_emitter,
                &ctx.task.collective_id,
            )
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
async fn execute_tool_call(
    agent_id: &str,
    tool_call: &ToolCall,
    tool_map: &HashMap<&str, &dyn Tool>,
    substrate: &Arc<dyn SubstrateProvider>,
    approval_handler: &dyn ApprovalHandler,
    event_emitter: &EventEmitter,
    collective_id: &pulsedb::CollectiveId,
) -> ToolResult {
    let Some(&tool) = tool_map.get(tool_call.name.as_str()) else {
        tracing::warn!(agent_id = %agent_id, tool = %tool_call.name, "Tool not found");
        return ToolResult::error(format!("Tool '{}' not found", tool_call.name));
    };

    // Check approval if required
    if tool.requires_approval() {
        event_emitter.emit(HiveEvent::ToolApprovalRequested {
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

        match approval_handler.request_approval(&action).await {
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
    )
    .await
}

/// Execute a tool and emit events.
async fn execute_tool_inner(
    agent_id: &str,
    tool_name: &str,
    params: serde_json::Value,
    tool: &dyn Tool,
    substrate: &Arc<dyn SubstrateProvider>,
    event_emitter: &EventEmitter,
    collective_id: &pulsedb::CollectiveId,
) -> ToolResult {
    event_emitter.emit(HiveEvent::ToolCallStarted {
        agent_id: agent_id.to_string(),
        tool_name: tool_name.to_string(),
    });

    let start = Instant::now();
    let context = ToolContext {
        agent_id: agent_id.to_string(),
        collective_id: *collective_id,
        substrate: Arc::clone(substrate),
        event_emitter: event_emitter.clone(),
    };

    let result = match tool.execute(params, &context).await {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!(agent_id = %agent_id, tool = %tool_name, error = %e, "Tool execution failed");
            ToolResult::error(e.to_string())
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    event_emitter.emit(HiveEvent::ToolCallCompleted {
        agent_id: agent_id.to_string(),
        tool_name: tool_name.to_string(),
        duration_ms,
    });

    result
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

    let experience_count = if messages.is_empty() { 0 } else { 1 }; // At least 1 context message
    event_emitter.emit(HiveEvent::SubstratePerceived {
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
    for exp in experiences {
        match ctx.substrate.store_experience(exp).await {
            Ok(id) => {
                ctx.event_emitter.emit(HiveEvent::ExperienceRecorded {
                    experience_id: id,
                    agent_id: ctx.agent_id.clone(),
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
    use pulsedb::CollectiveId;
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
            LlmResponse {
                content: Some(content.into()),
                tool_calls: vec![],
                usage: TokenUsage::default(),
            }
        }

        fn tool_call_response(id: &str, name: &str, args: serde_json::Value) -> LlmResponse {
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
        Task {
            description: "Test task".into(),
            collective_id: CollectiveId::new(),
        }
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
}
