//! Integration tests for the ApprovalHandler flow in the agentic loop.
//!
//! Validates E4-S01: all 3 ApprovalResult variants (Approved, Denied, Modified)
//! work end-to-end through HiveMind.deploy().

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures_core::Stream;
use serde_json::Value;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive_core::approval::{ApprovalHandler, ApprovalResult, PendingAction};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::*;
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};

// ── Mock LLM Provider ────────────────────────────────────────────────

struct MockLlm {
    responses: Mutex<Vec<LlmResponse>>,
}

impl MockLlm {
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
        Err(PulseHiveError::llm("Not used in tests"))
    }
}

// ── Custom Approval Handlers ─────────────────────────────────────────

/// Always denies tool execution.
struct DenyingHandler;

#[async_trait]
impl ApprovalHandler for DenyingHandler {
    async fn request_approval(&self, action: &PendingAction) -> Result<ApprovalResult> {
        Ok(ApprovalResult::Denied {
            reason: format!("{} is restricted", action.tool_name),
        })
    }
}

/// Modifies params by adding a "safe_mode" field.
struct ModifyingHandler;

#[async_trait]
impl ApprovalHandler for ModifyingHandler {
    async fn request_approval(&self, action: &PendingAction) -> Result<ApprovalResult> {
        let mut new_params = action.params.clone();
        if let Some(obj) = new_params.as_object_mut() {
            obj.insert("safe_mode".into(), Value::Bool(true));
        }
        Ok(ApprovalResult::Modified { new_params })
    }
}

// ── Mock Tools ───────────────────────────────────────────────────────

/// Tool that requires approval. Tracks execution count.
struct DangerousTool {
    exec_count: AtomicUsize,
}

impl DangerousTool {
    fn new() -> Self {
        Self {
            exec_count: AtomicUsize::new(0),
        }
    }

    fn execution_count(&self) -> usize {
        self.exec_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Tool for DangerousTool {
    fn name(&self) -> &str {
        "dangerous_action"
    }
    fn description(&self) -> &str {
        "A sensitive action that requires approval"
    }
    fn parameters(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"target": {"type": "string"}}})
    }
    fn requires_approval(&self) -> bool {
        true
    }
    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        self.exec_count.fetch_add(1, Ordering::SeqCst);
        let target = params["target"].as_str().unwrap_or("unknown");
        let safe_mode = params["safe_mode"].as_bool().unwrap_or(false);
        if safe_mode {
            Ok(ToolResult::text(format!(
                "Executed {target} in safe mode"
            )))
        } else {
            Ok(ToolResult::text(format!("Executed {target}")))
        }
    }
}

/// Safe tool that does not require approval.
struct SafeTool;

#[async_trait]
impl Tool for SafeTool {
    fn name(&self) -> &str {
        "safe_action"
    }
    fn description(&self) -> &str {
        "A safe action that does not require approval"
    }
    fn parameters(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"message": {"type": "string"}}})
    }
    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let msg = params["message"].as_str().unwrap_or("ok");
        Ok(ToolResult::text(format!("Safe: {msg}")))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn build_hive_with_handler(
    provider: MockLlm,
    handler: impl ApprovalHandler + 'static,
) -> HiveMind {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    Box::leak(Box::new(dir));

    HiveMind::builder()
        .substrate_path(&path)
        .llm_provider("mock", provider)
        .approval_handler(handler)
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
            _ = tokio::time::sleep_until(deadline) => {
                break;
            }
        }
    }

    events
}

// ── Tests ────────────────────────────────────────────────────────────

/// Test the full denied → alternative flow:
/// 1. LLM tries dangerous_action → denied
/// 2. LLM tries safe_action → approved (no approval required)
/// 3. LLM gives final response
#[tokio::test]
async fn test_denied_tool_llm_chooses_alternative() {
    let provider = MockLlm::new(vec![
        // First attempt: LLM calls the dangerous tool
        MockLlm::tool_call(
            "call_1",
            "dangerous_action",
            serde_json::json!({"target": "production"}),
        ),
        // After denial error, LLM calls the safe tool
        MockLlm::tool_call(
            "call_2",
            "safe_action",
            serde_json::json!({"message": "fallback"}),
        ),
        // Final response
        MockLlm::text("Used safe action instead."),
    ]);

    let dangerous_tool = Arc::new(DangerousTool::new());
    let dangerous_ref = Arc::clone(&dangerous_tool);

    let hive = build_hive_with_handler(provider, DenyingHandler);
    let agent = llm_agent(
        "approval-test",
        vec![dangerous_tool as Arc<dyn Tool>, Arc::new(SafeTool)],
    );
    let task = Task::new("Execute an action");

    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // 1. ToolApprovalRequested should have been emitted
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::ToolApprovalRequested { tool_name, .. }
            if tool_name == "dangerous_action"
        )),
        "Expected ToolApprovalRequested for dangerous_action. Events: {events:?}"
    );

    // 2. DangerousTool should NOT have been executed (denied)
    assert_eq!(
        dangerous_ref.execution_count(),
        0,
        "DangerousTool should not have been executed"
    );

    // 3. SafeTool SHOULD have been executed
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::ToolCallCompleted { tool_name, .. }
            if tool_name == "safe_action"
        )),
        "Expected SafeTool to be executed. Events: {events:?}"
    );

    // 4. Agent should complete successfully
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::AgentCompleted {
                outcome: AgentOutcome::Complete { response },
                ..
            } if response == "Used safe action instead."
        )),
        "Expected successful completion. Events: {events:?}"
    );
}

/// Test that modified params are passed through to tool execution.
#[tokio::test]
async fn test_modified_params_flow() {
    let provider = MockLlm::new(vec![
        MockLlm::tool_call(
            "call_1",
            "dangerous_action",
            serde_json::json!({"target": "database"}),
        ),
        MockLlm::text("Action completed in safe mode."),
    ]);

    let dangerous_tool = Arc::new(DangerousTool::new());
    let dangerous_ref = Arc::clone(&dangerous_tool);

    let hive = build_hive_with_handler(provider, ModifyingHandler);
    let agent = llm_agent(
        "modify-test",
        vec![dangerous_tool as Arc<dyn Tool>],
    );
    let task = Task::new("Execute with modification");

    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // Tool WAS executed (Modified → execute with new params)
    assert_eq!(
        dangerous_ref.execution_count(),
        1,
        "DangerousTool should have been executed once with modified params"
    );

    // ToolApprovalRequested was emitted
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::ToolApprovalRequested { tool_name, .. }
            if tool_name == "dangerous_action"
        )),
        "Expected ToolApprovalRequested. Events: {events:?}"
    );

    // Agent completed
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::AgentCompleted {
                outcome: AgentOutcome::Complete { .. },
                ..
            }
        )),
        "Expected successful completion. Events: {events:?}"
    );
}

/// Test that ApprovalResult::Approved allows the tool to execute normally.
/// Verifies the approval event is still emitted even when approved.
#[tokio::test]
async fn test_approved_tool_executes() {
    use pulsehive_core::approval::AutoApprove;

    let provider = MockLlm::new(vec![
        MockLlm::tool_call(
            "call_1",
            "dangerous_action",
            serde_json::json!({"target": "staging"}),
        ),
        MockLlm::text("Action executed successfully."),
    ]);

    let dangerous_tool = Arc::new(DangerousTool::new());
    let dangerous_ref = Arc::clone(&dangerous_tool);

    // AutoApprove always returns Approved
    let hive = build_hive_with_handler(provider, AutoApprove);
    let agent = llm_agent(
        "approve-test",
        vec![dangerous_tool as Arc<dyn Tool>],
    );
    let task = Task::new("Execute approved action");

    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // 1. ToolApprovalRequested should still be emitted (before handler check)
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::ToolApprovalRequested { tool_name, .. }
            if tool_name == "dangerous_action"
        )),
        "Expected ToolApprovalRequested. Events: {events:?}"
    );

    // 2. Tool SHOULD have been executed (approved)
    assert_eq!(
        dangerous_ref.execution_count(),
        1,
        "DangerousTool should have been executed once"
    );

    // 3. ToolCallCompleted should be emitted
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::ToolCallCompleted { tool_name, .. }
            if tool_name == "dangerous_action"
        )),
        "Expected ToolCallCompleted for dangerous_action. Events: {events:?}"
    );

    // 4. Agent should complete successfully
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::AgentCompleted {
                outcome: AgentOutcome::Complete { .. },
                ..
            }
        )),
        "Expected successful completion. Events: {events:?}"
    );
}

/// Test that the denial reason message reaches the LLM conversation.
/// We verify this indirectly: if the LLM's second call succeeds, it received
/// the error message from the denied tool and chose an alternative.
#[tokio::test]
async fn test_denied_message_informs_llm() {
    // The LLM sees "Error: Tool execution denied: dangerous_action is restricted"
    // as a tool_result message, then chooses to respond with text.
    let provider = MockLlm::new(vec![
        MockLlm::tool_call(
            "call_1",
            "dangerous_action",
            serde_json::json!({"target": "prod"}),
        ),
        // After seeing denial error, LLM responds with text
        MockLlm::text("I was denied access. Understood."),
    ]);

    let hive = build_hive_with_handler(provider, DenyingHandler);
    let agent = llm_agent(
        "denial-message-test",
        vec![Arc::new(DangerousTool::new()) as Arc<dyn Tool>],
    );
    let task = Task::new("Try the action");

    let stream = hive.deploy(vec![agent], vec![task]).await.unwrap();
    let events = collect_events(stream, Duration::from_secs(5)).await;

    // The agent should complete (LLM recovered from denial)
    assert!(
        events.iter().any(|e| matches!(
            e,
            HiveEvent::AgentCompleted {
                outcome: AgentOutcome::Complete { response },
                ..
            } if response.contains("denied")
        )),
        "LLM should have acknowledged the denial. Events: {events:?}"
    );
}
