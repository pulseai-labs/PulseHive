# Changelog

All notable changes to PulseHive will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
