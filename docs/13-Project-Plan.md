# PulseHive SDK — Project Plan

> **Document ID:** PP-PH-001
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** PRD-PH-001, SRS-PH-001, BL-PH-001
> **Duration:** 16 weeks (4 phases of 4 weeks each)

---

## 1. Project Overview

**Project:** PulseHive SDK — Shared Consciousness SDK for Multi-Agent AI Systems
**Developer:** Draco (solo) with Claude Code AI-assisted development
**Start date:** 2026-03-17
**Target completion:** 2026-07-06

### Development Model

Solo developer with AI pair programming. Each sprint (1 week) follows a consistent rhythm:
- **Monday:** Sprint planning, story selection, architecture decisions
- **Tuesday-Thursday:** Implementation with Claude Code assistance
- **Friday:** Testing, documentation, sprint review

Estimated velocity: 12-15 story points per sprint. Phases are time-boxed; unfinished work rolls to next sprint within the phase. If a phase runs over, the subsequent phase start shifts.

---

## 2. Phase 1: Foundation (Weeks 1-4)

**Goal:** Core traits, single LlmAgent with tool use, PulseDB integration, event streaming.
**Deliverable:** Deploy one agent, have it use tools, record experiences to substrate, consume events.
**Stories:** E1-S01 through E1-S11 (62 SP)

### Week 1: Workspace Setup and Core Traits (Sprint 1)

**Target SP:** 11 (E1-S01: 5, E1-S02: 3, E1-S03: 3)

| Day | Deliverable |
|-----|-------------|
| Mon | Cargo workspace init: `pulsehive/`, `pulsehive-core/`, `pulsehive-runtime/`, `pulsehive-openai/`, `pulsehive-anthropic/` (stubs). Workspace Cargo.toml with shared deps. CI setup (cargo test, clippy, fmt). |
| Tue | `pulsehive-core/src/agent.rs`: AgentDefinition, AgentKind, LlmAgentConfig, AgentOutcome. `pulsehive-core/src/tool.rs`: Tool trait, ToolContext, ToolResult. Unit tests for all types. |
| Wed | `pulsehive-core/src/lens.rs`: Lens struct, RecencyCurve, ExperienceTypeTag. `pulsehive-core/src/llm.rs`: LlmProvider trait, LlmConfig, Message, LlmResponse, LlmChunk, ToolDefinition. |
| Thu | `pulsehive-core/src/event.rs`: HiveEvent enum (all variants), EventEmitter. `pulsehive-core/src/approval.rs`: ApprovalHandler trait, PendingAction, ApprovalResult. `pulsehive-core/src/error.rs`: PulseHiveError enum. |
| Fri | `pulsehive-runtime/src/hivemind.rs`: HiveMind struct, HiveMindBuilder with substrate_path() and llm_provider(). Builder validates and constructs. Integration test: build a HiveMind successfully. |

**Exit criteria:** All core traits compile. HiveMindBuilder constructs a HiveMind with PulseDB substrate. `cargo test --workspace` passes.

### Week 2: OpenAI Provider and Agentic Loop (Sprint 2)

**Target SP:** 21 (E1-S04: 3, E1-S05: 8, E1-S06: 13) — stretch sprint

| Day | Deliverable |
|-----|-------------|
| Mon | `pulsehive-core/src/llm.rs` refinement: finalize Message serialization format matching OpenAI chat API. LlmProvider trait — confirm chat() and chat_stream() signatures work for both OpenAI and Anthropic. |
| Tue | `pulsehive-openai/src/lib.rs`: OpenAICompatibleProvider. HTTP client (reqwest) for chat completions. Request/response serialization. Tool definition mapping to OpenAI function format. |
| Wed | `pulsehive-openai`: SSE streaming parser for chat_stream(). Rate limit retry logic. Integration test with OpenAI API (or mock server). Test with configurable base_url for non-OpenAI endpoints. |
| Thu | `pulsehive-runtime/src/loop.rs`: Agentic loop engine. Perceive phase (stub — returns empty context). Think phase (sends messages to LLM via provider). Act phase (parses tool calls, executes tools, loops). Record phase (stub — records nothing yet). |
| Fri | Agentic loop: wire up deploy() to create agent runtime and execute loop. Integration test: deploy one agent with a mock LLM provider and a test tool. Verify tool call -> tool execute -> LLM response cycle. |

**Exit criteria:** Single agent completes a task using a tool via OpenAI-compatible API. deploy() returns event stream with AgentStarted/AgentCompleted.

### Week 3: PulseDB Integration and Experience Recording (Sprint 3)

**Target SP:** 18 (E1-S07: 8, E1-S08: 5, E1-S09: 5)

| Day | Deliverable |
|-----|-------------|
| Mon | `pulsehive-runtime/src/hivemind.rs`: Wire PulseDBSubstrate into HiveMind. Test: create HiveMind with substrate_path, store experience, retrieve experience. Confirm Builtin embeddings auto-compute. |
| Tue | Lens perception pipeline: implement pre-search embedding warping (multiply query embedding by lens focus vector). Implement post-search re-ranking (domain weight, temporal decay, type weight). |
| Wed | Experience recording in agentic loop: default ExperienceExtractor that pulls key learnings from conversation history. Wire into Record phase. Test: agent completes task, experience appears in substrate. |
| Thu | HiveEvent streaming: wire EventEmitter through agentic loop. deploy() returns real event stream. Test: consume all expected events from a single agent session. |
| Fri | Re-export PulseDB types in `pulsehive-core` prelude. Create `pulsehive/` meta-crate with `use pulsehive::prelude::*`. Verify clean import from consumer perspective. |

**Exit criteria:** Agent perceives substrate through lens, records experiences after task completion. Events stream correctly. `use pulsehive::prelude::*` provides all needed types.

### Week 4: Context Assembly, Polish, and CLI Example (Sprint 4)

**Target SP:** 13 (E1-S10: 5, E1-S11: 3, + 5 SP polish/bugfix)

| Day | Deliverable |
|-----|-------------|
| Mon | ContextOptimizer: basic implementation with temporal decay and token budget packing. Intrinsic knowledge formatting ("You understand that..."). Wire into Perceive phase. |
| Tue | `examples/cli_agent.rs`: Full working example — creates HiveMind, defines agent with tool, deploys, prints events. README snippet showing < 50 lines to multi-agent system. |
| Wed | Persistence test: run CLI example twice. Second run should show agent perceiving first run's experiences. Validate substrate continuity. |
| Thu | Polish: error messages, rustdoc on all public APIs, cargo clippy clean, cargo fmt. Fix any integration issues found during example development. |
| Fri | Phase 1 retrospective. Run full test suite. Benchmark: substrate search latency at 100, 500, 1K experiences. Document results. Tag v0.1.0-alpha. |

**Exit criteria:** CLI example works end-to-end. Substrate persists across runs. All Phase 1 acceptance criteria from SRS met. Benchmarks recorded.

---

## 3. Phase 2: Multi-Agent and Intelligence (Weeks 5-8)

**Goal:** Workflow agents, multi-agent shared consciousness, intelligence layer.
**Deliverable:** Deploy multi-agent swarm, watch shared learning in real-time, see derived insights.
**Stories:** E2-S01 through E2-S09 (55 SP)

### Week 5: Workflow Agents (Sprint 5)

**Target SP:** 13 (E2-S01: 5, E2-S02: 5, E2-S03: 3)

| Day | Deliverable |
|-----|-------------|
| Mon | `pulsehive-runtime/src/workflow.rs`: Sequential executor. Takes Vec<AgentDefinition>, runs each in order via existing agentic loop. Each child gets same collective_id. |
| Tue | Parallel executor: spawn all children as Tokio tasks with `tokio::join!` or `JoinSet`. Shared substrate via Arc<dyn SubstrateProvider>. Collect all events into unified stream. |
| Wed | Loop executor: repeat child agent up to max_iterations. Detect completion signal (special tool call or LLM output pattern). |
| Thu | Integration tests: Sequential — 2 agents, second perceives first's experience. Parallel — 2 agents, both write and eventually see each other's work. Loop — agent iterates 3 times then stops. |
| Fri | Nested workflow test: Sequential(Parallel(A, B), C). Verify C sees A and B's experiences. Edge cases: empty workflow, single-child workflow. |

**Exit criteria:** All three workflow types execute correctly with proper substrate sharing.

### Week 6: Multi-Agent Watch Integration (Sprint 6)

**Target SP:** 8 (E2-S04: 8)

| Day | Deliverable |
|-----|-------------|
| Mon | Wire PulseDB Watch system into HiveMind. On deploy, subscribe to collective's WatchStream. Route WatchEvents to active agents. |
| Tue | Mid-task substrate refresh: configurable `refresh_every_n_tool_calls` (default 5). When triggered during Act phase, re-run Perceive to pick up new experiences from other agents. |
| Wed | Integration test: Agent A writes experience at tool call 3. Agent B (running in parallel) perceives it during its next refresh cycle. Verify via event stream. |
| Thu | Event stream merging: unify HiveEvents from multiple concurrent agents into single ordered stream. Verify no event loss under concurrent writes. |
| Fri | Stress test: 5 concurrent agents, each making 10 tool calls, all writing experiences. Verify substrate consistency and Watch delivery. |

**Exit criteria:** Parallel agents perceive each other's experiences in real-time via Watch system.

### Week 7: RelationshipDetector and InsightSynthesizer (Sprint 7)

**Target SP:** 16 (E2-S05: 8, E2-S06: 8)

| Day | Deliverable |
|-----|-------------|
| Mon | `pulsehive-runtime/src/intelligence/relationship.rs`: RelationshipDetector with configurable thresholds. search_similar top 20 on each new experience. |
| Tue | Relation type heuristics: Difficulty+Solution->Supports, ErrorPattern+ErrorPattern->Supersedes, opposing content->Contradicts. Wire into record_experience(). |
| Wed | `pulsehive-runtime/src/intelligence/insight.rs`: InsightSynthesizer. Graph traversal to find relation clusters. Threshold check for density. |
| Thu | LLM-based synthesis: construct prompt from cluster experiences, call LLM, parse into NewDerivedInsight. Debounce logic. Wire into record_experience() pipeline. |
| Fri | Integration test: store 6 related experiences, verify RelationshipDetector creates relations, verify InsightSynthesizer generates insight. Test with mock LLM for deterministic results. |

**Exit criteria:** Relationship inference and insight synthesis work automatically on experience recording.

### Week 8: ContextOptimizer, Anthropic Provider, Integration Tests (Sprint 8)

**Target SP:** 18 (E2-S07: 5, E2-S08: 5, E2-S09: 8)

| Day | Deliverable |
|-----|-------------|
| Mon | ContextOptimizer: full implementation with configurable decay half-life and reinforcement boost. Priority ordering: insights > high-importance > recent. Activity awareness formatting. |
| Tue | `pulsehive-anthropic/src/lib.rs`: AnthropicProvider implementing LlmProvider. Anthropic Messages API with tool_use. Streaming via SSE. |
| Wed | Anthropic provider: test with Claude Sonnet. Verify tool use round-trip. Handle Anthropic-specific content blocks (text, tool_use, tool_result). |
| Thu | End-to-end integration tests: (1) 3 parallel agents sharing experiences. (2) Sequential workflow with relationship inference. (3) Insight generation from experience cluster. (4) Full perception pipeline with lens, decay, and budget. |
| Fri | Phase 2 retrospective. Benchmark: multi-agent deployment overhead, Watch latency, relationship inference time. Tag v0.2.0-alpha. |

**Exit criteria:** All Phase 2 SRS acceptance criteria met. Both OpenAI and Anthropic providers working. Intelligence layer active.

---

## 4. Phase 3: Polish and Python Bindings (Weeks 9-12)

**Goal:** Human-in-the-loop, observability polish, Python bindings via PyO3.
**Deliverable:** Python developers can `pip install pulsehive` and build multi-agent systems.
**Stories:** E3-S01, E4-S01, E5-S01, E5-S02 (34 SP)

### Week 9: Human-in-the-Loop and Observability (Sprint 9)

| Day | Deliverable |
|-----|-------------|
| Mon-Tue | ApprovalHandler integration: wire requires_approval() check into agentic loop. Implement CLIApproval handler as example. Test: tool blocked, LLM informed, alternative chosen. |
| Wed-Thu | Structured tracing: add tracing spans to agentic loop, LLM calls, tool execution, substrate operations. Verify compatibility with tracing-subscriber::fmt and tracing-opentelemetry. |
| Fri | Error recovery: partial experience recording on agent failure. Agent restart capability. Graceful shutdown on HiveMind drop. |

### Week 10: PyO3 Scaffolding (Sprint 10)

| Day | Deliverable |
|-----|-------------|
| Mon | `pulsehive-py/` crate setup with PyO3 and maturin. Python project structure. Basic build pipeline: `maturin develop` compiles and installs locally. |
| Tue-Wed | Core type bindings: HiveMind, AgentDefinition, AgentKind, Lens, LlmConfig as Python classes with `#[pyclass]`. Constructor mapping. |
| Thu-Fri | Async bridge: pyo3-asyncio integration for deploy() and event stream consumption. Python `async for event in stream` pattern. |

### Week 11: Python Tool Interface (Sprint 11)

| Day | Deliverable |
|-----|-------------|
| Mon-Wed | Python Tool class: define tools in Python, bridge to Rust Tool trait. ToolContext accessible from Python. Handle Python exceptions -> Rust errors. |
| Thu-Fri | Python integration test: define agent and tool in Python, deploy, consume events. Verify experiences stored in substrate. |

### Week 12: Python Polish and Release (Sprint 12)

| Day | Deliverable |
|-----|-------------|
| Mon-Tue | Python examples: getting_started.py, multi_agent.py. Python-specific documentation. |
| Wed-Thu | PyPI packaging: build wheels for macOS (arm64), Linux (x86_64), Windows (x86_64). GitHub Actions workflow for automated builds. |
| Fri | Phase 3 retrospective. Performance comparison: Python vs Rust for same workload. Tag v0.3.0-beta. |

**Exit criteria:** `pip install pulsehive` works. Python developer can build and run a multi-agent system.

---

## 5. Phase 4: Ecosystem Expansion (Weeks 13-16)

**Goal:** TypeScript bindings, advanced features, documentation, community readiness.
**Deliverable:** Full SDK ecosystem ready for external developers.
**Stories:** E6-S01, E6-S02, E7-S01, E7-S02, E7-S03 (39 SP)

### Week 13-14: TypeScript Bindings (Sprint 13-14)

- `pulsehive-js/` crate with napi-rs
- Core type bindings: HiveMind, AgentDefinition, Lens as TypeScript classes with full type definitions
- Tool interface bridging (TypeScript class -> Rust trait)
- Async support via napi-rs async functions
- npm packaging with prebuilds for macOS, Linux, Windows

### Week 15: Advanced Features (Sprint 15)

- EmbeddingProvider trait: custom embedding models, bridged to PulseDB External mode
- AttractorDynamics: strength, radius, warp_factor computation at query time
- Performance optimization pass: profiling, hotspot identification, optimization

### Week 16: Documentation and Release (Sprint 16)

- Comprehensive rustdoc for all public APIs
- 3 example applications: single agent CLI, multi-agent research pipeline, pharmacovigilance workflow
- Getting Started guide for Rust, Python, TypeScript
- Changelog, migration guide, contributing guide
- Final benchmarks at 1K, 10K, 100K experiences
- Tag v1.0.0

**Exit criteria:** All three language targets working. Documentation complete. Benchmarks published.

---

## 6. Critical Path

The critical path runs through the core substrate integration, since everything depends on it:

```
Week 1: Core traits (pulsehive-core)
  |
  v
Week 2: LLM provider + Agentic loop
  |
  v
Week 3: PulseDB integration + Lens perception  <-- CRITICAL: if this slips, everything slips
  |
  v
Week 4: Context assembly + CLI example
  |
  v
Week 5: Workflow agents (depends on working agentic loop)
  |
  v
Week 6: Watch integration (depends on PulseDB substrate + workflows)
  |
  v
Week 7: Intelligence layer (depends on substrate + relationships)
  |
  v
Week 8: Integration tests + Anthropic provider
```

**Key risk point:** Week 3 (PulseDB integration). If PulseDB v0.1.1 has issues or the SubstrateProvider trait needs modifications, this is where it surfaces. Mitigation: Draco owns both codebases and can patch PulseDB directly.

Phases 3 and 4 (bindings) are independent of each other and can be parallelized or deferred without affecting the core SDK.

---

## 7. Resource Allocation

### Developer Time

| Activity | Allocation |
|----------|-----------|
| Implementation (with Claude Code) | 60% |
| Testing (unit + integration) | 20% |
| Design decisions and architecture | 10% |
| Documentation and examples | 10% |

### Infrastructure

| Resource | Purpose | Cost |
|----------|---------|------|
| GitHub repo | Source control, CI/CD | Free |
| GitHub Actions | CI: cargo test, clippy, fmt | Free tier |
| LLM API keys (OpenAI, Anthropic) | Integration testing | ~$50/month during development |
| crates.io | Rust crate publishing | Free |
| PyPI | Python package publishing | Free |
| npm | TypeScript package publishing | Free |

### External Dependencies

| Dependency | Risk | Mitigation |
|------------|------|------------|
| PulseDB v0.1.1 | API changes | Same developer owns both; pin version |
| OpenAI API | Rate limits during testing | Use mock server for unit tests; real API for integration |
| Anthropic API | API changes | Thin provider crate isolates changes |
| PyO3 | Compatibility with Rust nightly changes | Pin PyO3 version; use stable Rust |
| napi-rs | Node.js version compatibility | Pin napi-rs version; test on LTS Node.js |

---

## 8. Risk Mitigation Schedule

| Week | Risk Review | Action |
|------|------------|--------|
| 1 | Workspace and dependency setup | Verify PulseDB v0.1.1 compiles cleanly as dependency. Test SubstrateProvider trait. |
| 2 | OpenAI API compatibility | Test with at least 2 OpenAI-compatible endpoints (OpenAI + one alternative). |
| 3 | PulseDB integration | Run PulseDB benchmarks within PulseHive context. Verify Builtin embeddings work end-to-end. If issues, patch PulseDB same week. |
| 4 | Phase 1 scope | If behind, defer CLI example polish to Phase 2 Week 5. Core functionality takes priority. |
| 6 | Watch system reliability | Stress test with concurrent writes. If Watch has issues, fall back to polling with configurable interval. |
| 7 | Intelligence layer LLM dependency | If LLM-based classification is too slow/expensive, use heuristic-only mode as default. LLM classification becomes opt-in. |
| 8 | Phase 2 scope | If behind, defer integration tests to Sprint 9. Intelligence layer core takes priority over edge cases. |
| 10 | PyO3 async bridge | If pyo3-asyncio has issues, use synchronous Python API with background Tokio runtime. Less ergonomic but functional. |
| 13 | napi-rs compatibility | If napi-rs issues arise, defer TypeScript bindings to post-v1.0. Focus resources on Rust + Python quality. |

---

## 9. Milestones

| Milestone | Target Date | Deliverable | Gate Criteria |
|-----------|-------------|-------------|---------------|
| M1: Foundation Complete | Week 4 (2026-04-11) | v0.1.0-alpha | Single agent with tool use, PulseDB persistence, event streaming |
| M2: Multi-Agent Complete | Week 8 (2026-05-09) | v0.2.0-alpha | Workflow agents, shared consciousness, intelligence layer |
| M3: Python Ready | Week 12 (2026-06-06) | v0.3.0-beta | pip install pulsehive works, Python developer builds agent |
| M4: Ecosystem Ready | Week 16 (2026-07-06) | v1.0.0 | All bindings, documentation, benchmarks, examples |

---

## 10. Definition of Done

A story is done when:

1. Implementation compiles with zero warnings (`cargo clippy`)
2. Unit tests cover all public API methods
3. Integration tests pass for cross-crate interactions
4. Rustdoc exists for all public types and methods
5. Code formatted (`cargo fmt`)
6. No regressions in existing tests (`cargo test --workspace`)

A phase is done when:

1. All stories in the phase meet definition of done
2. Phase acceptance criteria from SRS Section 8 are met
3. Benchmarks recorded and compared against targets
4. Alpha/beta tag created in git

---

*This plan is a living document. Updated at each sprint retrospective based on actual velocity and learnings.*
