# PulseHive SDK — Product Backlog

> **Document ID:** BL-PH-001
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** SRS-PH-001, PRD-PH-001

---

## Estimation Guide

Story points use Fibonacci scale: 1, 2, 3, 5, 8, 13. Baseline: 1 SP = a single-file trait definition with unit tests. 13 SP = multi-file subsystem with integration tests, PulseDB interaction, and LLM calls.

---

## Epic 1: Core SDK (Phase 1 — Sprints 1-4)

### E1-S01: HiveMind Builder and Orchestrator
**As a** Rust developer, **I want** to create a HiveMind via builder pattern **so that** I can configure substrate and LLM providers with compile-time safety.
- **AC:** `HiveMind::builder().substrate_path("path").llm_provider("name", provider).build()` returns `Result<HiveMind>`
- **AC:** Missing substrate configuration returns a descriptive error
- **AC:** Multiple LLM providers can be registered by name
- **SP:** 5 | **Sprint:** 1 | **FR:** FR-001, FR-002

### E1-S02: Agent Definition and AgentKind Enum
**As a** developer, **I want** to define agents as data (AgentDefinition + AgentKind) **so that** I can compose agent systems declaratively.
- **AC:** `AgentKind::Llm(config)` holds system_prompt, tools, lens, llm_config
- **AC:** Workflow variants (Sequential, Parallel, Loop) compile but are not yet executed (Phase 2)
- **AC:** AgentDefinition is Clone + Debug
- **SP:** 3 | **Sprint:** 1 | **FR:** FR-005

### E1-S03: Tool Trait and ToolContext
**As a** product developer, **I want** to implement the Tool trait **so that** my agents can perform domain-specific actions.
- **AC:** Tool trait has name(), description(), parameters(), execute(), requires_approval()
- **AC:** ToolContext provides agent_id, collective_id, substrate Arc, event_emitter
- **AC:** ToolResult supports text, JSON, and error variants
- **SP:** 3 | **Sprint:** 1 | **FR:** FR-006, FR-007

### E1-S04: LlmProvider Trait and Message Types
**As a** developer, **I want** an LLM abstraction **so that** I can swap providers without changing agent logic.
- **AC:** LlmProvider trait defines chat() and chat_stream()
- **AC:** Message type supports System, User, Assistant, ToolCall, ToolResult roles
- **AC:** LlmResponse contains text, optional tool_calls, and usage stats
- **SP:** 3 | **Sprint:** 1 | **FR:** FR-012

### E1-S05: OpenAI-Compatible LLM Provider
**As a** developer, **I want** to use any OpenAI-compatible API **so that** I can use GLM, vLLM, Ollama, or OpenAI.
- **AC:** Accepts api_key, base_url, model configuration
- **AC:** Sends chat completions requests with tool definitions
- **AC:** Parses streaming responses (SSE) into LlmChunk stream
- **AC:** Handles rate limit errors with configurable retry
- **SP:** 8 | **Sprint:** 2 | **FR:** FR-013

### E1-S06: Agentic Loop Engine
**As a** developer, **I want** the framework to run the Perceive-Think-Act-Record loop **so that** I only define what the agent knows and can do.
- **AC:** Loop perceives substrate, sends to LLM, handles tool calls, records experiences
- **AC:** Tool calls loop back to Think phase; final response triggers Record phase
- **AC:** Max iterations enforced (default 25)
- **AC:** Each phase emits appropriate HiveEvents
- **SP:** 13 | **Sprint:** 2 | **FR:** FR-004

### E1-S07: Lens Struct and Perception Pipeline
**As a** developer, **I want** to configure how each agent perceives the substrate **so that** different agents see different aspects of shared knowledge.
- **AC:** Lens struct with domain_focus, type_weights, recency_curve, purpose_embedding, attention_budget
- **AC:** Convenience constructor: `Lens::new(vec!["domain1", "domain2"])`
- **AC:** Pre-search embedding warping transforms query through lens
- **AC:** Post-search re-ranking applies domain weights, temporal decay, type weights
- **SP:** 8 | **Sprint:** 3 | **FR:** FR-008, FR-009, FR-010

### E1-S08: PulseDB Substrate Integration
**As a** developer, **I want** HiveMind to store and retrieve experiences via PulseDB **so that** agent knowledge persists across sessions.
- **AC:** `substrate_path("path.db")` creates PulseDBSubstrate with Builtin embeddings
- **AC:** record_experience() stores via SubstrateProvider
- **AC:** Re-exports all necessary PulseDB types in pulsehive prelude
- **SP:** 5 | **Sprint:** 3 | **FR:** FR-002, FR-011

### E1-S09: HiveEvent Enum and Event Streaming
**As a** developer, **I want** to observe all agent activity through a typed event stream **so that** I can build UIs, logs, and monitoring.
- **AC:** HiveEvent covers agent lifecycle, LLM calls, tool execution, substrate ops, perception
- **AC:** deploy() returns Stream<Item = HiveEvent>
- **AC:** Events integrate with tracing crate for subscriber compatibility
- **SP:** 5 | **Sprint:** 3 | **FR:** FR-014, FR-003

### E1-S10: Context Assembly and Intrinsic Knowledge Formatting
**As a** developer, **I want** the framework to assemble relevant context within a token budget **so that** agents receive the most important knowledge.
- **AC:** Queries PulseDB via get_context_candidates()
- **AC:** Applies lens-based re-ranking
- **AC:** Packs within ContextBudget (configurable max tokens)
- **AC:** Formats as "You understand that..." intrinsic knowledge, not "Retrieved:"
- **SP:** 5 | **Sprint:** 4 | **FR:** FR-009, FR-010

### E1-S11: CLI Example Application
**As a** developer evaluating PulseHive, **I want** a working CLI example **so that** I can see the SDK in action.
- **AC:** `examples/cli_agent.rs` deploys one agent with a dummy tool
- **AC:** Prints event stream to stdout
- **AC:** Demonstrates substrate persistence (run twice, second run perceives first run's experiences)
- **SP:** 3 | **Sprint:** 4 | **FR:** —

---

## Epic 2: Intelligence Layer (Phase 2 — Sprints 5-8)

### E2-S01: Workflow Agent Execution — Sequential
**As a** developer, **I want** Sequential workflows **so that** I can chain agents where each builds on the previous one's output.
- **AC:** Children execute in order; each starts only after previous completes
- **AC:** Later children perceive earlier children's experiences via substrate
- **SP:** 5 | **Sprint:** 5 | **FR:** FR-015

### E2-S02: Workflow Agent Execution — Parallel
**As a** developer, **I want** Parallel workflows **so that** multiple agents can work concurrently on the same substrate.
- **AC:** All children spawned as Tokio tasks
- **AC:** Children share substrate and perceive each other's experiences in real-time
- **AC:** Parallel block completes when all children complete
- **SP:** 5 | **Sprint:** 5 | **FR:** FR-016

### E2-S03: Workflow Agent Execution — Loop
**As a** developer, **I want** Loop workflows **so that** an agent can iterate on a task until satisfied.
- **AC:** Repeats child up to max_iterations
- **AC:** Early termination on completion signal
- **AC:** Each iteration sees cumulative experiences
- **SP:** 3 | **Sprint:** 5 | **FR:** FR-017

### E2-S04: Multi-Agent Deployment with Watch Integration
**As a** developer, **I want** parallel agents to perceive each other's work in real-time **so that** shared consciousness is instantaneous.
- **AC:** HiveMind subscribes to Watch system for active collective
- **AC:** Mid-task substrate refresh (re-perceive every N tool calls, configurable)
- **AC:** New experiences from other agents visible in next perception cycle
- **SP:** 8 | **Sprint:** 6 | **FR:** FR-016

### E2-S05: RelationshipDetector
**As a** developer, **I want** automatic relationship inference **so that** the knowledge graph grows organically.
- **AC:** Finds top 20 similar experiences on each new recording
- **AC:** Auto-creates relations above 0.85 similarity with type heuristics
- **AC:** Configurable thresholds
- **AC:** Emits RelationshipInferred events
- **SP:** 8 | **Sprint:** 7 | **FR:** FR-018

### E2-S06: InsightSynthesizer
**As a** developer, **I want** automatic insight generation **so that** agents benefit from consolidated knowledge.
- **AC:** Triggers when cluster density exceeds threshold (default 5)
- **AC:** Uses LLM to synthesize cluster into DerivedInsight
- **AC:** Insight stored with own embedding for future search
- **AC:** Debounced to avoid redundant synthesis
- **SP:** 8 | **Sprint:** 7 | **FR:** FR-019

### E2-S07: ContextOptimizer with Temporal Decay
**As a** developer, **I want** context to favor recent, reinforced, and important experiences **so that** agents stay current.
- **AC:** Exponential decay: `importance * 0.5^(hours/72) * (1 + count * 0.1)`
- **AC:** Priority order: insights > high-importance > recent
- **AC:** Token budget packing
- **AC:** Activity awareness included
- **SP:** 5 | **Sprint:** 8 | **FR:** FR-020

### E2-S08: Anthropic LLM Provider
**As a** developer, **I want** native Claude support **so that** I can use Opus, Sonnet, or Haiku with full tool use.
- **AC:** Implements LlmProvider for Anthropic Messages API
- **AC:** Supports tool use (tool_choice, tool definitions)
- **AC:** Streaming support via SSE
- **AC:** Handles Anthropic-specific error codes
- **SP:** 5 | **Sprint:** 8 | **FR:** FR-012

### E2-S09: Phase 2 Integration Tests
**As a** developer, **I want** end-to-end tests proving multi-agent shared consciousness works **so that** I can trust the system.
- **AC:** Test: 3 parallel agents, each writes experience, each perceives others' experiences
- **AC:** Test: Sequential workflow where agent 2 references agent 1's experience
- **AC:** Test: RelationshipDetector creates relation between related experiences
- **AC:** Test: InsightSynthesizer generates insight from cluster
- **SP:** 8 | **Sprint:** 8 | **FR:** All Phase 2 FRs

---

## Epic 3: Observability (Phase 2-3 — Sprints 6-10)

### E3-S01: Structured Tracing Integration
**As a** developer, **I want** all operations to emit tracing spans **so that** I can plug in any tracing subscriber.
- **AC:** Every agentic loop phase creates a tracing span
- **AC:** LLM calls, tool executions, substrate operations have structured fields
- **AC:** Compatible with tracing-subscriber, tracing-opentelemetry
- **SP:** 5 | **Sprint:** 9 | **FR:** FR-014

---

## Epic 4: Human-in-the-Loop (Phase 3 — Sprints 9-10)

### E4-S01: ApprovalHandler Trait and Integration
**As a** product developer, **I want** to require human approval for sensitive tool calls **so that** critical actions have oversight.
- **AC:** ApprovalHandler trait with request_approval() method
- **AC:** Agentic loop checks requires_approval() before tool execution
- **AC:** Supports Approved, Denied, Modified responses
- **AC:** Denied informs LLM to choose alternative
- **SP:** 5 | **Sprint:** 9 | **FR:** FR-021

---

## Epic 5: Python Bindings (Phase 3 — Sprints 10-12)

### E5-S01: PyO3 Core Bindings
**As a** Python developer, **I want** to use PulseHive from Python **so that** I get Rust performance with Python ergonomics.
- **AC:** `pip install pulsehive` works
- **AC:** HiveMind, AgentDefinition, Lens, LlmConfig available as Python classes
- **AC:** Async support via pyo3-asyncio (or equivalent)
- **SP:** 13 | **Sprint:** 10-11 | **FR:** PRD F-18

### E5-S02: Python Tool Interface
**As a** Python developer, **I want** to define tools as Python classes **so that** I can use my existing Python libraries.
- **AC:** Python class with name, description, parameters, execute methods
- **AC:** Automatic bridging to Rust Tool trait
- **AC:** ToolContext accessible from Python
- **SP:** 8 | **Sprint:** 11-12 | **FR:** PRD F-18

---

## Epic 6: TypeScript Bindings (Phase 4 — Sprints 13-15)

### E6-S01: napi-rs Core Bindings
**As a** TypeScript developer, **I want** to use PulseHive from Node.js **so that** I can build agent systems in my preferred language.
- **AC:** `npm install pulsehive` works
- **AC:** HiveMind, AgentDefinition, Lens available as TypeScript classes with types
- **AC:** Async support via napi-rs async functions
- **SP:** 13 | **Sprint:** 13-14 | **FR:** PRD F-19

### E6-S02: TypeScript Tool Interface
**As a** TypeScript developer, **I want** to define tools as TypeScript classes **so that** I can leverage the Node.js ecosystem.
- **AC:** TypeScript interface for Tool with name, description, parameters, execute
- **AC:** Automatic bridging to Rust Tool trait
- **SP:** 8 | **Sprint:** 14-15 | **FR:** PRD F-19

---

## Epic 7: Ecosystem and Polish (Phase 4 — Sprints 15-16)

### E7-S01: EmbeddingProvider Trait
**As a** developer, **I want** to use domain-specific embedding models **so that** my substrate search is optimized for my domain.
- **AC:** EmbeddingProvider trait: embed(), embed_batch(), dimensions()
- **AC:** When set on HiveMind, PulseHive computes embeddings before passing to PulseDB External mode
- **AC:** Falls back to PulseDB Builtin when not set
- **SP:** 5 | **Sprint:** 15 | **FR:** PRD F-20

### E7-S02: Advanced Field Dynamics
**As a** developer, **I want** attractor dynamics (strength, radius, warping) **so that** strong knowledge patterns attract agent attention.
- **AC:** AttractorDynamics computed at query time from experience fields
- **AC:** High-strength attractors influence nearby queries
- **AC:** Configurable warp_factor
- **SP:** 8 | **Sprint:** 15-16 | **FR:** —

### E7-S03: Documentation, Examples, and Templates
**As a** developer, **I want** comprehensive docs and examples **so that** I can get started quickly.
- **AC:** rustdoc for all public APIs
- **AC:** 3 example applications: single agent, multi-agent workflow, custom tool
- **AC:** Getting Started guide
- **SP:** 5 | **Sprint:** 16 | **FR:** —

---

## Backlog Summary

| Epic | Stories | Total SP | Phase | Sprints |
|------|---------|----------|-------|---------|
| E1: Core SDK | 11 | 62 | 1 | 1-4 |
| E2: Intelligence Layer | 9 | 55 | 2 | 5-8 |
| E3: Observability | 1 | 5 | 2-3 | 9 |
| E4: Human-in-the-Loop | 1 | 5 | 3 | 9 |
| E5: Python Bindings | 2 | 21 | 3 | 10-12 |
| E6: TypeScript Bindings | 2 | 21 | 4 | 13-15 |
| E7: Ecosystem | 3 | 18 | 4 | 15-16 |
| **Total** | **29** | **187** | — | 1-16 |

Average velocity target: ~12 SP/sprint (solo developer + AI assistance).

---

*This backlog is refined at the start of each sprint. Stories may be re-prioritized based on learnings.*
