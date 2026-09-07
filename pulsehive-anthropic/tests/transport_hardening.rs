//! Hermetic transport-hardening suite for `AnthropicProvider` (spec r1.s1.w3
//! §4.6; ledger line d5).
//!
//! Every test runs against a loopback-only `std::net::TcpListener` fixture
//! that scripts each response, records every raw request body on a channel,
//! and asserts through the public surface only. No network beyond loopback,
//! no wall clock beyond the bounded timeouts under test.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use pulsehive_anthropic::{AnthropicConfig, AnthropicProvider};
use pulsehive_core::error::PulseHiveError;
use pulsehive_core::llm::{
    LlmConfig, LlmError, LlmErrorKind, LlmProvider, Message, ReasoningEffort, ToolChoice,
};
use tokio_util::sync::CancellationToken;

// ── Scripted fixtures ────────────────────────────────────────────────

/// What the scripted server does once it has read one request.
#[derive(Clone)]
enum Script {
    /// Reply with a status line, optional `Retry-After`, and a body, then close.
    Respond {
        status: u16,
        reason: &'static str,
        retry_after: Option<u64>,
        body: String,
    },
    /// Read the request, hold the connection `secs` without replying, then
    /// answer with a text fixture (the client's timeout should fire first).
    Stall { secs: u64 },
    /// Read the request, then block until the client disconnects.
    StallUntilDisconnect,
    /// Send a full response head advertising `advertised_len` body bytes,
    /// write only `body`, then drop the connection mid-body: the client sees
    /// a body shorter than `Content-Length`.
    RespondTruncated {
        status: u16,
        reason: &'static str,
        advertised_len: usize,
        body: String,
    },
    /// Send a full response head advertising a body, then never finish the
    /// body: block until the client disconnects (its timeout fires first).
    HeadersThenStall {
        status: u16,
        reason: &'static str,
        advertised_len: usize,
    },
}

const TEXT_FIXTURE: &str = concat!(
    r#"{"id":"msg_fix","content":[{"type":"text","text":"Hi there"}],"#,
    r#""stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}"#
);

const MAX_TOKENS_FIXTURE: &str = concat!(
    r#"{"id":"msg_max","content":[{"type":"text","text":"stopped early"}],"#,
    r#""stop_reason":"max_tokens","usage":{"input_tokens":3,"output_tokens":9}}"#
);

const TOOL_USE_FIXTURE: &str = concat!(
    r#"{"id":"msg_tool","content":[{"type":"tool_use","id":"toolu_1","#,
    r#""name":"search","input":{"query":"rust"}}],"#,
    r#""stop_reason":"tool_use","usage":null}"#
);

const MALFORMED_TOOL_USE_FIXTURE: &str = concat!(
    r#"{"id":"msg_bad","content":[{"type":"tool_use","id":"toolu_2","#,
    r#""name":"search","input":"oops"}],"#,
    r#""stop_reason":"tool_use","usage":null}"#
);

fn ok_text() -> Script {
    respond(200, "OK", None, TEXT_FIXTURE)
}

fn respond(status: u16, reason: &'static str, retry_after: Option<u64>, body: &str) -> Script {
    Script::Respond {
        status,
        reason,
        retry_after,
        body: body.to_string(),
    }
}

/// Spawn a scripted HTTP/1.1 server on `127.0.0.1:0`; returns its base URL
/// and the channel every recorded request body is sent on. Each connection
/// serves exactly one request (`Connection: close`), so one attempt equals
/// one recorded body. When the script runs out, later requests get a 200
/// text fixture (visible as an unexpected request count).
fn spawn_server(scripts: Vec<Script>) -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture: bind loopback");
    let addr = listener.local_addr().expect("fixture: local addr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut next = 0usize;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let body = read_http_request(&mut stream);
            let _ = tx.send(body);
            let script = scripts.get(next).cloned().unwrap_or_else(ok_text);
            next += 1;
            match script {
                Script::Respond {
                    status,
                    reason,
                    retry_after,
                    body,
                } => write_response(&mut stream, status, reason, retry_after, &body),
                Script::Stall { secs } => {
                    thread::sleep(Duration::from_secs(secs));
                    write_response(&mut stream, 200, "OK", None, TEXT_FIXTURE);
                }
                Script::StallUntilDisconnect => {
                    let mut sink = [0u8; 64];
                    loop {
                        match stream.read(&mut sink) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {}
                        }
                    }
                }
                Script::RespondTruncated {
                    status,
                    reason,
                    advertised_len,
                    body,
                } => {
                    let head = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
                         Content-Length: {advertised_len}\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body.as_bytes());
                    let _ = stream.flush();
                    // Dropping the stream closes the connection with the
                    // body shorter than advertised.
                }
                Script::HeadersThenStall {
                    status,
                    reason,
                    advertised_len,
                } => {
                    let head = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
                         Content-Length: {advertised_len}\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.flush();
                    let mut sink = [0u8; 64];
                    loop {
                        match stream.read(&mut sink) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {}
                        }
                    }
                }
            }
        }
    });
    (format!("http://{addr}"), rx)
}

/// Read one HTTP/1.1 request head plus its `Content-Length` body and return
/// the raw body bytes as a string.
fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk).expect("fixture: read request head");
        assert!(
            n > 0,
            "fixture: client disconnected before sending a request"
        );
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let content_length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while buf.len() < head_end + content_length {
        let n = stream.read(&mut chunk).expect("fixture: read request body");
        assert!(
            n > 0,
            "fixture: client disconnected before sending the full body"
        );
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&buf[head_end..head_end + content_length]).into_owned()
}

fn write_response(
    stream: &mut std::net::TcpStream,
    status: u16,
    reason: &str,
    retry_after: Option<u64>,
    body: &str,
) {
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(secs) = retry_after {
        response.push_str(&format!("Retry-After: {secs}\r\n"));
    }
    response.push_str("Connection: close\r\n\r\n");
    response.push_str(body);
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// ── Test helpers ─────────────────────────────────────────────────────

/// A config against the fixture server with bounded, per-test knobs. Tests
/// that need a different timeout reassign the pub field before building the
/// provider.
fn base_config(base_url: &str, max_retries: u32) -> AnthropicConfig {
    let mut config = AnthropicConfig::new("test-key").with_base_url(base_url);
    config.timeout_secs = 5;
    config.max_retries = max_retries;
    config
}

/// Every request body recorded within `wait_ms` (first wait, then drain).
fn requests_seen(rx: &Receiver<String>, wait_ms: u64) -> Vec<String> {
    let mut seen = Vec::new();
    while let Ok(body) = rx.recv_timeout(Duration::from_millis(wait_ms)) {
        seen.push(body);
    }
    seen
}

/// Unwrap the typed transport error every failing test expects.
fn transport_error(error: PulseHiveError) -> LlmError {
    match error {
        PulseHiveError::LlmTransport(err) => err,
        other => panic!("expected LlmTransport error, got: {other:?}"),
    }
}

fn chat_config() -> LlmConfig {
    LlmConfig::new("anthropic", "claude-sonnet-4-6")
}

fn one_user_message() -> Vec<Message> {
    vec![Message::user("hello")]
}

/// Elapsed time is "about one second": at least the timeout, well under two.
fn assert_about_one_second(elapsed: Duration) {
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(2500),
        "expected ~1s, got {elapsed:?}"
    );
}

// ── 1. Timeout fails once, typed, never re-sent (E5) ─────────────────

#[tokio::test]
async fn timeout_fails_once_with_typed_timeout_and_is_not_resent() {
    let (base, rx) = spawn_server(vec![Script::Stall { secs: 3 }]);
    let mut config = base_config(&base, 3);
    config.timeout_secs = 1;
    let provider = AnthropicProvider::with_config(config);

    let start = Instant::now();
    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("stalled server must time out");
    let elapsed = start.elapsed();

    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::Timeout);
    assert_eq!(err.attempts, 1);
    assert_about_one_second(elapsed);

    // A timeout is never re-sent: a further second passes and the server has
    // still seen exactly one request.
    thread::sleep(Duration::from_secs(1));
    assert_eq!(requests_seen(&rx, 200).len(), 1);
}

// ── 2. Connection errors retry within the budget, then type as Connect ─

#[tokio::test]
async fn connect_error_is_retried_then_typed() {
    // Bind, capture the address, drop: connecting there is refused.
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture: bind loopback");
    let addr = listener.local_addr().expect("fixture: local addr");
    drop(listener);

    let provider = AnthropicProvider::with_config(base_config(&format!("http://{addr}"), 1));

    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("closed port must fail");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::Connect);
    assert_eq!(err.attempts, 2);
}

// ── 3. Per-call max_retries override wins on one instance (E12) ──────

#[tokio::test]
async fn per_call_max_retries_override_wins_on_one_instance() {
    let (base, rx) = spawn_server(vec![
        respond(529, "Overloaded", Some(0), "{}"),
        ok_text(),
        respond(529, "Overloaded", Some(0), "{}"),
    ]);
    let provider = AnthropicProvider::with_config(base_config(&base, 0));

    // Call A: per-call max_retries 1 retries the 529 and succeeds on the 200.
    let outcome = provider
        .chat(
            one_user_message(),
            vec![],
            &chat_config().with_max_retries(1),
        )
        .await;
    assert!(outcome.is_ok(), "call A should succeed: {outcome:?}");
    assert_eq!(requests_seen(&rx, 100).len(), 2);

    // Call B on the same provider, override unset: one 529, no retry.
    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("call B should exhaust at one attempt");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::ServerError);
    assert_eq!(err.attempts, 1);
    assert_eq!(err.status, Some(529));
}

// ── 3b. A u32::MAX retry budget still sends at least once (F09) ──────

#[tokio::test]
async fn max_retries_at_u32_max_does_not_overflow_to_zero_attempts() {
    // `LlmConfig` is an unvalidated Deserialize; a budget of u32::MAX must
    // saturate, not overflow (debug panic) or wrap to zero attempts. A 400
    // fails on the first attempt, which the wrapped budget could never
    // reach.
    let (base, rx) = spawn_server(vec![respond(
        400,
        "Bad Request",
        None,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#,
    )]);
    let mut config = base_config(&base, u32::MAX);
    config.timeout_secs = 5;
    let provider = AnthropicProvider::with_config(config);

    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("400 must fail");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::ClientError);
    assert_eq!(err.attempts, 1, "attempts must be at least 1");
    assert_eq!(requests_seen(&rx, 100).len(), 1);
}

// ── 4. Per-call timeout overrides a long client timeout ───────────────

#[tokio::test]
async fn per_call_timeout_override_shortens_a_long_client_timeout() {
    let (base, _rx) = spawn_server(vec![Script::Stall { secs: 3 }]);
    let mut config = base_config(&base, 0);
    config.timeout_secs = 30;
    let provider = AnthropicProvider::with_config(config);

    let start = Instant::now();
    let error = provider
        .chat(
            one_user_message(),
            vec![],
            &chat_config().with_timeout_secs(1),
        )
        .await
        .expect_err("per-call timeout must fire");
    let elapsed = start.elapsed();

    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::Timeout);
    assert_eq!(err.attempts, 1);
    assert_about_one_second(elapsed);
}

// ── 5. Rate limit carries Retry-After, status, and raw body ──────────

#[tokio::test]
async fn rate_limit_carries_retry_after() {
    let rate_body = "rate-limit body";
    let (base, rx) = spawn_server(vec![
        respond(429, "Too Many Requests", Some(0), rate_body),
        respond(429, "Too Many Requests", Some(0), rate_body),
    ]);
    let provider = AnthropicProvider::with_config(base_config(&base, 1));

    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("two 429s must exhaust the budget");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::RateLimited);
    assert_eq!(err.attempts, 2);
    assert_eq!(err.status, Some(429));
    assert_eq!(err.retry_after, Some(Duration::ZERO));
    assert_eq!(err.body.as_deref(), Some(rate_body));
    assert_eq!(requests_seen(&rx, 100).len(), 2);
}

// ── 5b. Retry-After honored on 429/529 only; raw value reported ──────

#[tokio::test]
async fn retry_after_on_500_family_is_not_honored() {
    // A 500 with Retry-After: 0 retries on the exponential backoff (1s),
    // not the header, and the error does not carry retry_after.
    let (base, rx) = spawn_server(vec![
        respond(500, "Internal Server Error", Some(0), "{}"),
        respond(500, "Internal Server Error", Some(0), "{}"),
    ]);
    let provider = AnthropicProvider::with_config(base_config(&base, 1));

    let start = Instant::now();
    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("two 500s must exhaust the budget");
    let elapsed = start.elapsed();

    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::ServerError);
    assert_eq!(err.attempts, 2);
    assert_eq!(err.retry_after, None, "500 does not honor Retry-After");
    assert!(
        elapsed >= Duration::from_millis(900),
        "500 must use the 1s backoff, not Retry-After: 0 (took {elapsed:?})"
    );
    assert_eq!(requests_seen(&rx, 100).len(), 2);
}

#[tokio::test]
async fn oversized_retry_after_is_reported_verbatim_but_never_slept() {
    // Retry-After: 100000 (~27.8h) on a 429: the error carries the raw value
    // for the caller; the honored sleep is capped (unit-tested) and a
    // zero-budget call does not sleep at all.
    let (base, _rx) = spawn_server(vec![respond(
        429,
        "Too Many Requests",
        Some(100_000),
        "rate-limit body",
    )]);
    let provider = AnthropicProvider::with_config(base_config(&base, 0));

    let start = Instant::now();
    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("one 429 with a zero budget must fail");
    let elapsed = start.elapsed();

    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::RateLimited);
    assert_eq!(err.retry_after, Some(Duration::from_secs(100_000)));
    assert!(
        elapsed < Duration::from_millis(2500),
        "a zero-budget call must not sleep for the header value, took {elapsed:?}"
    );
}

// ── 6. Other 4xx: immediate, envelope message, raw body ──────────────

#[tokio::test]
async fn client_error_is_immediate_with_anthropic_message_and_body() {
    let envelope = r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#;
    let (base, rx) = spawn_server(vec![respond(400, "Bad Request", None, envelope)]);
    let provider = AnthropicProvider::with_config(base_config(&base, 3));

    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("400 must fail");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::ClientError);
    assert_eq!(err.attempts, 1);
    assert_eq!(err.status, Some(400));
    assert!(err.message.contains("bad"), "message was {:?}", err.message);
    assert_eq!(err.body.as_deref(), Some(envelope));
    assert_eq!(requests_seen(&rx, 100).len(), 1);
}

// ── 7. Success status with unparseable body is a Parse error ─────────

#[tokio::test]
async fn unparseable_success_body_is_parse_error() {
    let (base, _rx) = spawn_server(vec![respond(200, "OK", None, "not json")]);
    let provider = AnthropicProvider::with_config(base_config(&base, 1));

    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("non-JSON body must fail");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::Parse);
    assert_eq!(err.status, Some(200));
    assert_eq!(err.body.as_deref(), Some("not json"));
}

// ── 8. stop_reason is finish_reason verbatim; reasoning stays None (E3) ─

#[tokio::test]
async fn stop_reason_is_finish_reason_verbatim_and_reasoning_is_none() {
    let (base, _rx) = spawn_server(vec![
        respond(200, "OK", None, MAX_TOKENS_FIXTURE),
        respond(200, "OK", None, TOOL_USE_FIXTURE),
    ]);
    let provider = AnthropicProvider::with_config(base_config(&base, 0));

    let text_response = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect("text fixture must parse");
    assert_eq!(text_response.finish_reason.as_deref(), Some("max_tokens"));
    assert!(text_response.reasoning.is_none());

    let tool_response = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect("tool_use fixture must parse");
    assert_eq!(tool_response.finish_reason.as_deref(), Some("tool_use"));
    assert!(tool_response.reasoning.is_none());
    assert_eq!(tool_response.tool_calls.len(), 1);
    assert_eq!(
        tool_response.tool_calls[0].arguments,
        serde_json::json!({"query": "rust"})
    );
}

// ── 9. Non-object tool_use input is a typed MalformedToolCall (E6) ────

#[tokio::test]
async fn non_object_tool_use_input_is_malformed_tool_call() {
    let (base, _rx) = spawn_server(vec![respond(200, "OK", None, MALFORMED_TOOL_USE_FIXTURE)]);
    let provider = AnthropicProvider::with_config(base_config(&base, 0));

    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("string tool_use input must fail");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::MalformedToolCall);
    assert_eq!(err.finish_reason.as_deref(), Some("tool_use"));
    assert_eq!(err.body.as_deref(), Some("\"oops\""));
}

// ── 10. tool_choice maps to Anthropic wire shapes only when set ──────

#[tokio::test]
async fn tool_choice_maps_to_anthropic_wire_shapes_only_when_set() {
    let (base, rx) = spawn_server(vec![ok_text(); 6]);
    let provider = AnthropicProvider::with_config(base_config(&base, 0));

    // tool_choice is only sent when the request carries tools, so every
    // mapping case sends one.
    let one_tool = vec![pulsehive_core::llm::ToolDefinition {
        name: "search".into(),
        description: "Search the web".into(),
        parameters: serde_json::json!({"type": "object"}),
    }];

    let cases: Vec<(ToolChoice, &str)> = vec![
        (ToolChoice::Auto, r#"{"type":"auto"}"#),
        (ToolChoice::Required, r#"{"type":"any"}"#),
        (
            ToolChoice::Function {
                name: "f".to_string(),
            },
            r#"{"type":"tool","name":"f"}"#,
        ),
        (ToolChoice::None, r#"{"type":"none"}"#),
    ];
    for (choice, wire) in cases {
        let outcome = provider
            .chat(
                one_user_message(),
                one_tool.clone(),
                &chat_config().with_tool_choice(choice),
            )
            .await;
        assert!(
            outcome.is_ok(),
            "tool_choice case {wire} failed: {outcome:?}"
        );
        let body = rx
            .recv_timeout(Duration::from_millis(500))
            .expect("request body recorded");
        let expected = format!("\"tool_choice\":{wire}");
        assert!(
            body.contains(&expected),
            "body {body:?} must contain {expected}"
        );
    }

    // Unset: the key is absent entirely.
    let outcome = provider
        .chat(one_user_message(), one_tool.clone(), &chat_config())
        .await;
    assert!(outcome.is_ok());
    let body = rx
        .recv_timeout(Duration::from_millis(500))
        .expect("unset-case body recorded");
    assert!(
        !body.contains("tool_choice"),
        "body must not contain tool_choice: {body:?}"
    );

    // Set but no tools on the request: the Messages API rejects
    // tool_choice-without-tools, so the provider omits the key.
    let outcome = provider
        .chat(
            one_user_message(),
            vec![],
            &chat_config().with_tool_choice(ToolChoice::Required),
        )
        .await;
    assert!(
        outcome.is_ok(),
        "tool_choice with no tools must not break the call: {outcome:?}"
    );
    let body = rx
        .recv_timeout(Duration::from_millis(500))
        .expect("no-tools-case body recorded");
    assert!(
        !body.contains("tool_choice"),
        "body must not contain tool_choice when tools is empty: {body:?}"
    );
}

// ── 11. reasoning_effort is accepted and never sent (#47 R4) ──────────

#[tokio::test]
async fn reasoning_effort_is_accepted_and_not_sent() {
    let (base, rx) = spawn_server(vec![ok_text()]);
    let provider = AnthropicProvider::with_config(base_config(&base, 0));

    let outcome = provider
        .chat(
            one_user_message(),
            vec![],
            &chat_config().with_reasoning_effort(ReasoningEffort::Low),
        )
        .await;
    assert!(outcome.is_ok(), "call must succeed: {outcome:?}");

    let body = rx
        .recv_timeout(Duration::from_millis(500))
        .expect("request body recorded");
    assert!(
        !body.contains("reasoning_effort"),
        "reasoning_effort must not be sent: {body:?}"
    );
    assert!(
        !body.contains("thinking"),
        "thinking must not be sent: {body:?}"
    );
}

// ── 12. A cancelled token aborts the in-flight request (L3) ───────────

#[tokio::test]
async fn cancel_token_aborts_in_flight_request() {
    let (base, rx) = spawn_server(vec![Script::StallUntilDisconnect]);
    let provider = AnthropicProvider::with_config(base_config(&base, 0));

    // Cancel from a spawned task 100 ms into a stalled request.
    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });

    let start = Instant::now();
    let error = provider
        .chat(
            one_user_message(),
            vec![],
            &chat_config().with_cancel(token),
        )
        .await
        .expect_err("cancelled call must fail");
    let elapsed = start.elapsed();

    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::Cancelled);
    assert_eq!(err.attempts, 1);
    // A generous window: the canceller fires at 100ms, but scheduler
    // contention on a loaded CI runner can delay the observed return well
    // past a tight bound.
    assert!(
        elapsed < Duration::from_millis(2500),
        "cancel must return promptly, took {elapsed:?}"
    );

    // A token cancelled before the call never sends a request.
    let pre_cancelled = CancellationToken::new();
    pre_cancelled.cancel();
    let error = provider
        .chat(
            one_user_message(),
            vec![],
            &chat_config().with_cancel(pre_cancelled),
        )
        .await
        .expect_err("pre-cancelled call must fail");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::Cancelled);
    assert_eq!(err.attempts, 0);

    assert_eq!(requests_seen(&rx, 200).len(), 1);
}

// ── 13. Success status, body dropped mid-stream => Parse, no retry ────

#[tokio::test]
async fn success_body_dropped_mid_stream_is_parse_error() {
    let (base, rx) = spawn_server(vec![Script::RespondTruncated {
        status: 200,
        reason: "OK",
        advertised_len: 1024,
        body: r#"{"id":"msg_cut","content":[{"type":"te"#.to_string(),
    }]);
    let provider = AnthropicProvider::with_config(base_config(&base, 0));

    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("truncated body must fail");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::Parse);
    assert_eq!(err.status, Some(200));
    assert_eq!(err.attempts, 1);
    assert!(
        err.body.is_none(),
        "no partial body is recoverable from a dropped read"
    );
    assert_eq!(requests_seen(&rx, 200).len(), 1);
}

// ── 14. Success status, body stalls past the timeout => Timeout, no retry ─

#[tokio::test]
async fn success_body_stall_past_timeout_is_timeout_not_retried() {
    let (base, rx) = spawn_server(vec![Script::HeadersThenStall {
        status: 200,
        reason: "OK",
        advertised_len: 1024,
    }]);
    let mut config = base_config(&base, 3);
    config.timeout_secs = 1;
    let provider = AnthropicProvider::with_config(config);

    let start = Instant::now();
    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("stalled body must time out");
    let elapsed = start.elapsed();

    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::Timeout);
    assert_eq!(err.attempts, 1);
    assert_about_one_second(elapsed);
    assert_eq!(requests_seen(&rx, 200).len(), 1);
}

// ── 15. Error status, body dropped mid-stream => Connect, retried ─────

#[tokio::test]
async fn error_body_dropped_mid_stream_is_connect_and_retries() {
    let (base, rx) = spawn_server(vec![
        Script::RespondTruncated {
            status: 503,
            reason: "Service Unavailable",
            advertised_len: 1024,
            body: String::new(),
        },
        Script::RespondTruncated {
            status: 503,
            reason: "Service Unavailable",
            advertised_len: 1024,
            body: String::new(),
        },
    ]);
    let provider = AnthropicProvider::with_config(base_config(&base, 1));

    let error = provider
        .chat(one_user_message(), vec![], &chat_config())
        .await
        .expect_err("dropped 503 body must fail");
    let err = transport_error(error);
    assert_eq!(err.kind, LlmErrorKind::Connect);
    assert_eq!(err.attempts, 2);
    assert_eq!(requests_seen(&rx, 200).len(), 2);
}

// ── 16. config() returns a non-secret view (E7) ───────────────────────

#[test]
fn config_accessor_returns_a_non_secret_view() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture: bind loopback");
    let addr = listener.local_addr().expect("fixture: local addr");
    let base = format!("http://{addr}");
    let secret = "sk-ant-definitely-secret";

    let mut config = AnthropicConfig::new(secret).with_base_url(&base);
    config.timeout_secs = 5;
    config.max_retries = 2;
    let provider = AnthropicProvider::with_config(config);

    let view = provider.config();
    assert_eq!(view.base_url, base);
    assert_eq!(view.max_retries, 2);
    assert_eq!(view.timeout_secs, 5);

    // No Debug path reachable from the provider renders the key.
    assert!(
        !format!("{view:?}").contains(secret),
        "view Debug leaked the api_key: {view:?}"
    );
    assert!(
        !format!("{:?}", AnthropicConfig::new(secret)).contains(secret),
        "AnthropicConfig Debug leaked the api_key"
    );
}
