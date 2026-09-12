# Changelog

All notable changes to PulseHive will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.1.0] - Unreleased

### Fixed
- **pulsehive-openai**: a timed-out request now fails once with a typed `LlmErrorKind::Timeout` and is never re-sent (#46) — previously a timeout was retried like a connection error, re-billing the consumer for generation already spent.
- **pulsehive-openai**: a tool call whose `arguments` do not parse as a JSON object (e.g. truncated by `finish_reason: "length"`) now surfaces as a typed `LlmErrorKind::MalformedToolCall` carrying the raw arguments and the finish reason, instead of a dispatchable call with empty `{}` arguments. An empty or whitespace-only `arguments` string is not a parse failure: it is a zero-argument call (Ollama, LM Studio and vLLM emit `""` for those) and parses as `{}` — unless the completion was truncated (`finish_reason: "length"`), where the empty string means the arguments were cut off before any JSON was emitted and is a `MalformedToolCall`.

### Added
- **Streaming tools** — a new `pulsehive_core::tool::StreamingTool: Tool` trait for long-running tools that report live progress. Tools expose it by overriding `Tool::as_streaming()` to return `Some(self)`; the agent loop then calls `StreamingTool::execute_streaming(params, context, progress_tx)` and forwards each pushed event.
- **pulsehive-openai**: every transport failure from `OpenAICompatibleProvider` is now a typed `PulseHiveError::LlmTransport(LlmError)` with kind (`Timeout`, `Connect`, `RateLimited`, `ServerError`, `ClientError`, `Parse`, `MalformedToolCall`, `Cancelled`), status, attempts, verbatim body and `retry_after` — including mid-stream body-read failures from `chat_stream`, classified like `chat()`'s body read. A terminal 3xx (a 304, or a redirect without a usable `Location`) classifies as `ClientError`, not `ServerError`. `PulseHiveError::Llm(String)` remains for request-build failures — serialization, or a request that cannot be built at all (malformed `base_url`), which fails immediately without retrying or sending anything.
- **pulsehive-openai**: per-call `LlmConfig::timeout_secs` and `LlmConfig::max_retries` override the provider's configured values for that one call on a single provider instance (the budget saturates: `u32::MAX` still sends at least one attempt), and `LlmConfig::cancel` aborts an in-flight request (or a backoff sleep, or a read of a `chat_stream` body already in progress) mid-flight, returning `LlmErrorKind::Cancelled`.
- **pulsehive-openai**: `reasoning_effort` and `tool_choice` are sent on the wire only when set (`tool_choice` maps to `"auto"` / `"none"` / `"required"` / `{"type":"function","function":{"name":…}}`) and only when the request carries tools — every OpenAI-compatible endpoint rejects `tool_choice` without `tools`; with both absent the request body is byte-identical to 2.0.2. On a retryable status, `Retry-After` is honored only on 429/529 and the honored sleep is capped at the 8s backoff ceiling (the raw header value still travels on the error's `retry_after`).
- **pulsehive-openai**: `finish_reason` and `reasoning` (including the `reasoning_content` alias) come off the wire onto `LlmResponse`; a non-string `reasoning`/`reasoning_content` value (the structured objects some compatible endpoints return) is ignored as `None` instead of failing the response, and a response carrying both spellings (one commonly null) parses with `reasoning` preferred when both carry strings. `chat_stream` still carries neither (documented limitation), and the streamed response ends at the `Done` chunk — polling past it yields end-of-stream without waiting on the body, so a cancellation after `Done` (or a stalled body) produces no error and no delay.
- **pulsehive-openai**: `OpenAICompatibleProvider::config()` returns an `OpenAIConfigView` — the transport settings (endpoint, model, timeout, retry budget) with no path to the API key. Neither the view nor `OpenAIConfig` renders the key in `Debug`, and `base_url` renders with any URL userinfo (`user:pass@`) stripped; the provider keeps dialing the original URL.
- **`pulsehive_core::tool::ToolProgress`** enum — the progress payload (`Started` / `Progress { fraction, message }` / `PartialResult` / `Log` / `Completed { duration_ms }`). The `Started` / `Completed` bookends are emitted by the agent loop; tool bodies push the intermediate variants.
- **`HiveEvent::ToolProgress { agent_id, tool_name, progress }`** — the agent loop forwards each `ToolProgress` from a streaming tool as this event on the `HiveMind::deploy()` stream, so consumers see live progress instead of a frozen wait. Delivery on `deploy()` is **best-effort** (a lossy broadcast that drops events for lagging subscribers); the ordered `Started → … → Completed` envelope is emitted in order by the loop but preserved by **neither** transport for consumers (the `deploy()` broadcast drops on lag; a configured `EventExporter` receives each event via an independent fire-and-forget task), so consumers should not treat `Completed` as a guaranteed terminal marker.
- New runnable, hermetic example `pulsehive-runtime/examples/streaming_tool.rs` (no API key) and a "Streaming Tools" section in `docs/05-API-Spec.md`.
- **`pulsehive_core::llm::LlmError` and `LlmErrorKind`** — a structured transport failure (`kind`, `message`, `status`, `attempts`, `body`, `finish_reason`, `retry_after`) carried by the new **`PulseHiveError::LlmTransport`** variant, with `PulseHiveError::llm_transport()` and a `From<LlmError>` conversion. `LlmErrorKind` covers `Timeout`, `Connect`, `RateLimited`, `ServerError`, `ClientError`, `Parse`, `MalformedToolCall` and `Cancelled`. Callers can branch on the kind instead of matching on message text. `Display` renders kind, attempts, message and status only — the verbatim `body` is for explicit inspection and never reaches a log line through `to_string()`. `PulseHiveError::Llm(String)` is unchanged and still carries request-build and serialization failures.
- **`LlmConfig::{timeout_secs, max_retries, reasoning_effort, tool_choice, cancel}`** and the builders `with_temperature`, `with_max_tokens`, `with_timeout_secs`, `with_max_retries`, `with_reasoning_effort`, `with_tool_choice`, `with_cancel`. `timeout_secs` and `max_retries` override the provider's configured value for a single call; `cancel` carries a `tokio_util::sync::CancellationToken` and is never serialized.
- **`pulsehive_core::llm::ReasoningEffort`** (`Minimal` / `Low` / `Medium` / `High`) and **`ToolChoice`** (`Auto` / `None` / `Required` / `Function { name }`).
- **`LlmResponse::{finish_reason, reasoning}`** plus the constructors `LlmResponse::new` and `LlmResponse::text` and the builders `with_tool_calls`, `with_usage`, `with_finish_reason`, `with_reasoning`. Both providers fill them today: `pulsehive-openai` populates `finish_reason` and `reasoning` (including the `reasoning_content` alias); `pulsehive-anthropic` populates `finish_reason` verbatim from `stop_reason` and always leaves `reasoning` `None` (it never requests `thinking` blocks).
- `tokio-util` is now a dependency of `pulsehive-core` (cancellation tokens only).
- **`pulsehive_core::testing::ScriptedProvider`** — a queued-response `LlmProvider` for deterministic, offline agent tests, behind the new `testing` feature on `pulsehive-core` and on the `pulsehive` meta-crate, where it reaches consumers as `pulsehive::testing`. Script replies with `ScriptedProvider::new()` and the `then_text` / `then_tool_call` / `then_response` / `then_error` / `then_hang` builder (tool-call ids `call_1`, `call_2`, … numbered at script time); every call records a `RecordedRequest` (messages, tools, config) readable through `requests()`, and all clones share one queue and one request log. An exhausted script is a typed `Llm` error naming the calls served — never a panic or default reply — and cancellation follows the provider transport contract: an already-cancelled token returns `LlmTransport` with kind `Cancelled` and `attempts == 0` (nothing was sent), a fired `then_hang` with `attempts == 1` (the hang models one in-flight request). `chat_stream` replays the same step as `Text` (when content is set), `ToolCallStart` plus one full-arguments `ToolCallDelta` per tool call, then `Done`; on the streaming path cancellation arrives inside the stream — the call returns `Ok(stream)` and the `Cancelled` error is its single `Err` item, like `pulsehive-openai`'s `chat_stream` — while `then_error` and script exhaustion fail the call itself, matching that provider's pre-stream failure path.

### Changed
- **BREAKING: PulseDB 0.7 upgrade.** Re-exported PulseDB value types now use the 0.7 construction shape. Existing PulseDB 0.5.1 collectives migrate on first writable open; preserve both sidecars and follow ADR-012's restore-before-downgrade rollback guide.
- **BREAKING: `HiveEvent` is now `#[non_exhaustive]`.** New event variants (such as `ToolProgress`) can be added in a minor release without a major bump. External code that matches on `HiveEvent` exhaustively must add a `_ => {}` catch-all arm, or it will fail to compile.
- **BREAKING: `LlmConfig`, `LlmResponse`, `LlmError`, `LlmErrorKind`, `ReasoningEffort`, `ToolChoice` and `PulseHiveError` are now `#[non_exhaustive]`.** Struct literals for `LlmConfig`, `LlmResponse` and `LlmError` outside `pulsehive-core` no longer compile — move to `LlmConfig::new(..)` / `LlmResponse::new(..)` / `LlmResponse::text(..)` / `LlmError::new(..)` and the `with_*` builders. An exhaustive `match` on `PulseHiveError` needs a `_ =>` arm. Reading and assigning fields is unchanged. **This is the one coordinated break on the provider transport contract**: every field added to these types after it is additive and lands without a further break.
- **`pulsehive-anthropic`: transport failures are typed `LlmTransport` errors** — `AnthropicProvider` now classifies every transport failure as `PulseHiveError::LlmTransport` with the shared kind table (`Timeout`, `Connect`, `RateLimited`, `ServerError`, `ClientError`, `Parse`, `MalformedToolCall`, `Cancelled`) carrying `status`, `attempts`, the raw `body`, `finish_reason` and `retry_after`; a request that cannot be built (malformed `base_url`, an `api_key` that cannot be a header value) is the exception — a request-build `Llm(String)` failure surfaced immediately, without retrying or sending anything. A timeout fails once and is never re-sent; connection errors and HTTP 500/502/503/529 now retry within the attempt budget (previously only 429/529 retried and nothing else did), and 429/529 honour an integer-seconds `Retry-After` header, capped at the 16s backoff ceiling (the raw header value still travels on the error's `retry_after`); any other 4xx/5xx fails immediately, a `ClientError` carrying the Anthropic error envelope's `error.message` when the body parses as one; a success status with an unparseable body is `Parse`; a `tool_use` block whose `input` is not a JSON object is a typed `MalformedToolCall` (verbatim `body`, `status 200`, the response's `stop_reason`) instead of an unchecked value.
- **`pulsehive-anthropic`: per-call overrides and cancellation** — `LlmConfig::timeout_secs` and `LlmConfig::max_retries` override `AnthropicConfig`'s values for a single call (the budget saturates: `u32::MAX` still sends at least one attempt), and a cancelled `LlmConfig::cancel` token aborts the in-flight request, returning `Cancelled` with the attempts made so far (0 if cancelled before the first send).
- **`pulsehive-anthropic`: `stop_reason` is reported verbatim as `finish_reason`** (`end_turn`, `max_tokens`, `stop_sequence`, `tool_use`, … — no normalization); `reasoning` is always `None` on this provider.
- **`pulsehive-anthropic`: `tool_choice` is mapped to Anthropic's wire shapes** — `Auto` → `{"type":"auto"}`, `Required` → `{"type":"any"}`, `Function { name }` → `{"type":"tool","name":…}`, `None` → `{"type":"none"}`, sent only when `LlmConfig::tool_choice` is set and the request carries tools (the Messages API rejects `tool_choice` without `tools`); the request body is unchanged when it is not.
- **`pulsehive-anthropic`: `reasoning_effort` is accepted and ignored** — the field is never sent on the wire (no `reasoning_effort` and no `thinking` parameter); mapping it to Anthropic extended thinking is a recorded feature-map entry, not provider parity.
- **`pulsehive-anthropic`: `config()` accessor** — `AnthropicProvider::config()` returns an `AnthropicConfigView`: the transport settings (endpoint, model, timeout, retry budget) with no path to the API key. Neither the view nor `AnthropicConfig` renders the key in `Debug`, and `base_url` renders with any URL userinfo (`user:pass@`) stripped; the provider keeps dialing the original URL.
- **`pulsehive-anthropic`: new hermetic fixture suite** `tests/transport_hardening.rs` — sixteen loopback-only `TcpListener` tests pinning the kind table, retries, overrides, cancellation, wire mapping and `config()`; `tokio-util` becomes a dev-dependency of the crate for constructing cancellation tokens in tests.

## [2.0.2] - 2026-07-01

### Security
- **pulsehive-py**: upgrade PyO3 `0.28` -> `0.29`, fixing **RUSTSEC-2026-0176** (out-of-bounds read in `PyList`/`PyTuple` iterators) and **RUSTSEC-2026-0177** (missing `Sync` bound on `PyCFunction::new_closure`). Also upgrades `pyo3-async-runtimes` to `0.29`.
- Declare a Rust **1.83** MSRV (PyO3 0.29 requirement) across all crates.

### Fixed
- Correct crate metadata `repository`/`homepage` URLs to `github.com/pulseai-labs/PulseHive` (previously the stale `pulsehive/pulsehive` path).

### Changed
- Security/CI hardening: `cargo-deny` + `cargo-audit` gates (SHA-pinned actions), read-only workflow `GITHUB_TOKEN`, Dependabot config, `SECURITY.md` / `LICENSING.md` / `PUBLIC_BOUNDARY.md`, and a hardened `.gitignore`.

## [2.0.0] - 2026-03-26

### Breaking Changes — PulseVision-Ready Events

#### HiveEvent Enrichment (BREAKING)
- All 14 `HiveEvent` variants now include `timestamp_ms: u64` (epoch milliseconds)
- `HiveEvent` now derives `Serialize, Deserialize` — events are JSON-serializable for WebSocket transmission
- `AgentOutcome` and `AgentKindTag` now derive `Serialize, Deserialize`
- `LlmCallCompleted`: added `input_tokens: u32`, `output_tokens: u32` (token usage tracking)
- `ToolCallStarted`: added `params: String` (JSON-stringified tool arguments)
- `ToolCallCompleted`: added `result_preview: String` (first 200 chars of tool result)
- `ExperienceRecorded`: added `content_preview: String`, `experience_type: String`, `importance: f32`
- `RelationshipInferred`: added `agent_id: String` (was missing — couldn't correlate to agent)
- `InsightGenerated`: added `agent_id: String` (same)

#### New: EventExporter Trait
- `pulsehive_core::export::EventExporter` trait for streaming events to external systems (PulseVision)
- `HiveMindBuilder::event_exporter()` registration method
- Fire-and-forget export via `tokio::spawn` — zero latency on emit path

#### Other Changes
- Upgraded `pulsehive-db` dependency from 0.2 → 0.4 (PulseVision-ready APIs)
- `TokenUsage` now derives `Serialize, Deserialize`
- `now_ms()` public helper for epoch millisecond timestamps
- Python + JS bindings updated with all new event fields
- 233 Rust tests (up from 229), all passing

## [1.0.0] - 2026-03-25

### Production Release — PulseHive v1.0.0

PulseHive is production-ready with full support for Rust, Python, and TypeScript. 16 sprints, 138 tickets, 224 Rust tests + 52 Python tests + 47 TypeScript tests.

#### Advanced Features (Sprint 15)
- `EmbeddingProvider` trait: `embed()`, `embed_batch()`, `dimensions()` for domain-specific embedding models
- `HiveMindBuilder::embedding_provider()` registration — PulseDB External mode when provider set
- Embedding computation in experience recording pipeline with graceful degradation
- `AttractorDynamics` struct: `strength`, `radius`, `warp_factor` computed at query time
- `influence_at()` with cosine distance, linear falloff within radius
- `AttractorConfig` with configurable defaults (radius=0.3, warp=1.0, boost=0.1)
- Perception `rerank()` enhanced with optional attractor boost (additive)
- `PulseHiveError::Embedding` variant + `HiveEvent::EmbeddingComputed` (14 event variants)
- `cosine_distance()` helper for embedding space computation
- `field_bench.rs` benchmark suite for field dynamics
- 5 embedding integration tests (mock provider, builtin fallback, graceful degradation)
- Python + TypeScript event bindings updated for `EmbeddingComputed` variant

#### Documentation and Release (Sprint 16)
- 3 Rust example applications: `cli_agent`, `multi_agent_workflow`, `custom_tool`
- Rustdoc completeness pass: zero `cargo doc` warnings across all crates
- Getting Started guide for Rust, Python, and TypeScript (`docs/getting-started.md`)
- `CONTRIBUTING.md` with development setup, code quality, PR process
- Performance benchmarks published (`docs/benchmarks.md`) — all targets met
- Version bump: all crates 0.1.0 → 1.0.0, pulsehive-py 0.3.0-beta.1 → 1.0.0, pulsehive-js 0.4.0-alpha.1 → 1.0.0

#### Performance Results (v1.0.0)
| Operation | 1K experiences | 10K experiences | Target (1K) |
|-----------|---------------|-----------------|-------------|
| `search_similar(k=20)` | 200 µs | 279 µs | < 1 ms |
| `get_recent(k=20)` | 95 µs | 588 µs | < 10 ms |
| `store_experience` | 7.0 ms | — | < 15 ms |
| `cosine_distance(384d)` | 357 ns | — | < 1 µs |
| `rerank(100 exp)` | 26 µs | — | < 1 ms |

## [0.4.0-alpha.1] - 2026-03-24

### Added — Phase 4: Ecosystem Expansion — TypeScript Bindings (Sprints 13-14)

#### TypeScript/Node.js Bindings (pulsehive-js)
- napi-rs 3.x-based Node.js bindings with `npm install @pulsehive/sdk` support
- Core types: `LlmConfig`, `Lens`, `RecencyCurve`, `AgentKind`, `AgentDefinition`, `AgentOutcome`
- `HiveMind` builder with fluent `.substratePath().llmProvider().build()` chaining
- `openaiProvider()` and `anthropicProvider()` factory functions
- Async `deploy()` returning `EventStream` consumable via `for await (const event of stream)`
- `Symbol.asyncIterator` on EventStream for idiomatic TypeScript iteration
- All 13 `HiveEvent` variants accessible with `.eventType`, `.data`, `.agentId`
- `Tool` class with `ThreadsafeFunction` for async Rust-JS tool callback bridging
- `defineTool()` ergonomic wrapper: typed params + context, no manual JSON serialization
- `ToolContext` with `agentId` and `collectiveId` accessible from JavaScript
- `ToolResult.text()`, `.json()`, `.error()` for tool return values
- Sequential, Parallel, and Loop workflow agents from TypeScript
- `cfg(feature = "napi")` gating for clean workspace compilation
- 47 TypeScript tests (unit + integration via vitest)
- 3 example scripts: getting-started.ts, custom-tools.ts, multi-agent.ts
- GitHub Actions CI: Node 18/20 matrix for test validation
- GitHub Actions npm release workflow for cross-platform prebuilds (macOS arm64, Linux x64, Windows x64)

## [0.3.0-beta] - 2026-03-24

### Added — Phase 3: Polish + Python Bindings (Sprints 9-12)

#### Python Bindings (pulsehive-py)
- PyO3-based Python bindings with `pip install pulsehive` support
- Core types: `LlmConfig`, `Lens`, `RecencyCurve`, `AgentKind`, `AgentDefinition`, `AgentOutcome`
- `HiveMind` builder with method chaining from Python
- `openai_provider()` and `anthropic_provider()` factory functions
- Async `deploy()` returning `EventStream` consumable via `async for event in stream`
- All 13 `HiveEvent` variants accessible with `.event_type`, `.data`, `.agent_id`
- Python Tool bridge: define tools as plain Python classes (duck-typing protocol)
- `ToolContext` with `agent_id` and `collective_id` accessible from Python
- `ToolResult.text()`, `.json()`, `.error()` for tool return values
- Fail-fast validation: missing tool methods raise `TypeError` at construction time
- Sequential, Parallel, and Loop workflow agents from Python
- 52 Python tests (unit + integration + tool protocol)
- 3 example scripts: getting_started.py, multi_agent.py, custom_tools.py
- GitHub Actions workflow for cross-platform wheel builds (macOS arm64, Linux x86_64, Windows x86_64)

#### Observability (Sprint 9)
- Structured tracing spans for Perceive/Think/Act/Record phases
- Spans on dispatch_agent, query_substrate, infer_relations, synthesize_insight
- Compatible with tracing-subscriber::fmt and tracing-opentelemetry
- CLIApproval handler doc example with all 3 approval paths

#### Error Recovery (Sprint 9)
- Partial experience recording: errors after tool calls produce partial_completion + error experiences
- `HiveMind::shutdown()` with `AtomicBool` flag for Watch task graceful termination
- `Drop` impl triggers shutdown non-blockingly
- `HiveMind::redeploy()` for restarting failed agents

#### Human-in-the-Loop (Sprint 9)
- Integration tests for all 3 `ApprovalResult` variants (Approved, Denied, Modified)
- Denied tool flow: LLM informed → alternative action chosen
- `ToolApprovalRequested` event verified in event stream

## [0.2.0-alpha] - 2026-03-20

### Added — Phase 2: Multi-Agent Intelligence (Sprints 5-8)
- Workflow agents: Sequential, Parallel, Loop with recursive dispatch via `Box::pin`
- Shared consciousness: children perceive previous agents' experiences via substrate
- Mid-task substrate refresh (`refresh_every_n_tool_calls`)
- Watch system: real-time substrate change notifications via `WatchNotification` events
- `RelationshipDetector`: automatic relation inference using embedding similarity + ExperienceType heuristics
- `InsightSynthesizer`: LLM-based cluster synthesis with BFS traversal + debouncing
- `ContextOptimizer`: 72-hour exponential decay + reinforcement boost + insights-first priority
- Anthropic Claude provider (`pulsehive-anthropic`): chat with tool_use, content blocks, retry on 429/529
- 13 `HiveEvent` variants covering full agent lifecycle
- `AgentDefinition` is `Clone` (Arc-based tools and extractors)

## [0.1.0-alpha] - 2026-03-17

### Added — Phase 1: Core SDK Foundation (Sprints 1-4)
- Core primitives: `HiveMind`, `Agent`, `Tool`, `Lens`, `Experience`
- `pulsehive-core`: traits for Agent, Tool, Lens, LlmProvider, HiveEvent, ApprovalHandler
- `pulsehive-runtime`: HiveMind orchestrator, agentic loop (Perceive→Think→Act→Record)
- `pulsehive-openai`: OpenAI-compatible provider (chat, SSE streaming, retry)
- `pulsehive`: meta-crate with feature flags (`openai`, `anthropic`)
- PulseDB integration via `SubstrateProvider` trait
- Perception pipeline: query → re-rank through lens → budget pack → format as intrinsic knowledge
- `DefaultExperienceExtractor`: rule-based extraction for Complete/Error/MaxIterations outcomes
- `ContextBudget`: token and experience count limits for context assembly
- Event streaming via `tokio::sync::broadcast`
- Builder pattern for `HiveMind` construction with validated configuration
