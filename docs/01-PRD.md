# PulseHive SDK — Product Requirements Document

> **Document ID:** PRD-PH-001
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Approved for Phase 1 Development
> **Reference:** SPEC v0.4.0

---

## 1. Product Vision

PulseHive is a Rust SDK for building multi-agent AI systems where agents share consciousness through a persistent substrate (PulseDB) instead of passing messages. When one agent learns something, all agents in that collective immediately perceive it — no coordination protocol, no message queue, no explicit sharing.

**Vision statement:** Enable any developer to build production-grade multi-agent AI systems in under 50 lines of code, with shared consciousness that compounds intelligence over time.

**Core innovation:** Agents do not communicate — they share consciousness. PulseHive makes the shared-state model database-native through PulseDB, providing real-time reactivity, semantic queryability, and persistence that in-memory approaches cannot match.

---

## 2. Goals and Objectives

### 2.1 Primary Goals

| ID | Goal | Success Criterion |
|----|------|-------------------|
| G-01 | Provide a minimal, composable SDK for multi-agent AI systems | 5 core primitives, zero product-specific code |
| G-02 | Enable shared consciousness through PulseDB substrate | Agents perceive each other's experiences without message passing |
| G-03 | Support heterogeneous LLM providers per agent | Anthropic Claude + any OpenAI-compatible API (GLM, vLLM, Ollama) |
| G-04 | Ship with intelligence that compounds over time | RelationshipDetector, InsightSynthesizer, ContextOptimizer |
| G-05 | Make Rust-native multi-agent systems accessible to Python/TS developers | PyO3 and napi-rs bindings in Phases 3-4 |

### 2.2 Success Metrics (KPIs)

| Metric | Target | Measurement |
|--------|--------|-------------|
| Time to first agent | < 15 minutes | From `cargo add` to running agent with tool use |
| Lines of code for multi-agent system | < 50 | Agent definition + deployment + event consumption |
| Substrate search latency (1K experiences) | < 1ms | `search_similar(k=20)` benchmark |
| Context assembly latency (1K experiences) | < 10ms | Full lens perception + ranking pipeline |
| Experience recording latency | < 15ms | Store + relationship inference (no LLM classification) |
| Agent deployment overhead | < 5ms | Time from `deploy()` call to first agent starting |
| Crate compile time (clean build) | < 60s | `cargo build --release` on M-series Mac |
| API surface stability | 0 breaking changes post-1.0 | Trait signatures frozen after stabilization |

---

## 3. User Personas

### 3.1 Persona 1: Rust Systems Developer (Primary — Phase 1-2)

**Name:** Maya
**Background:** Senior Rust developer building backend services. Comfortable with async, traits, and crate ecosystems. Wants to add AI agent capabilities to an existing Rust application.
**Goals:** Embed a multi-agent AI system into a Rust server without leaving the Rust ecosystem. Needs compile-time safety, zero-cost abstractions, and predictable performance.
**Pain points:** LangChain/CrewAI are Python-only. Existing Rust AI libraries are thin wrappers with no orchestration. No shared-state model available in Rust.
**Usage:** Imports `pulsehive` crate, defines custom tools implementing the `Tool` trait, configures agents with lenses, deploys via `HiveMind::deploy()`.

### 3.2 Persona 2: Python AI/ML Developer (Phase 3)

**Name:** Jordan
**Background:** Data scientist or ML engineer who builds AI pipelines in Python. Has used LangChain or CrewAI. Frustrated by their instability, over-abstraction, or lack of true shared state.
**Goals:** Get the performance and reliability of Rust with the ergonomics of Python. Shared consciousness model for multi-agent research or production workloads.
**Pain points:** LangChain has unstable APIs and too many abstractions. CrewAI uses message passing which loses context. Neither persists agent learning across sessions.
**Usage:** `pip install pulsehive`, uses Python API that mirrors the Rust API closely. Defines tools as Python classes, deploys agents in `async` context.

### 3.3 Persona 3: Enterprise R&D Team (Phase 4+)

**Name:** The Meridian Labs Team
**Background:** 3-5 person R&D team at a pharma/fintech/defense company. Building domain-specific multi-agent systems (pharmacovigilance, risk analysis, intelligence fusion). Need audit trails, observability, and compliance.
**Goals:** Deploy a multi-agent pipeline where specialized agents (safety, finance, legal) share findings through a persistent substrate. Need structured observability, human-in-the-loop approval for critical actions, and deterministic workflow orchestration.
**Pain points:** In-house solutions lack shared state. Commercial platforms are black boxes. Need to self-host for compliance. Need audit trails for regulatory review.
**Usage:** Full SDK with custom tools, approval handlers, OpenTelemetry integration, workflow agents for deterministic pipelines, LLM agents for reasoning tasks.

---

## 4. Core Features

### 4.1 P0 — Must Have (Phase 1)

| ID | Feature | Description |
|----|---------|-------------|
| F-01 | HiveMind orchestrator | Builder pattern, substrate management, agent deployment |
| F-02 | LLM Agent with agentic loop | Perceive-Think-Act-Record cycle with tool use |
| F-03 | Tool trait | Pluggable capabilities with JSON Schema parameters |
| F-04 | Lens perception | Domain filtering, type weighting, recency curve |
| F-05 | PulseDB substrate integration | Experience storage, search, context assembly via SubstrateProvider |
| F-06 | OpenAI-compatible LLM provider | Supports OpenAI, GLM, vLLM, LM Studio, Ollama |
| F-07 | Event streaming | `deploy()` returns `Stream<Item = HiveEvent>` for all lifecycle events |
| F-08 | Experience recording | Automatic extraction from agent sessions, stored in substrate |
| F-09 | Context assembly | Lens-warped query + PulseDB search + post-search re-ranking |

### 4.2 P1 — Should Have (Phase 2)

| ID | Feature | Description |
|----|---------|-------------|
| F-10 | Workflow agents | Sequential, Parallel, Loop — deterministic orchestration without LLM overhead |
| F-11 | Multi-agent deployment | Multiple agents sharing one substrate with real-time Watch integration |
| F-12 | RelationshipDetector | Automatic inference of Supports/Contradicts/Elaborates/Supersedes relations |
| F-13 | InsightSynthesizer | Cross-experience insight generation from relation clusters |
| F-14 | ContextOptimizer | Temporal decay, reinforcement boost, token budget packing |
| F-15 | Anthropic LLM provider | Claude models (Opus, Sonnet, Haiku) |
| F-16 | Mid-task substrate refresh | Agents perceive changes from other agents during long tasks |

### 4.3 P2 — Nice to Have (Phase 3-4)

| ID | Feature | Description |
|----|---------|-------------|
| F-17 | Human-in-the-loop | ApprovalHandler trait, PendingAction, Approved/Denied/Modified responses |
| F-18 | Python bindings | PyO3-based `pulsehive-py` crate published to PyPI |
| F-19 | TypeScript bindings | napi-rs-based `pulsehive-js` published to npm |
| F-20 | EmbeddingProvider trait | Domain-specific embedding models (medical, code, multilingual) |
| F-21 | Advanced observability | Structured tracing spans, OpenTelemetry compatibility |

---

## 5. Functional Requirements

| ID | Requirement | Priority | Feature |
|----|-------------|----------|---------|
| FR-001 | HiveMind SHALL be constructable via builder pattern with compile-time validation | P0 | F-01 |
| FR-002 | HiveMind SHALL accept a `SubstrateProvider` for PulseDB integration | P0 | F-01, F-05 |
| FR-003 | HiveMind SHALL deploy agents and return a `Stream<Item = HiveEvent>` | P0 | F-01, F-07 |
| FR-004 | LlmAgent SHALL execute the agentic loop: Perceive, Think, Act, Record | P0 | F-02 |
| FR-005 | LlmAgent SHALL support configurable tools, lens, system prompt, and LLM config | P0 | F-02, F-03, F-04 |
| FR-006 | Tool trait SHALL define name, description, JSON Schema parameters, and async execute | P0 | F-03 |
| FR-007 | Tools SHALL receive ToolContext with agent_id, collective_id, substrate access, event emitter | P0 | F-03 |
| FR-008 | Lens SHALL support domain_focus, type_weights, recency_curve, purpose_embedding, attention_budget | P0 | F-04 |
| FR-009 | Context assembly SHALL warp query embedding through lens before PulseDB search | P0 | F-04, F-09 |
| FR-010 | Context assembly SHALL apply post-search re-ranking with temporal decay and attractor strength | P0 | F-09 |
| FR-011 | Experience recording SHALL store experiences via SubstrateProvider with automatic embedding | P0 | F-08 |
| FR-012 | LlmProvider trait SHALL support chat() and chat_stream() with tool definitions | P0 | F-06 |
| FR-013 | OpenAI-compatible provider SHALL support configurable base_url for GLM, vLLM, Ollama, etc. | P0 | F-06 |
| FR-014 | HiveEvent enum SHALL cover agent lifecycle, LLM calls, tool execution, substrate operations | P0 | F-07 |
| FR-015 | Sequential workflow agent SHALL run sub-agents in order, each seeing previous experiences | P1 | F-10 |
| FR-016 | Parallel workflow agent SHALL run sub-agents concurrently sharing substrate in real-time | P1 | F-10 |
| FR-017 | Loop workflow agent SHALL repeat sub-agent until max_iterations or completion signal | P1 | F-10 |
| FR-018 | RelationshipDetector SHALL infer relations using embedding similarity + type-pair heuristics | P1 | F-12 |
| FR-019 | InsightSynthesizer SHALL generate insights when relation cluster density exceeds threshold | P1 | F-13 |
| FR-020 | ContextOptimizer SHALL compute decayed importance: `importance * e^(-lambda * elapsed) * reinforcement` | P1 | F-14 |
| FR-021 | ApprovalHandler trait SHALL support Approved, Denied, and Modified responses for tool execution | P2 | F-17 |

---

## 6. Out of Scope (MVP)

The following are explicitly deferred and SHALL NOT be implemented in Phase 1-2:

| Item | Rationale | Target Phase |
|------|-----------|--------------|
| Python bindings (PyO3) | Stabilize Rust API first | Phase 3 |
| TypeScript bindings (napi-rs) | Stabilize Rust API first | Phase 4 |
| REFRAG optimization | Requires decoder-level API access from LLM providers | Future |
| PostgresSubstrate | PulseDB embedded is sufficient for MVP; cloud deployment not yet needed | Future |
| WisdomAbstractor | Cross-collective sharing needs real-world usage data to design correctly | Phase 4 |
| Built-in agent types | SDK provides primitives; products define their own agent types | Never (by design) |
| Built-in tools | SDK provides the Tool trait; products implement domain-specific tools | Never (by design) |
| EmbeddingProvider trait | PulseDB Builtin embeddings (all-MiniLM-L6-v2, 384d) sufficient for MVP | Phase 4 |
| Proprietary observability platform | Use `tracing` crate ecosystem; no vendor lock-in | Never (by design) |
| GUI / dashboard | PulseHive is a library crate; UI is product-level concern | Never (by design) |

---

## 7. Constraints

### 7.1 Technical Constraints

- **Language:** Rust (stable toolchain, 2024 edition)
- **Async runtime:** Tokio (multi-threaded)
- **Storage dependency:** PulseDB v0.1.1 (`pulsehive-db` crate, AGPL-3.0)
- **Embedding model:** all-MiniLM-L6-v2 (384 dimensions) via PulseDB Builtin mode
- **Minimum supported platforms:** macOS (Apple Silicon), Linux (x86_64), Windows (x86_64)

### 7.2 Resource Constraints

- **Team:** Solo developer (Draco) with Claude Code AI-assisted development
- **Timeline:** 16 weeks across 4 phases
- **Budget:** Open source; no paid infrastructure for development

### 7.3 Design Constraints

- Maximum 5 core primitives (HiveMind, Agent, Tool, Lens, Experience)
- No abstraction without a demonstrated use case (anti-LangChain principle)
- Illegal states must be unrepresentable at compile time (Rust type system)
- All intelligence lives in PulseHive, never in PulseDB

---

## 8. Assumptions and Dependencies

### 8.1 Assumptions

1. PulseDB v0.1.1 is stable and its SubstrateProvider trait will not change during Phase 1
2. LLM providers (OpenAI, Anthropic, GLM) maintain backward-compatible APIs
3. PulseDB Builtin embeddings are sufficient quality for MVP use cases
4. Solo developer with AI assistance can deliver Phase 1 in 4 weeks

### 8.2 Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| `pulsehive-db` | 0.1.1 | Storage substrate (SubstrateProvider, Experience types) |
| `tokio` | 1.x | Async runtime |
| `serde` / `serde_json` | 1.x | Serialization |
| `tracing` | 0.1.x | Structured observability |
| `async-trait` | 0.1.x | Async trait support |
| `reqwest` | 0.12.x | HTTP client for LLM providers |
| `uuid` | 1.x | ID generation (v7, time-ordered) |

---

## 9. Risks

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| PulseDB API changes break SubstrateProvider integration | Low | High | Both codebases owned by same developer; pin to v0.1.1 |
| LLM provider API changes break pulsehive-openai | Medium | Medium | Abstraction layer isolates changes; provider crates are thin |
| Solo developer velocity insufficient for 4-week phases | Medium | High | AI-assisted development; strict scope per phase; cut P2 if needed |
| PulseDB performance degrades at scale (>10K experiences) | Low | Medium | Benchmarks at end of each phase; optimize PulseDB if needed |
| Trait design locks in wrong abstraction | Medium | High | Phase 1 is internal-only; stabilize traits only after Phase 2 validation |

---

## 10. Approval

This PRD authorizes development of PulseHive SDK Phase 1 (Foundation) beginning immediately, with subsequent phases gated on Phase 1 deliverable completion.

| Role | Name | Date |
|------|------|------|
| Project Lead / Developer | Draco | 2026-03-17 |

---

*This is a living document. Updated as requirements evolve through development phases.*
