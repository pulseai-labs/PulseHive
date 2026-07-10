# Changelog

All notable changes to PulseHive will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.1.0] - Unreleased

### Added
- **Streaming tools** — a new `pulsehive_core::tool::StreamingTool: Tool` trait for long-running tools that report live progress. Tools expose it by overriding `Tool::as_streaming()` to return `Some(self)`; the agent loop then calls `StreamingTool::execute_streaming(params, context, progress_tx)` and forwards each pushed event.
- **`pulsehive_core::tool::ToolProgress`** enum — the progress payload (`Started` / `Progress { fraction, message }` / `PartialResult` / `Log` / `Completed { duration_ms }`). The `Started` / `Completed` bookends are emitted by the agent loop; tool bodies push the intermediate variants.
- **`HiveEvent::ToolProgress { agent_id, tool_name, progress }`** — the agent loop forwards each `ToolProgress` from a streaming tool as this event on the `HiveMind::deploy()` stream, so consumers see live progress instead of a frozen wait.
- New runnable, hermetic example `pulsehive-runtime/examples/streaming_tool.rs` (no API key) and a "Streaming Tools" section in `docs/05-API-Spec.md`.

### Changed
- **BREAKING: `HiveEvent` is now `#[non_exhaustive]`.** New event variants (such as `ToolProgress`) can be added in a minor release without a major bump. External code that matches on `HiveEvent` exhaustively must add a `_ => {}` catch-all arm, or it will fail to compile.

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
