//! Release 1 exit criterion 6 — a consumer drives an agent turn in a unit
//! test through `pulsehive::testing::ScriptedProvider`, with no API key and
//! no network, and asserts both what the agent did and what the provider saw.
//!
//! Written against `pulsehive::*` only: no provider-crate feature, no
//! provider-crate import. The scripted tool call runs the tool, the scripted
//! answer completes the turn, and the provider's second recorded request
//! carries the tool's result.

use std::future::poll_fn;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{json, Value};

use pulsehive::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive::error::Result;
use pulsehive::event::HiveEvent;
use pulsehive::lens::Lens;
use pulsehive::llm::{LlmConfig, Message};
use pulsehive::testing::ScriptedProvider;
use pulsehive::tool::{Tool, ToolContext, ToolResult};
use pulsehive::{HiveMind, Task};

/// The tool the scripted model calls: echoes its `text` argument back and
/// counts its invocations.
struct EchoTool {
    invocations: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes its text argument back"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        })
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(ToolResult::text(text))
    }
}

/// One event off the deploy stream, without a stream-ext crate: `poll_fn`
/// bridges the boxed `futures_core::Stream`.
async fn next_event(
    stream: &mut Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
) -> Option<HiveEvent> {
    poll_fn(|cx| stream.as_mut().poll_next(cx)).await
}

#[tokio::test]
async fn drives_an_agent_turn_offline_through_the_scripted_provider() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let invocations = Arc::new(AtomicUsize::new(0));

    let provider = ScriptedProvider::new()
        .then_tool_call("echo", json!({"text": "hi"}))
        .then_text("done");

    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("scripted-agent-turn.db"))
        .llm_provider("scripted", provider.clone())
        .build()
        .expect("build HiveMind");

    let agent = AgentDefinition {
        name: "echoer".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You echo text back.".into(),
            tools: vec![Arc::new(EchoTool {
                invocations: invocations.clone(),
            })],
            lens: Lens::default(),
            llm_config: LlmConfig::new("scripted", "test"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    };

    let mut stream = hive
        .deploy(vec![agent], vec![Task::new("Echo 'hi' back")])
        .await
        .expect("deploy agents");

    // Drain until the agent completes. The turn itself is deterministic; the
    // 60s timeout is a hang guard only (as in the streaming_tool example).
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        let mut outcome = None;
        while let Some(event) = next_event(&mut stream).await {
            if let HiveEvent::AgentCompleted {
                outcome: agent_outcome,
                ..
            } = event
            {
                outcome = Some(agent_outcome);
                break;
            }
        }
        outcome
    })
    .await
    .expect("drain timed out before AgentCompleted")
    .expect("stream ended without AgentCompleted");

    // The scripted final answer completed the turn.
    match outcome {
        AgentOutcome::Complete { response } => assert_eq!(response, "done"),
        other => panic!("expected AgentOutcome::Complete, got {other:?}"),
    }

    // The scripted tool call ran the tool exactly once.
    assert_eq!(invocations.load(Ordering::SeqCst), 1);

    // The provider saw two calls, and the second carried the tool's result.
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].messages.iter().any(|message| matches!(
            message,
            Message::ToolResult { content, .. } if content == "hi"
        )),
        "the second recorded request must carry the tool's 'hi' result: {:?}",
        requests[1].messages
    );
}
