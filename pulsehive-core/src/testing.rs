//! Test doubles for driving PulseHive agents deterministically, offline.
//!
//! [`ScriptedProvider`] is an [`LlmProvider`] whose
//! replies are queued in advance with a `then_*` builder. A test scripts the
//! conversation it wants, hands a clone to `HiveMindBuilder` (providers are
//! taken by value), drives the agent, and reads back what the agent sent.
//! No API key and no network are involved — the provider is the product-owned
//! swappable interface, so the turn it drives is the real agent loop.
//!
//! One complete consumer example (the `testing` feature also exposes this
//! module through the meta-crate as `pulsehive::testing`):
//!
//! ```rust,ignore
//! use std::{future::poll_fn, sync::Arc, time::Duration};
//!
//! use futures_core::Stream;
//! use pulsehive::testing::ScriptedProvider;
//! use pulsehive::{AgentDefinition, AgentKind, AgentOutcome, HiveEvent, HiveMind,
//!                 LlmAgentConfig, LlmConfig, Lens, Task, Tool, ToolContext, ToolResult};
//! use serde_json::json;
//!
//! #[tokio::test]
//! async fn drives_one_agent_turn_offline() {
//!     struct Echo;
//!     #[async_trait::async_trait]
//!     impl Tool for Echo {
//!         fn name(&self) -> &str { "echo" }
//!         fn description(&self) -> &str { "echoes its text argument back" }
//!         fn parameters(&self) -> serde_json::Value { json!({"type": "object"}) }
//!         async fn execute(&self, params: serde_json::Value, _: &ToolContext)
//!             -> pulsehive::Result<ToolResult>
//!         {
//!             Ok(ToolResult::text(params["text"].as_str().unwrap_or_default()))
//!         }
//!     }
//!
//!     let dir = tempfile::tempdir().unwrap();
//!     let provider = ScriptedProvider::new()
//!         .then_tool_call("echo", json!({"text": "hi"}))
//!         .then_text("done");
//!     let hive = HiveMind::builder()
//!         .substrate_path(dir.path().join("test.db"))
//!         .llm_provider("scripted", provider.clone())
//!         .build()
//!         .unwrap();
//!     let agent = AgentDefinition {
//!         name: "echoer".into(),
//!         kind: AgentKind::Llm(Box::new(LlmAgentConfig {
//!             system_prompt: "You echo.".into(),
//!             tools: vec![Arc::new(Echo)],
//!             lens: Lens::default(),
//!             llm_config: LlmConfig::new("scripted", "test"),
//!             experience_extractor: None,
//!             refresh_every_n_tool_calls: None,
//!         })),
//!     };
//!     let mut stream = hive
//!         .deploy(vec![agent], vec![Task::new("echo hi")])
//!         .await
//!         .unwrap();
//!     let outcome = tokio::time::timeout(Duration::from_secs(60), async {
//!         loop {
//!             match poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
//!                 Some(HiveEvent::AgentCompleted { outcome, .. }) => break outcome,
//!                 Some(_) => {}
//!                 None => panic!("stream ended before AgentCompleted"),
//!             }
//!         }
//!     })
//!     .await
//!     .unwrap();
//!     match outcome {
//!         AgentOutcome::Complete { response } => assert_eq!(response, "done"),
//!         other => panic!("expected a complete outcome, got {other:?}"),
//!     }
//!     assert_eq!(provider.requests().len(), 2); // the tool result went back in
//! }
//! ```

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures_core::Stream;
use tokio_util::sync::CancellationToken;

use crate::error::{PulseHiveError, Result};
use crate::llm::{
    LlmChunk, LlmConfig, LlmError, LlmErrorKind, LlmProvider, LlmResponse, Message, TokenUsage,
    ToolCall, ToolDefinition,
};

/// One queued reply. Consumed FIFO, exactly one per call.
#[derive(Debug)]
enum Step {
    /// Returned (or replayed) verbatim.
    Response(LlmResponse),
    /// Returned as-is.
    Error(PulseHiveError),
    /// Pends until the call's cancel token fires.
    Hang,
}

#[derive(Debug, Default)]
struct Inner {
    steps: VecDeque<Step>,
    requests: Vec<RecordedRequest>,
    /// Next per-provider tool-call number, assigned at script time.
    next_tool_call: usize,
    /// Steps already handed to calls — the count an exhaustion error names.
    served: usize,
}

/// A queued-response [`LlmProvider`] for tests.
///
/// Script replies in call order with the `then_*` builders, hand the provider
/// (or a clone — all clones share one queue and one request log) to the
/// component under test, and inspect what it sent with [`ScriptedProvider::requests`].
///
/// Each `chat`/`chat_stream` call first records its inputs, then takes exactly
/// one step from the front of the queue; a cancelled call still consumes its
/// step. When the queue is empty the call fails with
/// [`PulseHiveError::Llm`] naming how many
/// calls were served — never a panic, never a default reply. Exhaustion wins
/// over cancellation: an already-cancelled call with an empty queue gets the
/// exhaustion error, because step-taking is checked first.
///
/// Cancellation follows the provider transport contract: a token already
/// cancelled at call start, or a [`ScriptedProvider::then_hang`] step whose
/// token fires, returns
/// [`PulseHiveError::LlmTransport`]
/// with kind [`Cancelled`](crate::llm::LlmErrorKind::Cancelled) and
/// `attempts == 1`.
///
/// The type is non-exhaustive and grows only through `new()` and the builders.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ScriptedProvider {
    inner: Arc<Mutex<Inner>>,
}

impl ScriptedProvider {
    /// Creates a provider with an empty script.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a text reply: `LlmResponse::text(..)` with finish reason
    /// `"stop"` and zero usage.
    pub fn then_text(self, content: impl Into<String>) -> Self {
        self.lock().steps.push_back(Step::Response(
            LlmResponse::text(content).with_finish_reason("stop"),
        ));
        self
    }

    /// Queues a reply carrying exactly one tool call. The id is assigned at
    /// script time, numbered per provider (`call_1`, `call_2`, …), whatever
    /// steps sit between them. Finish reason is `"tool_calls"`, usage zero.
    pub fn then_tool_call(self, name: impl Into<String>, arguments: serde_json::Value) -> Self {
        {
            let mut inner = self.lock();
            let id = format!("call_{}", inner.next_tool_call + 1);
            inner.next_tool_call += 1;
            inner.steps.push_back(Step::Response(
                LlmResponse::new(
                    None,
                    vec![ToolCall {
                        id,
                        name: name.into(),
                        arguments,
                    }],
                    TokenUsage::default(),
                )
                .with_finish_reason("tool_calls"),
            ));
        }
        self
    }

    /// Queues a response returned verbatim — several tool calls, usage,
    /// reasoning, all preserved.
    pub fn then_response(self, response: LlmResponse) -> Self {
        self.lock().steps.push_back(Step::Response(response));
        self
    }

    /// Queues an error returned as-is.
    pub fn then_error(self, error: PulseHiveError) -> Self {
        self.lock().steps.push_back(Step::Error(error));
        self
    }

    /// Queues a step that pends until the call's cancel token fires, then
    /// returns the `Cancelled` transport error. **Without a token it pends
    /// forever** — script it only for calls that carry `LlmConfig::cancel`.
    pub fn then_hang(self) -> Self {
        self.lock().steps.push_back(Step::Hang);
        self
    }

    /// The inputs of every call so far, in call order — including calls that
    /// were cancelled or found the script exhausted.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.lock().requests.clone()
    }

    /// How many steps remain in the queue.
    pub fn remaining(&self) -> usize {
        self.lock().steps.len()
    }

    /// Locks the shared state, recovering a poisoned lock rather than
    /// panicking (MASTER-SPEC §9.4).
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// S2 — record the request, then take exactly one step from the front.
    fn record_and_take(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> std::result::Result<Step, PulseHiveError> {
        let mut inner = self.lock();
        inner.requests.push(RecordedRequest {
            messages,
            tools,
            config: config.clone(),
        });
        match inner.steps.pop_front() {
            Some(step) => {
                inner.served += 1;
                Ok(step)
            }
            None => Err(PulseHiveError::llm(format!(
                "script is exhausted: {} call(s) served, no step remains",
                inner.served
            ))),
        }
    }
}

/// A call the provider observed, in call order.
///
/// The type is non-exhaustive and read-only from outside; later fields are
/// additive (ADR-005).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RecordedRequest {
    /// The conversation the caller sent, verbatim.
    pub messages: Vec<Message>,
    /// The tool schemas the caller offered.
    pub tools: Vec<ToolDefinition>,
    /// The per-call config, including its cancel token.
    pub config: LlmConfig,
}

/// S3 — the error every cancelled scripted call returns.
fn cancelled_error() -> PulseHiveError {
    PulseHiveError::llm_transport(
        LlmError::new(LlmErrorKind::Cancelled, "scripted call was cancelled").with_attempts(1),
    )
}

/// Applies the shared post-step semantics: pre-cancelled check, then the step
/// itself. The lock is already released here — it is never held across an
/// await (S4).
async fn resolve_step(step: Step, config: &LlmConfig) -> Result<LlmResponse> {
    if config
        .cancel
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(cancelled_error());
    }
    match step {
        Step::Response(response) => Ok(response),
        Step::Error(error) => Err(error),
        Step::Hang => match config.cancel.as_ref() {
            Some(token) => {
                token.cancelled().await;
                Err(cancelled_error())
            }
            // Documented in `then_hang`: a hang without a token never resolves.
            None => Err(std::future::pending::<PulseHiveError>().await),
        },
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<LlmResponse> {
        let step = self.record_and_take(messages, tools, config)?;
        resolve_step(step, config).await
    }

    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk>> + Send>>> {
        let step = self.record_and_take(messages, tools, config)?;
        let response = resolve_step(step, config).await?;

        // S6 — replay the response as prepared chunks: text (when set), then
        // ToolCallStart + one full-arguments ToolCallDelta per tool call, then
        // Done. Errors, hangs, cancellation and exhaustion were already
        // returned above, from `chat_stream` itself.
        let mut chunks: Vec<Result<LlmChunk>> = Vec::new();
        if let Some(text) = response.content.as_ref() {
            chunks.push(Ok(LlmChunk::Text(text.clone())));
        }
        for call in &response.tool_calls {
            chunks.push(Ok(LlmChunk::ToolCallStart {
                id: call.id.clone(),
                name: call.name.clone(),
            }));
            chunks.push(Ok(LlmChunk::ToolCallDelta {
                id: call.id.clone(),
                arguments_delta: call.arguments.to_string(),
            }));
        }
        chunks.push(Ok(LlmChunk::Done));
        Ok(Box::pin(ChunkStream {
            chunks: chunks.into_iter(),
        }))
    }
}

/// The stream `chat_stream` returns: the prepared chunks, one `Ok` per item,
/// over `futures_core::Stream` (no `futures` dependency).
struct ChunkStream {
    chunks: std::vec::IntoIter<Result<LlmChunk>>,
}

impl Stream for ChunkStream {
    type Item = Result<LlmChunk>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().chunks.next())
    }
}
