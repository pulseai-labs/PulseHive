//! Test doubles for driving PulseHive agents deterministically, offline.
//!
//! [`ScriptedProvider`] is an [`LlmProvider`] whose
//! replies are queued in advance with a `then_*` builder. A test scripts the
//! conversation it wants, hands a clone to `HiveMindBuilder` (providers are
//! taken by value), drives the agent, and reads back what the agent sent.
//! No API key and no network are involved — the provider is the product-owned
//! swappable interface, so the turn it drives is the real agent loop.
//!
//! Sketch of the consumer flow (the `testing` feature also exposes this
//! module through the meta-crate as `pulsehive::testing`; the paths below are
//! the meta-crate's — swap the `pulsehive::` prefix for `pulsehive_core::`
//! when depending on the core crate directly):
//!
//! ```rust,ignore
//! use pulsehive::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
//! use pulsehive::llm::LlmConfig;
//! use pulsehive::testing::ScriptedProvider;
//! use pulsehive::{HiveMind, Task};
//!
//! let provider = ScriptedProvider::new()
//!     .then_tool_call("echo", json!({"text": "hi"}))
//!     .then_text("done");
//! let hive = HiveMind::builder()
//!     .substrate_path(dir.path().join("test.db"))
//!     .llm_provider("scripted", provider.clone())
//!     .build()?;
//! // Deploy an AgentKind::Llm agent whose llm_config routes to "scripted",
//! // drain the deploy stream to HiveEvent::AgentCompleted, then assert on
//! // what the provider saw:
//! assert_eq!(provider.requests().len(), 2); // the tool result went back in
//! ```
//!
//! The complete, compiling version — tool definition, agent assembly, event
//! drain with a hang guard, and the assertions on the recorded requests — is
//! the integration test `pulsehive/tests/scripted_agent_turn.rs` (behind the
//! meta-crate's `testing` feature). It is the source of truth for every
//! import path and builder call this sketch abbreviates.
//!
//! One runtime interaction to know before scripting long turns: when insight
//! synthesis is enabled (the `HiveMind` builder's default),
//! `record_experience` consumes one extra completion per synthesis from the
//! first registered provider — see the note on [`ScriptedProvider`].

use std::collections::VecDeque;
use std::future::Future;
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
/// with kind [`Cancelled`](crate::llm::LlmErrorKind::Cancelled) — with
/// `attempts == 0` in the first case (nothing was sent, per
/// [`LlmError::attempts`](crate::llm::LlmError::attempts)) and `attempts == 1`
/// in the second (the hang models one in-flight request).
///
/// **Insight synthesis is an extra consumer.** `HiveMind`'s builder enables
/// insight synthesis by default (`InsightSynthesizer::with_defaults()`,
/// relation-density threshold 5), and `record_experience` — the Record phase
/// of every agent turn — issues one additional `chat` completion per
/// synthesis once a cluster crosses that threshold. It takes the first
/// registered provider in arbitrary `HashMap` order and a fresh synthesis
/// config (`LlmConfig::new(provider_name, "default")`, no cancel token), so
/// against a `ScriptedProvider` the call consumes a scripted step and lands
/// in `requests()` even when the synthesizer discards the outcome (a queued
/// text reply becomes the insight; script exhaustion or a `then_error`
/// yields no insight — the step is spent either way). A test that crosses
/// the threshold should either script the extra steps or disable synthesis
/// with `HiveMindBuilder::no_insight_synthesizer()` (or install a custom
/// synthesizer via `HiveMindBuilder::insight_synthesizer`).
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
    /// reasoning, all preserved. A token already cancelled when the call
    /// starts pre-empts this outcome: the step is still consumed and the
    /// `Cancelled` transport error (`attempts == 0`) is returned instead
    /// (on `chat_stream`, as the stream's single `Err` item).
    pub fn then_response(self, response: LlmResponse) -> Self {
        self.lock().steps.push_back(Step::Response(response));
        self
    }

    /// Queues an error returned as-is. A token already cancelled when the
    /// call starts pre-empts this outcome: the step is still consumed and the
    /// `Cancelled` transport error (`attempts == 0`) is returned instead (on
    /// `chat_stream`, as the stream's single `Err` item).
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

/// S3 — the error every cancelled scripted call returns. `attempts` follows
/// the transport contract ([`LlmError::attempts`](crate::llm::LlmError)):
/// `0` when the token was already cancelled at call start — nothing was
/// sent — and `1` when a `then_hang` step's token fired mid-flight, the hang
/// modelling one in-flight request.
fn cancelled_error(attempts: u32) -> PulseHiveError {
    PulseHiveError::llm_transport(
        LlmError::new(LlmErrorKind::Cancelled, "scripted call was cancelled")
            .with_attempts(attempts),
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
        return Err(cancelled_error(0));
    }
    match step {
        Step::Response(response) => Ok(response),
        Step::Error(error) => Err(error),
        Step::Hang => match config.cancel.as_ref() {
            Some(token) => {
                token.cancelled().await;
                Err(cancelled_error(1))
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
        // S2 — record, then take exactly one step. Exhaustion stays a
        // call-level error, like a request that cannot be built on
        // `pulsehive-openai`'s streaming path.
        let step = self.record_and_take(messages, tools, config)?;

        // S6 — cancellation is delivered in the stream, not by the call: like
        // `pulsehive-openai`'s `chat_stream`, the call itself returns
        // `Ok(stream)` and a cancellation surfaces as exactly one `Err` item
        // — an already-cancelled token immediately (`attempts == 0`), a
        // `then_hang` step when its token fires (`attempts == 1`). A queued
        // `then_error` stays a call-level error, matching that provider's
        // pre-stream failure path, where a request that fails before the
        // stream starts fails the call.
        if config
            .cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Ok(Box::pin(ScriptedStream::error(cancelled_error(0))));
        }
        match step {
            Step::Error(error) => Err(error),
            Step::Response(response) => Ok(Box::pin(ScriptedStream::replay(response))),
            Step::Hang => match config.cancel.clone() {
                Some(token) => Ok(Box::pin(ScriptedStream::await_cancel(token))),
                // Documented in `then_hang`: a hang without a token never
                // resolves.
                None => Ok(Box::pin(ScriptedStream::hang_forever())),
            },
        }
    }
}

/// The stream `chat_stream` returns, over `futures_core::Stream` (no
/// `futures` dependency): either the prepared chunks — text (when set), then
/// `ToolCallStart` + one full-arguments `ToolCallDelta` per tool call, then
/// `Done`, each an `Ok` item — or a pending cancellation that resolves to
/// exactly one `Err` item.
enum ScriptedStream {
    /// Prepared chunks, one `Ok` item each.
    Replay(std::vec::IntoIter<Result<LlmChunk>>),
    /// Pends until the cancel token fires, then yields one `Err` item — the
    /// shape `pulsehive-openai`'s `chat_stream` gives a cancelled in-flight
    /// request. `done` latches after that item: the underlying wait future
    /// cannot be polled again, and every later poll ends the stream.
    AwaitCancel {
        wait: Pin<Box<dyn Future<Output = ()> + Send>>,
        done: bool,
    },
}

impl ScriptedStream {
    /// A stream that replays a response as prepared chunks: text (when set),
    /// then `ToolCallStart` + one full-arguments `ToolCallDelta` per tool
    /// call, then `Done`.
    fn replay(response: LlmResponse) -> Self {
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
        Self::Replay(chunks.into_iter())
    }

    /// A stream whose single item is `error` — a cancellation already
    /// latched when the call returned.
    fn error(error: PulseHiveError) -> Self {
        Self::Replay(vec![Err(error)].into_iter())
    }

    /// A stream that pends until `token` fires, then yields the mid-flight
    /// `Cancelled` error (`attempts == 1`).
    fn await_cancel(token: CancellationToken) -> Self {
        Self::AwaitCancel {
            wait: Box::pin(async move {
                token.cancelled().await;
            }),
            done: false,
        }
    }

    /// A stream that never yields — `then_hang` without a token.
    fn hang_forever() -> Self {
        Self::AwaitCancel {
            wait: Box::pin(std::future::pending::<()>()),
            done: false,
        }
    }
}

impl Stream for ScriptedStream {
    type Item = Result<LlmChunk>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Replay(chunks) => Poll::Ready(chunks.next()),
            // A hang models one in-flight request, so its cancellation
            // reports one attempt (S3).
            Self::AwaitCancel { wait, done } => {
                if *done {
                    return Poll::Ready(None);
                }
                match wait.as_mut().poll(cx) {
                    // Latch after the one error item; the wait future cannot
                    // be polled past completion.
                    Poll::Ready(()) => {
                        *done = true;
                        Poll::Ready(Some(Err(cancelled_error(1))))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}
