//! Contract tests for `pulsehive_core::testing::ScriptedProvider`.
//!
//! Every behaviour comes from the r1.s5 grill decisions S1–S6. Tests that bound
//! a wait use a paused clock with `tokio::time::timeout`, so a regression fails
//! fast instead of hanging (MASTER-SPEC §9.4).

use std::pin::Pin;
use std::time::Duration;

use futures_core::Stream;
use tokio_util::sync::CancellationToken;

use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::llm::{
    LlmChunk, LlmConfig, LlmErrorKind, LlmProvider, LlmResponse, Message, TokenUsage, ToolCall,
    ToolDefinition,
};
use pulsehive_core::testing::ScriptedProvider;

fn config() -> LlmConfig {
    LlmConfig::new("scripted", "test-model")
}

async fn simple_chat(provider: &ScriptedProvider) -> Result<LlmResponse> {
    provider
        .chat(vec![Message::user("hi")], vec![], &config())
        .await
}

/// Drains a `chat_stream` without pulling in a stream-ext crate: `poll_fn`
/// bridges the boxed `futures_core::Stream` one item at a time.
async fn collect_stream(
    mut stream: Pin<Box<dyn Stream<Item = Result<LlmChunk>> + Send>>,
) -> Vec<Result<LlmChunk>> {
    let mut items = Vec::new();
    loop {
        let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        match next {
            Some(item) => items.push(item),
            None => return items,
        }
    }
}

/// S1/S2/L2 — steps replay FIFO; tool-call ids number per provider at script
/// time, whatever steps sit between them.
#[tokio::test]
async fn replays_steps_in_order_with_per_provider_tool_call_ids() {
    let provider = ScriptedProvider::new()
        .then_tool_call("echo", serde_json::json!({"text": "hi"}))
        .then_text("done")
        .then_tool_call("other", serde_json::json!({}));

    let first = simple_chat(&provider).await.unwrap();
    assert_eq!(first.content, None);
    assert_eq!(first.tool_calls.len(), 1);
    assert_eq!(first.tool_calls[0].id, "call_1");
    assert_eq!(first.tool_calls[0].name, "echo");
    assert_eq!(first.finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(first.usage.input_tokens, 0);
    assert_eq!(first.usage.output_tokens, 0);

    let second = simple_chat(&provider).await.unwrap();
    assert_eq!(second.content.as_deref(), Some("done"));
    assert!(second.tool_calls.is_empty());
    assert_eq!(second.finish_reason.as_deref(), Some("stop"));

    let third = simple_chat(&provider).await.unwrap();
    assert_eq!(third.tool_calls.len(), 1);
    assert_eq!(third.tool_calls[0].id, "call_2");
    assert_eq!(third.finish_reason.as_deref(), Some("tool_calls"));
}

/// L2 — `then_response` returns its response verbatim; `then_error` returns its
/// error as-is.
#[tokio::test]
async fn then_response_is_returned_verbatim_and_then_error_as_is() {
    let scripted = LlmResponse::new(
        None,
        vec![ToolCall {
            id: "call_x".into(),
            name: "t".into(),
            arguments: serde_json::json!({"k": 1}),
        }],
        TokenUsage {
            input_tokens: 7,
            output_tokens: 3,
        },
    )
    .with_finish_reason("length")
    .with_reasoning("thought");
    let provider = ScriptedProvider::new()
        .then_response(scripted)
        .then_error(PulseHiveError::llm("boom"));

    let got = simple_chat(&provider).await.unwrap();
    assert_eq!(got.content, None);
    assert_eq!(got.tool_calls.len(), 1);
    assert_eq!(got.tool_calls[0].id, "call_x");
    assert_eq!(got.tool_calls[0].arguments, serde_json::json!({"k": 1}));
    assert_eq!(got.usage.input_tokens, 7);
    assert_eq!(got.usage.output_tokens, 3);
    assert_eq!(got.finish_reason.as_deref(), Some("length"));
    assert_eq!(got.reasoning.as_deref(), Some("thought"));

    match simple_chat(&provider).await {
        Err(PulseHiveError::Llm(message)) => assert_eq!(message, "boom"),
        other => panic!("expected Llm error, got {other:?}"),
    }
}

/// S1 — an exhausted script is a typed error naming the calls served, never a
/// panic and never a default reply; the exhausted call still records (S2).
#[tokio::test]
async fn exhausted_script_errors_naming_calls_served_and_still_records() {
    let provider = ScriptedProvider::new().then_text("only");
    simple_chat(&provider).await.unwrap();

    match simple_chat(&provider).await {
        Err(PulseHiveError::Llm(message)) => {
            assert!(
                message.contains('1'),
                "message should name the calls served: {message}"
            );
        }
        other => panic!("expected Llm error, got {other:?}"),
    }
    assert_eq!(provider.remaining(), 0);
    assert_eq!(provider.requests().len(), 2);
}

/// S4/S5 — every call records messages, tools and config; clones registered
/// elsewhere write to the one log the original reads.
#[tokio::test]
async fn requests_capture_inputs_and_clones_share_one_log() {
    let provider = ScriptedProvider::new().then_text("a").then_text("b");
    let remote = provider.clone();

    let tools = vec![ToolDefinition {
        name: "echo".into(),
        description: "echoes its text argument".into(),
        parameters: serde_json::json!({"type": "object"}),
    }];
    provider
        .chat(vec![Message::user("hi")], tools, &config())
        .await
        .unwrap();
    remote
        .chat(
            vec![Message::system("s")],
            vec![],
            &LlmConfig::new("scripted", "other-model"),
        )
        .await
        .unwrap();

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    match &requests[0].messages[0] {
        Message::User { content } => assert_eq!(content, "hi"),
        other => panic!("expected a user message, got {other:?}"),
    }
    assert_eq!(requests[0].tools.len(), 1);
    assert_eq!(requests[0].tools[0].name, "echo");
    assert_eq!(requests[0].config.model, "test-model");
    assert_eq!(requests[1].config.model, "other-model");
    assert_eq!(remote.requests().len(), 2);
}

/// S3 — a token already cancelled at call start returns `Cancelled` with
/// `attempts == 0` (nothing was sent), consumes its step, and still records
/// the request.
#[tokio::test(start_paused = true)]
async fn pre_cancelled_token_returns_cancelled_and_consumes_its_step() {
    let token = CancellationToken::new();
    token.cancel();
    let provider = ScriptedProvider::new().then_text("a").then_text("b");
    let cancelled_config = config().with_cancel(token);

    let outcome = tokio::time::timeout(
        Duration::from_secs(1_000),
        provider.chat(vec![Message::user("hi")], vec![], &cancelled_config),
    )
    .await;
    match outcome.expect("a pre-cancelled call must resolve immediately") {
        Err(PulseHiveError::LlmTransport(err)) => {
            assert_eq!(err.kind, LlmErrorKind::Cancelled);
            assert_eq!(err.attempts, 0);
        }
        other => panic!("expected Cancelled transport error, got {other:?}"),
    }
    assert_eq!(provider.remaining(), 1);
    assert_eq!(provider.requests().len(), 1);
}

/// S3 — a `then_hang` step pends until its token fires (cancelled from another
/// task), then returns the same `Cancelled` error with `attempts == 1` — the
/// hang models one in-flight request.
#[tokio::test(start_paused = true)]
async fn hang_step_resolves_cancelled_when_another_task_cancels_the_token() {
    let provider = ScriptedProvider::new().then_hang();
    let token = CancellationToken::new();
    let hang_config = config().with_cancel(token.clone());

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
    });

    let outcome = tokio::time::timeout(
        Duration::from_secs(1_000),
        provider.chat(vec![Message::user("hi")], vec![], &hang_config),
    )
    .await;
    match outcome.expect("hang step must resolve once the token fires") {
        Err(PulseHiveError::LlmTransport(err)) => {
            assert_eq!(err.kind, LlmErrorKind::Cancelled);
            assert_eq!(err.attempts, 1);
        }
        other => panic!("expected Cancelled transport error, got {other:?}"),
    }
    assert_eq!(provider.remaining(), 0);
    assert_eq!(provider.requests().len(), 1);
}

/// S3/S6 — on `chat_stream` a token already cancelled at call start yields
/// `Ok(stream)` whose single item is the `Cancelled` error with
/// `attempts == 0`; the step is still consumed and the call still records.
#[tokio::test(start_paused = true)]
async fn chat_stream_pre_cancelled_token_errors_inside_the_stream() {
    let token = CancellationToken::new();
    token.cancel();
    let provider = ScriptedProvider::new().then_text("a").then_text("b");
    let cancelled_config = config().with_cancel(token);

    let stream = provider
        .chat_stream(vec![Message::user("hi")], vec![], &cancelled_config)
        .await;
    let stream = stream.expect("chat_stream must return Ok(stream) for a pre-cancelled call");
    let chunks = tokio::time::timeout(Duration::from_secs(1_000), collect_stream(stream))
        .await
        .expect("a pre-cancelled stream must resolve immediately");
    assert_eq!(chunks.len(), 1);
    match &chunks[0] {
        Err(PulseHiveError::LlmTransport(err)) => {
            assert_eq!(err.kind, LlmErrorKind::Cancelled);
            assert_eq!(err.attempts, 0);
        }
        other => panic!("expected Cancelled transport error item, got {other:?}"),
    }
    assert_eq!(provider.remaining(), 1);
    assert_eq!(provider.requests().len(), 1);
}

/// S6 — a `then_hang` step on `chat_stream` pends inside the stream until
/// its token fires (cancelled from another task), then yields exactly one
/// `Cancelled` item with `attempts == 1`.
#[tokio::test(start_paused = true)]
async fn chat_stream_hang_step_errors_in_stream_when_token_fires() {
    let provider = ScriptedProvider::new().then_hang();
    let token = CancellationToken::new();
    let hang_config = config().with_cancel(token.clone());

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
    });

    let stream = provider
        .chat_stream(vec![Message::user("hi")], vec![], &hang_config)
        .await
        .expect("chat_stream must return Ok(stream) for a hang step");
    let chunks = tokio::time::timeout(Duration::from_secs(1_000), collect_stream(stream))
        .await
        .expect("hang step must resolve once the token fires");
    assert_eq!(chunks.len(), 1);
    match &chunks[0] {
        Err(PulseHiveError::LlmTransport(err)) => {
            assert_eq!(err.kind, LlmErrorKind::Cancelled);
            assert_eq!(err.attempts, 1);
        }
        other => panic!("expected Cancelled transport error item, got {other:?}"),
    }
    assert_eq!(provider.remaining(), 0);
    assert_eq!(provider.requests().len(), 1);
}

/// S6 — `then_error` and script exhaustion are call-level errors on
/// `chat_stream`, like a pre-stream request failure on `pulsehive-openai`;
/// both calls still record.
#[tokio::test]
async fn chat_stream_then_error_and_exhaustion_fail_the_call_itself() {
    let provider = ScriptedProvider::new().then_error(PulseHiveError::llm("boom"));

    match provider
        .chat_stream(vec![Message::user("hi")], vec![], &config())
        .await
    {
        Err(PulseHiveError::Llm(message)) => assert_eq!(message, "boom"),
        Ok(_) => panic!("expected call-level Llm error, got Ok(stream)"),
        Err(other) => panic!("expected call-level Llm error, got {other:?}"),
    }

    match provider
        .chat_stream(vec![Message::user("hi")], vec![], &config())
        .await
    {
        Err(PulseHiveError::Llm(message)) => {
            assert!(
                message.contains('1'),
                "message should name the calls served: {message}"
            );
        }
        Ok(_) => panic!("expected call-level exhaustion error, got Ok(stream)"),
        Err(other) => panic!("expected call-level exhaustion error, got {other:?}"),
    }
    assert_eq!(provider.requests().len(), 2);
}

/// S6 — `chat_stream` takes the same step and replays `Text` (when content is
/// set), `ToolCallStart` plus one full-arguments `ToolCallDelta` per tool call,
/// then `Done`, each item `Ok`.
#[tokio::test]
async fn chat_stream_replays_prepared_chunks_in_order() {
    let provider = ScriptedProvider::new()
        .then_response(LlmResponse::text("ans").with_tool_calls(vec![ToolCall {
            id: "call_9".into(),
            name: "echo".into(),
            arguments: serde_json::json!({"a": 1}),
        }]))
        .then_tool_call("echo", serde_json::json!({"b": 2}));

    let stream = provider
        .chat_stream(vec![Message::user("hi")], vec![], &config())
        .await
        .unwrap();
    let chunks = collect_stream(stream).await;
    assert_eq!(chunks.len(), 4);
    match &chunks[0] {
        Ok(LlmChunk::Text(text)) => assert_eq!(text, "ans"),
        other => panic!("expected Text chunk, got {other:?}"),
    }
    match &chunks[1] {
        Ok(LlmChunk::ToolCallStart { id, name }) => {
            assert_eq!(id, "call_9");
            assert_eq!(name, "echo");
        }
        other => panic!("expected ToolCallStart chunk, got {other:?}"),
    }
    match &chunks[2] {
        Ok(LlmChunk::ToolCallDelta {
            id,
            arguments_delta,
        }) => {
            assert_eq!(id, "call_9");
            assert_eq!(arguments_delta, "{\"a\":1}");
        }
        other => panic!("expected ToolCallDelta chunk, got {other:?}"),
    }
    assert!(matches!(chunks[3], Ok(LlmChunk::Done)));

    // A step with no content emits no Text chunk.
    let stream = provider
        .chat_stream(vec![Message::user("again")], vec![], &config())
        .await
        .unwrap();
    let chunks = collect_stream(stream).await;
    assert_eq!(chunks.len(), 3);
    match &chunks[0] {
        Ok(LlmChunk::ToolCallStart { id, name }) => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "echo");
        }
        other => panic!("expected ToolCallStart chunk, got {other:?}"),
    }
    match &chunks[1] {
        Ok(LlmChunk::ToolCallDelta {
            id,
            arguments_delta,
        }) => {
            assert_eq!(id, "call_1");
            assert_eq!(arguments_delta, "{\"b\":2}");
        }
        other => panic!("expected ToolCallDelta chunk, got {other:?}"),
    }
    assert!(matches!(chunks[2], Ok(LlmChunk::Done)));
}

/// L2 — `remaining()` counts down as steps are served, and the provider is
/// usable as `Box<dyn LlmProvider>` (ADR-010).
#[tokio::test]
async fn remaining_counts_down_and_provider_works_boxed() {
    let provider = ScriptedProvider::new().then_text("a").then_text("b");
    assert_eq!(provider.remaining(), 2);

    let boxed: Box<dyn LlmProvider> = Box::new(provider.clone());
    let response = boxed
        .chat(vec![Message::user("hi")], vec![], &config())
        .await
        .unwrap();
    assert_eq!(response.content.as_deref(), Some("a"));
    assert_eq!(provider.remaining(), 1);

    simple_chat(&provider).await.unwrap();
    assert_eq!(provider.remaining(), 0);
}
