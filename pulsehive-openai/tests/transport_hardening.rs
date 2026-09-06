//! Hermetic transport-hardening fixture suite for `OpenAICompatibleProvider`.
//!
//! Every test runs against an in-process `TcpListener` fixture on loopback: no
//! external network, no new dev-dependency. The listener accepts connections in
//! order and replays a scripted step per connection, recording each request's
//! raw body on a channel. Spec §4.7; ledger line d3 — this suite must stay green.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use pulsehive_core::error::{PulseHiveError, Result as PhResult};
use pulsehive_core::llm::{
    LlmConfig, LlmError, LlmErrorKind, LlmProvider, LlmResponse, Message, ReasoningEffort,
    ToolChoice,
};
use pulsehive_openai::{OpenAICompatibleProvider, OpenAIConfig};
use tokio_util::sync::CancellationToken;

// ── Fixture ──────────────────────────────────────────────────────────

/// What the fixture does with one accepted connection.
enum Step {
    /// Write a full HTTP/1.1 response and close.
    Respond {
        status: u16,
        reason: &'static str,
        retry_after: Option<u64>,
        body: String,
    },
    /// Hold the connection open without responding, then drop it.
    Stall { secs: u64 },
    /// Hold the connection open until the client disconnects.
    StallUntilDisconnect,
    /// Write response headers for `status` promising a body of
    /// `content_length` bytes, then hold the connection open without ever
    /// sending the body.
    RespondHeadThenStall {
        status: u16,
        reason: &'static str,
        content_length: usize,
    },
    /// Write response headers for `status` promising a body, then drop the
    /// connection mid-body.
    DropMidBody { status: u16, reason: &'static str },
}

/// A scripted `200 OK` carrying `body` as a chat completion.
fn respond_ok(body: impl Into<String>) -> Step {
    Step::Respond {
        status: 200,
        reason: "OK",
        retry_after: None,
        body: body.into(),
    }
}

/// A scripted response with an explicit status, reason and `Retry-After`.
fn respond_with_retry_after(
    status: u16,
    reason: &'static str,
    retry_after: u64,
    body: impl Into<String>,
) -> Step {
    Step::Respond {
        status,
        reason,
        retry_after: Some(retry_after),
        body: body.into(),
    }
}

/// A scripted response with an explicit status and reason, no `Retry-After`.
fn respond_status(status: u16, reason: &'static str, body: impl Into<String>) -> Step {
    Step::Respond {
        status,
        reason,
        retry_after: None,
        body: body.into(),
    }
}

/// Hold one connection open for `secs` without ever responding.
fn stall(secs: u64) -> Step {
    Step::Stall { secs }
}

/// Hold one connection open until the client side disconnects.
fn stall_until_disconnect() -> Step {
    Step::StallUntilDisconnect
}

/// Send `status` headers promising a body, then stall without sending it.
fn respond_head_then_stall(status: u16, reason: &'static str, content_length: usize) -> Step {
    Step::RespondHeadThenStall {
        status,
        reason,
        content_length,
    }
}

/// Send `status` headers promising a body, then drop the connection mid-body.
fn drop_mid_body(status: u16, reason: &'static str) -> Step {
    Step::DropMidBody { status, reason }
}

/// Unwraps a typed transport error out of a call result, panicking otherwise.
fn transport_error(result: PhResult<LlmResponse>) -> LlmError {
    match result {
        Err(PulseHiveError::LlmTransport(err)) => err,
        other => panic!("expected LlmTransport error, got: {other:?}"),
    }
}

/// A loopback HTTP fixture that replays `steps`, one per accepted connection.
struct Fixture {
    base_url: String,
    bodies: Receiver<String>,
}

impl Fixture {
    fn spawn(steps: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        let addr = listener.local_addr().expect("read listener address");
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            for step in steps {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(_) => return,
                };
                let body = read_http_request(&mut stream);
                // The receiver may already be gone once the test's assertions
                // have finished; that is not the fixture's problem.
                let _ = tx.send(body);

                match step {
                    Step::Respond {
                        status,
                        reason,
                        retry_after,
                        body,
                    } => {
                        let mut head = format!(
                            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
                            body.len()
                        );
                        if let Some(secs) = retry_after {
                            head.push_str(&format!("Retry-After: {secs}\r\n"));
                        }
                        head.push_str("Connection: close\r\n\r\n");
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.write_all(body.as_bytes());
                        let _ = stream.flush();
                    }
                    Step::Stall { secs } => thread::sleep(Duration::from_secs(secs)),
                    Step::StallUntilDisconnect => {
                        let mut sink = [0u8; 1024];
                        loop {
                            match stream.read(&mut sink) {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {}
                            }
                        }
                    }
                    Step::RespondHeadThenStall {
                        status,
                        reason,
                        content_length,
                    } => {
                        let head = format!(
                            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.flush();
                        thread::sleep(Duration::from_secs(3));
                    }
                    Step::DropMidBody { status, reason } => {
                        let head = format!(
                            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.flush();
                        // Dropping the stream closes the connection with the
                        // promised body unsent.
                    }
                }
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            bodies: rx,
        }
    }

    /// A provider pointed at this fixture with the default test credentials.
    fn provider(&self) -> OpenAICompatibleProvider {
        OpenAICompatibleProvider::new(
            OpenAIConfig::new("test-key", "test-model").with_base_url(&self.base_url),
        )
    }

    /// Waits for the next recorded request body.
    fn next_body(&self) -> String {
        self.bodies
            .recv_timeout(Duration::from_secs(5))
            .expect("fixture saw a request")
    }

    /// Asserts no further request arrives within a second.
    fn assert_no_more_requests(&self) {
        match self.bodies.recv_timeout(Duration::from_secs(1)) {
            Ok(extra) => panic!("fixture saw an unexpected extra request: {extra}"),
            Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
    }
}

/// Reads one HTTP/1.1 request (head plus `Content-Length` body) from `stream`.
///
/// Tolerant by design: a client that disconnects mid-request yields whatever
/// arrived, never a panic.
fn read_http_request(stream: &mut TcpStream) -> String {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];

    let head_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return String::from_utf8_lossy(&buf).into_owned(),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let content_length = head
        .lines()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")?
                .trim()
                .parse::<usize>()
                .ok()
        })
        .unwrap_or(0);

    while buf.len() < head_end + 4 + content_length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }

    let start = head_end + 4;
    let end = (start + content_length).min(buf.len());
    String::from_utf8_lossy(&buf[start..end]).into_owned()
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The minimal valid chat completion body the fixture's `200 OK` steps send.
fn minimal_completion() -> String {
    r#"{"id":"fx","choices":[{"message":{"content":"ok","tool_calls":null},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#.into()
}

// ── 9. Request-body compatibility golden ─────────────────────────────

/// What 2.0.2's `build_request` put on the wire for `LlmConfig::new("openai",
/// "test-model")` with temperature 0.2, max_tokens 512, one user message
/// "hi" and no tools. Captured against the unchanged crate before any
/// provider edit (#47 R1, R6): if this stops matching, request
/// compatibility broke.
const GOLDEN_2_0_2_BODY: &str = "{\"model\":\"test-model\",\"messages\":[{\"content\":\"hi\",\"role\":\"user\"}],\"temperature\":0.2,\"max_tokens\":512}";

#[tokio::test]
async fn request_body_is_byte_identical_to_2_0_2_when_new_fields_unset() {
    let fixture = Fixture::spawn(vec![respond_ok(minimal_completion())]);
    let provider = fixture.provider();

    let config = LlmConfig::new("openai", "test-model")
        .with_temperature(0.2)
        .with_max_tokens(512);

    let result = provider
        .chat(vec![Message::user("hi")], vec![], &config)
        .await;
    assert!(result.is_ok(), "golden call should succeed: {result:?}");

    let body = fixture.next_body();
    assert_eq!(body, GOLDEN_2_0_2_BODY);
}

// ── 1. Timeout is typed, single-attempt, never re-sent (#46) ────────

#[tokio::test]
async fn timeout_fails_once_with_typed_timeout_and_is_not_resent() {
    let fixture = Fixture::spawn(vec![stall(3)]);
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("test-key", "test-model")
            .with_base_url(&fixture.base_url)
            .with_timeout(1)
            .with_max_retries(3),
    );

    let started = std::time::Instant::now();
    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await;
    let elapsed = started.elapsed();

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::Timeout);
    assert_eq!(err.attempts, 1, "a timed-out request must never be re-sent");
    assert!(
        elapsed.as_millis() >= 900 && elapsed.as_millis() < 2500,
        "expected the timeout in about one second, got {elapsed:?}"
    );

    fixture.next_body();
    fixture.assert_no_more_requests();
}

// ── 1c. Non-success body-read failures are transport failures ───────

#[tokio::test]
async fn non_success_body_read_timeout_is_typed_timeout_not_retried() {
    // 503 headers arrive; the promised body never does. The per-call
    // deadline fires while reading the error body.
    let fixture = Fixture::spawn(vec![respond_head_then_stall(
        503,
        "Service Unavailable",
        2048,
    )]);
    let provider = fixture.provider();

    let started = std::time::Instant::now();
    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model").with_timeout_secs(1),
        )
        .await;
    let elapsed = started.elapsed();

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::Timeout);
    assert_eq!(err.attempts, 1);
    assert_eq!(err.status, None);
    assert!(
        elapsed.as_millis() >= 900 && elapsed.as_millis() < 2500,
        "expected the per-call timeout in about one second, got {elapsed:?}"
    );

    fixture.next_body();
    fixture.assert_no_more_requests();
}

#[tokio::test]
async fn non_success_body_read_drop_is_connect_and_retries() {
    // Both connections send 503 headers, then drop mid-body: a body-read
    // transport error, retried within the per-call budget.
    let fixture = Fixture::spawn(vec![
        drop_mid_body(503, "Service Unavailable"),
        drop_mid_body(503, "Service Unavailable"),
    ]);
    let provider = fixture.provider();

    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model").with_max_retries(1),
        )
        .await;

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::Connect);
    assert_eq!(err.attempts, 2);

    fixture.next_body();
    fixture.next_body();
}

// ── 2. Per-call max_retries override, one provider instance (E12) ───

#[tokio::test]
async fn per_call_max_retries_override_wins_on_one_instance() {
    let over_limit_body = "{\"error\":{\"message\":\"overloaded\"}}";
    let fixture = Fixture::spawn(vec![
        respond_with_retry_after(503, "Service Unavailable", 0, over_limit_body),
        respond_ok(minimal_completion()),
        respond_status(503, "Service Unavailable", over_limit_body),
    ]);
    // Provider default: zero retries (one attempt).
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("test-key", "test-model")
            .with_base_url(&fixture.base_url)
            .with_max_retries(0),
    );

    // Call A overrides the retry budget for this one call.
    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model").with_max_retries(1),
        )
        .await;
    assert!(result.is_ok(), "call A should retry to success: {result:?}");
    fixture.next_body();
    fixture.next_body();

    // Call B on the same instance leaves the budget unset: one attempt only.
    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await;
    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::ServerError);
    assert_eq!(err.attempts, 1);
    assert_eq!(err.status, Some(503));
    fixture.next_body();
    fixture.assert_no_more_requests();
}

// ── 3. Per-call timeout override shortens a long client timeout ─────

#[tokio::test]
async fn per_call_timeout_override_shortens_a_long_client_timeout() {
    let fixture = Fixture::spawn(vec![stall(3)]);
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("test-key", "test-model")
            .with_base_url(&fixture.base_url)
            .with_timeout(30),
    );

    let started = std::time::Instant::now();
    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model").with_timeout_secs(1),
        )
        .await;
    let elapsed = started.elapsed();

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::Timeout);
    assert_eq!(err.attempts, 1);
    assert!(
        elapsed.as_millis() >= 900 && elapsed.as_millis() < 2500,
        "expected the per-call timeout in about one second, got {elapsed:?}"
    );
}

// ── 3b. Post-retry errors report the requests actually sent (4.1) ───

#[tokio::test]
async fn post_retry_parse_and_malformed_errors_carry_attempts_sent() {
    let over_limit_body = "{\"error\":{\"message\":\"overloaded\"}}";

    // A retried 503 followed by an unparseable 200: Parse, attempts == 2.
    let fixture = Fixture::spawn(vec![
        respond_with_retry_after(503, "Service Unavailable", 0, over_limit_body),
        respond_ok("not json"),
    ]);
    let provider = fixture.provider();
    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model").with_max_retries(1),
        )
        .await;
    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::Parse);
    assert_eq!(err.attempts, 2, "both requests were actually sent");
    assert_eq!(err.status, Some(200));

    // Sibling: a retried 503 followed by a 200 with truncated tool
    // arguments: MalformedToolCall, attempts == 2.
    let truncated = "{\"id\":\"fx\",\"choices\":[{\"message\":{\"content\":null,\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\": \"}}]},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":5}}";
    let fixture = Fixture::spawn(vec![
        respond_with_retry_after(503, "Service Unavailable", 0, over_limit_body),
        respond_ok(truncated),
    ]);
    let provider = fixture.provider();
    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model").with_max_retries(1),
        )
        .await;
    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::MalformedToolCall);
    assert_eq!(err.attempts, 2, "both requests were actually sent");
    assert_eq!(err.finish_reason.as_deref(), Some("length"));
}

// ── 4. Rate limit carries Retry-After and exhausts typed ────────────

#[tokio::test]
async fn rate_limit_carries_retry_after_and_exhausts_to_typed_error() {
    let limited_body = "{\"error\":{\"message\":\"rate limited\"}}";
    let fixture = Fixture::spawn(vec![
        respond_with_retry_after(429, "Too Many Requests", 0, limited_body),
        respond_with_retry_after(429, "Too Many Requests", 0, limited_body),
    ]);
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("test-key", "test-model")
            .with_base_url(&fixture.base_url)
            .with_max_retries(1),
    );

    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await;

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::RateLimited);
    assert_eq!(err.attempts, 2);
    assert_eq!(err.status, Some(429));
    assert_eq!(err.retry_after, Some(Duration::ZERO));
    assert_eq!(err.body.as_deref(), Some(limited_body));
}

// ── 5. Client errors are immediate and carry the raw body ───────────

#[tokio::test]
async fn client_error_is_immediate_and_carries_body() {
    let bad_request_body = "{\"error\":{\"message\":\"bad\",\"type\":\"invalid_request_error\"}}";
    let fixture = Fixture::spawn(vec![respond_status(400, "Bad Request", bad_request_body)]);
    let provider = fixture.provider();

    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await;

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::ClientError);
    assert_eq!(err.attempts, 1);
    assert_eq!(err.status, Some(400));
    assert_eq!(err.body.as_deref(), Some(bad_request_body));

    fixture.next_body();
    fixture.assert_no_more_requests();
}

// ── 6. Unparseable success body is a typed Parse error ──────────────

#[tokio::test]
async fn unparseable_success_body_is_parse_error() {
    let fixture = Fixture::spawn(vec![respond_ok("not json")]);
    let provider = fixture.provider();

    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await;

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::Parse);
    assert_eq!(err.status, Some(200));
    assert_eq!(err.body.as_deref(), Some("not json"));
}

// ── 7. Truncated tool arguments are MalformedToolCall, not {} (E6) ──

#[tokio::test]
async fn truncated_tool_arguments_are_malformed_tool_call_not_empty_call() {
    let truncated = "{\"id\":\"fx\",\"choices\":[{\"message\":{\"content\":null,\"tool_calls\":[{\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\": \"}}]},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":5}}";
    let fixture = Fixture::spawn(vec![respond_ok(truncated)]);
    let provider = fixture.provider();

    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await;

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::MalformedToolCall);
    assert_eq!(err.finish_reason.as_deref(), Some("length"));
    assert_eq!(err.body.as_deref(), Some("{\"path\": "));

    // A model that legitimately sends "{}" still yields a callable {}.
    let empty_args = "{\"id\":\"fx\",\"choices\":[{\"message\":{\"content\":null,\"tool_calls\":[{\"id\":\"call_2\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":5}}";
    let fixture = Fixture::spawn(vec![respond_ok(empty_args)]);
    let provider = fixture.provider();
    let response = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await
        .expect("\"{}\" arguments are a legitimate empty object call");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].arguments, serde_json::json!({}));
}

// ── 8. Cancel token aborts the in-flight request (L3) ───────────────

#[tokio::test]
async fn cancel_token_aborts_in_flight_request() {
    let fixture = Fixture::spawn(vec![stall_until_disconnect()]);
    let provider = fixture.provider();

    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });

    let started = std::time::Instant::now();
    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model").with_cancel(token),
        )
        .await;
    let elapsed = started.elapsed();

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::Cancelled);
    assert_eq!(err.attempts, 1);
    assert!(
        elapsed.as_millis() < 1000,
        "cancellation should return well under one second, got {elapsed:?}"
    );
    fixture.next_body();

    // A token cancelled before the call never sends anything.
    let fixture = Fixture::spawn(vec![stall_until_disconnect()]);
    let provider = fixture.provider();
    let token = CancellationToken::new();
    token.cancel();

    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model").with_cancel(token),
        )
        .await;

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::Cancelled);
    assert_eq!(err.attempts, 0);
    fixture.assert_no_more_requests();
}

// ── 10. New request fields serialize only when set (#47 R6) ─────────

#[tokio::test]
async fn reasoning_effort_and_tool_choice_serialize_only_when_set() {
    async fn sent_body(config: &LlmConfig) -> String {
        let fixture = Fixture::spawn(vec![respond_ok(minimal_completion())]);
        let provider = fixture.provider();
        let result = provider
            .chat(vec![Message::user("hi")], vec![], config)
            .await;
        assert!(result.is_ok(), "fixture call should succeed: {result:?}");
        fixture.next_body()
    }

    let body = sent_body(
        &LlmConfig::new("openai", "test-model").with_reasoning_effort(ReasoningEffort::Low),
    )
    .await;
    assert!(
        body.contains("\"reasoning_effort\":\"low\""),
        "body: {body}"
    );

    for (choice, expected) in [
        (ToolChoice::Required, "\"tool_choice\":\"required\""),
        (ToolChoice::Auto, "\"tool_choice\":\"auto\""),
        (ToolChoice::None, "\"tool_choice\":\"none\""),
        (
            ToolChoice::Function { name: "f".into() },
            "\"tool_choice\":{\"type\":\"function\",\"function\":{\"name\":\"f\"}}",
        ),
    ] {
        let body =
            sent_body(&LlmConfig::new("openai", "test-model").with_tool_choice(choice)).await;
        assert!(body.contains(expected), "body: {body}");
    }

    // With neither set, neither key appears (test 9 pins the full golden).
    let body = sent_body(&LlmConfig::new("openai", "test-model")).await;
    assert!(!body.contains("reasoning_effort"), "body: {body}");
    assert!(!body.contains("tool_choice"), "body: {body}");
}

// ── 11. finish_reason and reasoning come off the wire (#47 R3, R6) ──

#[tokio::test]
async fn finish_reason_and_reasoning_come_off_the_wire() {
    // The exact #47 failure shape: cut off mid-reasoning with no text.
    let length_cut = "{\"id\":\"fx\",\"choices\":[{\"message\":{\"content\":\"\",\"reasoning\":\"I was thinking about the answer\",\"tool_calls\":null},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":9}}";
    let fixture = Fixture::spawn(vec![respond_ok(length_cut)]);
    let provider = fixture.provider();

    let response = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await
        .expect("the length-cut fixture is a valid response");
    assert_eq!(response.finish_reason.as_deref(), Some("length"));
    assert_eq!(
        response.reasoning.as_deref(),
        Some("I was thinking about the answer")
    );
    assert!(response.tool_calls.is_empty());

    // The reasoning_content alias maps to reasoning too.
    let aliased = "{\"id\":\"fx\",\"choices\":[{\"message\":{\"content\":\"ok\",\"reasoning_content\":\"via the alias\",\"tool_calls\":null},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":9}}";
    let fixture = Fixture::spawn(vec![respond_ok(aliased)]);
    let provider = fixture.provider();
    let response = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await
        .expect("the aliased fixture is a valid response");
    assert_eq!(response.reasoning.as_deref(), Some("via the alias"));

    // With neither field present, reasoning stays None.
    let plain = "{\"id\":\"fx\",\"choices\":[{\"message\":{\"content\":\"ok\",\"tool_calls\":null},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":9}}";
    let fixture = Fixture::spawn(vec![respond_ok(plain)]);
    let provider = fixture.provider();
    let response = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await
        .expect("the plain fixture is a valid response");
    assert_eq!(response.reasoning, None);
}

// ── 12. config() accessor (E7) ──────────────────────────────────────

#[tokio::test]
async fn config_accessor_returns_the_provider_config() {
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("test-key", "test-model")
            .with_timeout(17)
            .with_max_retries(2),
    );

    let config = provider.config();
    assert_eq!(config.model, "test-model");
    assert_eq!(config.timeout_secs, 17);
    assert_eq!(config.max_retries, 2);
}
