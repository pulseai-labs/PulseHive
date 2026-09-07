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
    ToolChoice, ToolDefinition,
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

    /// Asserts no further request arrives within a second. A disconnected
    /// channel is a failure, not a pass: it means the fixture thread already
    /// exited and the wait proved nothing.
    fn assert_no_more_requests(&self) {
        match self.bodies.recv_timeout(Duration::from_secs(1)) {
            Ok(extra) => panic!("fixture saw an unexpected extra request: {extra}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!(
                "fixture channel disconnected before assert_no_more_requests: \
                 the scripted steps were exhausted"
            ),
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
    // A trailing spare step keeps the fixture alive for the no-more-requests
    // assertion and would record an unexpected retry instead of exhausting
    // the script.
    let fixture = Fixture::spawn(vec![stall(3), respond_ok(minimal_completion())]);
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
        // Spare step: keeps the fixture (and its channel) alive for the
        // no-more-requests assertion and records any unexpected retry.
        respond_ok(minimal_completion()),
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

// ── 2b. A u32::MAX retry budget still sends at least once (F09) ──────

#[tokio::test]
async fn max_retries_at_u32_max_does_not_overflow_to_zero_attempts() {
    // `LlmConfig` is an unvalidated Deserialize; a budget of u32::MAX must
    // saturate, not overflow (debug panic) or wrap to zero attempts. A 400
    // fails on the first attempt, which the wrapped budget could never
    // reach ("retry budget exhausted", attempts: 0).
    let fixture = Fixture::spawn(vec![respond_status(400, "Bad Request", "{}")]);
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("test-key", "test-model")
            .with_base_url(&fixture.base_url)
            .with_max_retries(u32::MAX),
    );

    let result = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await;

    let err = transport_error(result);
    assert_eq!(err.kind, LlmErrorKind::ClientError);
    assert_eq!(err.attempts, 1, "attempts must be at least 1");
    fixture.next_body();
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

// ── 4b. Retry-After is honored on 429/529 only, capped for sleep ─────

#[tokio::test]
async fn retry_after_on_500_family_is_not_honored() {
    // A 503 with Retry-After: 0 retries on the exponential backoff (1s),
    // not the header, and the error does not carry retry_after.
    let fixture = Fixture::spawn(vec![
        respond_with_retry_after(503, "Service Unavailable", 0, "{}"),
        respond_with_retry_after(503, "Service Unavailable", 0, "{}"),
    ]);
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("test-key", "test-model")
            .with_base_url(&fixture.base_url)
            .with_max_retries(1),
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
    assert_eq!(err.kind, LlmErrorKind::ServerError);
    assert_eq!(err.attempts, 2);
    assert_eq!(err.retry_after, None, "503 does not honor Retry-After");
    assert!(
        elapsed.as_millis() >= 900,
        "503 must use the 1s backoff, not Retry-After: 0 (took {elapsed:?})"
    );
}

#[tokio::test]
async fn oversized_retry_after_is_reported_verbatim_but_never_slept() {
    // Retry-After: 100000 (~27.8h) on a 429: the error carries the raw value
    // for the caller; the honored sleep is capped (unit-tested) and a
    // zero-budget call does not sleep at all.
    let fixture = Fixture::spawn(vec![respond_with_retry_after(
        429,
        "Too Many Requests",
        100_000,
        "{}",
    )]);
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("test-key", "test-model")
            .with_base_url(&fixture.base_url)
            .with_max_retries(0),
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
    assert_eq!(err.kind, LlmErrorKind::RateLimited);
    assert_eq!(err.retry_after, Some(Duration::from_secs(100_000)));
    assert!(
        elapsed.as_millis() < 2500,
        "a zero-budget call must not sleep for the header value, took {elapsed:?}"
    );
}

// ── 5. Client errors are immediate and carry the raw body ───────────

#[tokio::test]
async fn client_error_is_immediate_and_carries_body() {
    let bad_request_body = "{\"error\":{\"message\":\"bad\",\"type\":\"invalid_request_error\"}}";
    // The trailing spare step keeps the fixture alive for the assertion and
    // records any unexpected retry.
    let fixture = Fixture::spawn(vec![
        respond_status(400, "Bad Request", bad_request_body),
        respond_ok(minimal_completion()),
    ]);
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

// ── 8a. Cancel aborts a stalled mid-stream body read (F03) ───────────

#[tokio::test]
async fn cancel_mid_stream_aborts_the_returned_stream() {
    // 200 headers arrive, then the SSE body never does. The cancel token
    // must abort the stalled body read, not ride it out to the client
    // deadline.
    let fixture = Fixture::spawn(vec![respond_head_then_stall(200, "OK", 65536)]);
    let provider = fixture.provider();

    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });

    let started = std::time::Instant::now();
    let stream = provider
        .chat_stream(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model").with_cancel(token),
        )
        .await
        .expect("headers arrive, so the stream is handed out");

    let mut items = Vec::new();
    let mut stream = stream;
    while let Some(item) = futures::StreamExt::next(&mut stream).await {
        items.push(item);
        if items.len() > 8 {
            panic!("stream did not terminate after cancellation: {items:?}");
        }
    }
    let elapsed = started.elapsed();

    assert!(
        items.iter().any(|item| matches!(
            item,
            Err(PulseHiveError::LlmTransport(err)) if err.kind == LlmErrorKind::Cancelled
        )),
        "expected a Cancelled transport error in the stream, got: {items:?}"
    );
    assert!(
        elapsed.as_millis() < 2500,
        "cancellation should abort the stalled read promptly, got {elapsed:?}"
    );
}

// ── 8b. A mid-stream body-read failure is a typed error (F10) ────────

#[tokio::test]
async fn mid_stream_body_failure_is_typed_transport_error() {
    // 200 headers, then the body stalls past the 1s client deadline: the
    // read failure must surface from the stream as a typed LlmTransport
    // Timeout, never the stringly Llm(String).
    let fixture = Fixture::spawn(vec![respond_head_then_stall(200, "OK", 65536)]);
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("test-key", "test-model")
            .with_base_url(&fixture.base_url)
            .with_timeout(1),
    );

    let stream = provider
        .chat_stream(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await
        .expect("headers arrive, so the stream is handed out");

    let started = std::time::Instant::now();
    let mut items = Vec::new();
    let mut stream = stream;
    while let Some(item) = futures::StreamExt::next(&mut stream).await {
        items.push(item);
        if items.len() > 8 {
            panic!("stream did not terminate after the read failure: {items:?}");
        }
    }
    let elapsed = started.elapsed();

    let err = items
        .iter()
        .find_map(|item| match item {
            Err(PulseHiveError::LlmTransport(err)) => Some(err.clone()),
            _ => None,
        })
        .expect("expected a typed transport error in the stream");
    assert_eq!(err.kind, LlmErrorKind::Timeout);
    assert_eq!(err.attempts, 1);
    assert!(
        elapsed.as_millis() >= 900 && elapsed.as_millis() < 2500,
        "expected the body-read timeout in about one second, got {elapsed:?}"
    );
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
    // A generous window: the canceller fires at 100ms, but scheduler
    // contention on a loaded CI runner can delay the observed return well
    // past a tight bound.
    assert!(
        elapsed.as_millis() < 2500,
        "cancellation should return promptly, got {elapsed:?}"
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

// ── 7b. Empty-string tool arguments are a zero-argument call (F06) ───

#[tokio::test]
async fn empty_string_tool_arguments_parse_as_empty_object() {
    // Ollama / LM Studio / vLLM emit "" for zero-argument tool calls; that
    // is a call with {} arguments, not a malformed one.
    let empty_args = "{\"id\":\"fx\",\"choices\":[{\"message\":{\"content\":null,\"tool_calls\":[{\"id\":\"call_9\",\"type\":\"function\",\"function\":{\"name\":\"get_time\",\"arguments\":\"\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":5}}";
    let fixture = Fixture::spawn(vec![respond_ok(empty_args)]);
    let provider = fixture.provider();

    let response = provider
        .chat(
            vec![Message::user("hi")],
            vec![],
            &LlmConfig::new("openai", "test-model"),
        )
        .await
        .expect("empty-string arguments are a legitimate zero-argument call");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].name, "get_time");
    assert_eq!(response.tool_calls[0].arguments, serde_json::json!({}));
}

// ── 10. New request fields serialize only when set (#47 R6) ─────────

/// One tool definition for requests that exercise `tool_choice` — the wire
/// field is only emitted when the request carries tools.
fn one_tool() -> ToolDefinition {
    ToolDefinition {
        name: "search".into(),
        description: "Search the web".into(),
        parameters: serde_json::json!({"type": "object"}),
    }
}

#[tokio::test]
async fn reasoning_effort_and_tool_choice_serialize_only_when_set() {
    async fn sent_body(config: &LlmConfig, tools: Vec<ToolDefinition>) -> String {
        let fixture = Fixture::spawn(vec![respond_ok(minimal_completion())]);
        let provider = fixture.provider();
        let result = provider
            .chat(vec![Message::user("hi")], tools, config)
            .await;
        assert!(result.is_ok(), "fixture call should succeed: {result:?}");
        fixture.next_body()
    }

    let body = sent_body(
        &LlmConfig::new("openai", "test-model").with_reasoning_effort(ReasoningEffort::Low),
        vec![],
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
        let body = sent_body(
            &LlmConfig::new("openai", "test-model").with_tool_choice(choice),
            vec![one_tool()],
        )
        .await;
        assert!(body.contains(expected), "body: {body}");
    }

    // With neither set, neither key appears (test 9 pins the full golden).
    let body = sent_body(&LlmConfig::new("openai", "test-model"), vec![]).await;
    assert!(!body.contains("reasoning_effort"), "body: {body}");
    assert!(!body.contains("tool_choice"), "body: {body}");
}

// ── 10b. tool_choice without tools is omitted, not rejected (F05) ────

#[tokio::test]
async fn tool_choice_is_omitted_when_the_request_carries_no_tools() {
    // Every OpenAI-compatible endpoint answers tool_choice-without-tools
    // with a 400, so the provider must drop the field instead.
    let fixture = Fixture::spawn(vec![respond_ok(minimal_completion())]);
    let provider = fixture.provider();

    let config = LlmConfig::new("openai", "test-model").with_tool_choice(ToolChoice::Required);
    let result = provider
        .chat(vec![Message::user("hi")], vec![], &config)
        .await;
    assert!(
        result.is_ok(),
        "tool_choice with no tools must not break the call: {result:?}"
    );
    let body = fixture.next_body();
    assert!(
        !body.contains("tool_choice"),
        "tool_choice must be omitted when tools is empty: {body}"
    );
    assert!(!body.contains("\"tools\""), "body: {body}");
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

// ── 12. config() accessor (E7): a non-secret view ────────────────────

#[test]
fn config_accessor_returns_a_non_secret_view() {
    let secret = "sk-definitely-secret";
    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new(secret, "test-model")
            .with_timeout(17)
            .with_max_retries(2),
    );

    let view = provider.config();
    assert_eq!(view.model, "test-model");
    assert_eq!(view.timeout_secs, 17);
    assert_eq!(view.max_retries, 2);
    assert_eq!(view.base_url, "https://api.openai.com/v1");

    // No Debug path reachable from the provider renders the key.
    assert!(
        !format!("{view:?}").contains(secret),
        "view Debug leaked the api_key: {view:?}"
    );
    assert!(
        !format!("{:?}", OpenAIConfig::new(secret, "m")).contains(secret),
        "OpenAIConfig Debug leaked the api_key"
    );
}
