# PulseHive Architecture

> **Version:** 0.4.0
> **Status:** Pre-implementation (architecture finalized)
> **Last Updated:** March 2026

---

## 1. Overview

PulseHive is a Rust SDK for building multi-agent AI systems where agents share consciousness through a persistent substrate (PulseDB) instead of passing messages. This document describes the system architecture using the C4 model approach, covering context, containers (crates), components, and key architectural decisions.

PulseHive is a **library crate**, not a deployed service. There is no REST API, no frontend, and no server process. Products embed PulseHive as a Rust dependency and interact with it through public traits, structs, and methods.

---

## 2. Level 1: System Context

At the highest level, PulseHive sits between three categories of external actors:

```
                        ┌─────────────────────────┐
                        │     PRODUCT CODE         │
                        │  (DevStudio, PV Agent,   │
                        │   Personal Assistant,    │
                        │   Research Engine, ...)   │
                        └───────────┬─────────────┘
                                    │  uses PulseHive SDK
                                    │  (Rust crate dependency)
                                    ▼
                        ┌─────────────────────────┐
                        │     PULSEHIVE SDK        │
                        │                          │
                        │  Orchestration           │
                        │  Intelligence            │
                        │  Perception (Lens)       │
                        │  Agentic Loop            │
                        └─────┬──────────┬────────┘
                              │          │
               uses           │          │  uses
          SubstrateProvider    │          │  LlmProvider
                              ▼          ▼
                ┌──────────────┐  ┌──────────────────┐
                │   PulseDB    │  │  LLM Providers   │
                │  (embedded   │  │  (Anthropic,     │
                │   file DB)   │  │   OpenAI, GLM,   │
                │              │  │   vLLM, Ollama)  │
                └──────────────┘  └──────────────────┘
```

**Product code** is any Rust application (or Python/TypeScript application via future bindings) that imports PulseHive as a dependency. Products define domain-specific agents, tools, and approval policies. PulseHive handles orchestration, intelligence, and substrate interaction.

**PulseDB** is the storage substrate. It is an embedded file-based database (like SQLite) providing experience CRUD, HNSW vector search over 384-dimensional embeddings, knowledge graph relations, derived insights, and a real-time Watch system. PulseHive accesses PulseDB exclusively through the `SubstrateProvider` trait.

**LLM providers** are external inference services. PulseHive communicates with them through the `LlmProvider` trait using standard text-in/text-out HTTP APIs. No direct embedding injection or KV-cache manipulation -- all LLM interaction is through chat completion endpoints.

---

## 3. Level 2: Containers (Crate Structure)

PulseHive is organized as a Cargo workspace with five crates. Each crate has a single responsibility and well-defined dependency direction.

```
┌───────────────────────────────────────────────────────────────────────┐
│                         pulsehive (meta-crate)                       │
│            Re-exports everything. Feature flags: anthropic, openai   │
│            Usage: cargo add pulsehive --features anthropic           │
└──────────────────────┬──────────────────────────┬────────────────────┘
                       │                          │
          ┌────────────▼────────────┐  ┌──────────▼──────────────┐
          │   pulsehive-runtime     │  │  pulsehive-anthropic    │
          │                         │  │  pulsehive-openai       │
          │  HiveMind, HiveMindBld  │  │                         │
          │  Agentic Loop Engine    │  │  LlmProvider impls      │
          │  Workflow Engine        │  │  for Claude, OpenAI,    │
          │  Intelligence Layer     │  │  GLM, vLLM, Ollama      │
          │  Event Bus              │  │                         │
          │  Watch Integration      │  │                         │
          └────────────┬────────────┘  └──────────┬──────────────┘
                       │                          │
                       │      both depend on      │
                       └──────────┬───────────────┘
                                  ▼
                    ┌──────────────────────────┐
                    │    pulsehive-core         │
                    │                          │
                    │  Agent trait/types        │
                    │  Tool trait               │
                    │  Lens struct              │
                    │  LlmProvider trait        │
                    │  HiveEvent enum           │
                    │  ApprovalHandler trait     │
                    │  EmbeddingProvider trait   │
                    │  PulseHiveError enum      │
                    │  Re-exports from PulseDB  │
                    └──────────────┬────────────┘
                                   │
                                   │ depends on
                                   ▼
                    ┌──────────────────────────┐
                    │   pulsehive-db (PulseDB)  │
                    │                          │
                    │  SubstrateProvider trait  │
                    │  Experience, Relation,    │
                    │  Insight, Activity types  │
                    │  HNSW vector index        │
                    │  Watch system             │
                    └──────────────────────────┘
```

### Crate Responsibilities

| Crate | Purpose | Key Dependencies |
|---|---|---|
| `pulsehive-core` | Trait definitions and shared types. Zero provider dependencies. Defines the public API surface that products and providers code against. | `pulsehive-db` (re-exports), `serde`, `async-trait` |
| `pulsehive-runtime` | Execution engine. Contains HiveMind orchestrator, the agentic loop, workflow agent execution, intelligence algorithms, event bus, and Watch integration. | `pulsehive-core`, `pulsehive-db`, `tokio`, `tracing` |
| `pulsehive-anthropic` | Claude LlmProvider implementation. Translates PulseHive's `LlmProvider` trait calls into Anthropic Messages API requests. | `pulsehive-core`, `anthropic-sdk` |
| `pulsehive-openai` | OpenAI-compatible LlmProvider implementation. Works with any provider exposing the OpenAI chat completions API: OpenAI, GLM, vLLM, LM Studio, Ollama, Together, Groq. | `pulsehive-core`, `reqwest` |
| `pulsehive` | Meta-crate. Re-exports `pulsehive-core` and `pulsehive-runtime`. Feature flags gate provider crates. | All of the above (conditionally) |

### Dependency Rules

1. **Dependency arrows point downward only.** Runtime depends on core. Provider crates depend on core. Nothing depends on the meta-crate.
2. **pulsehive-core has no runtime dependencies.** It defines traits and types only. This keeps compile times fast and allows provider crates to depend on core without pulling in tokio.
3. **Provider crates are independent.** `pulsehive-anthropic` and `pulsehive-openai` do not depend on each other. Products opt in via feature flags.
4. **PulseDB is an external crate** published as `pulsehive-db` on crates.io. PulseHive depends on it; it does not contain PulseDB source code.

---

## 4. Level 3: Components within pulsehive-runtime

The runtime crate contains six major components:

```
┌──────────────────────────────────────────────────────────────────┐
│                       pulsehive-runtime                          │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    HiveMind (Orchestrator)                │   │
│  │  Owns: substrate, llm_router, event_bus, intelligence     │   │
│  │  API: builder(), deploy(), record_experience(),           │   │
│  │       get_context()                                       │   │
│  └───────┬────────────┬──────────────┬───────────────────────┘   │
│          │            │              │                            │
│          ▼            ▼              ▼                            │
│  ┌───────────┐ ┌────────────┐ ┌──────────────────────────┐     │
│  │ Agentic   │ │ Workflow   │ │  Intelligence Layer      │     │
│  │ Loop      │ │ Engine     │ │                          │     │
│  │           │ │            │ │  RelationshipDetector    │     │
│  │ perceive  │ │ Sequential │ │  InsightSynthesizer      │     │
│  │ think     │ │ Parallel   │ │  ContextOptimizer        │     │
│  │ act       │ │ Loop       │ │                          │     │
│  │ record    │ │            │ │  AttractorDynamics       │     │
│  └─────┬─────┘ └─────┬──────┘ └────────────┬─────────────┘     │
│        │              │                     │                    │
│        └──────────────┼─────────────────────┘                    │
│                       │                                          │
│                       ▼                                          │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  EventBus                          WatchIntegration       │   │
│  │  Broadcasts HiveEvent to           Bridges PulseDB Watch  │   │
│  │  consumers via Stream.             events into runtime.   │   │
│  │  Built on tracing crate.           Enables mid-task       │   │
│  │                                    substrate refresh.     │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### HiveMind

The central orchestrator and primary entry point for product code. HiveMind owns the substrate connection, LLM router (maps provider names to `LlmProvider` instances), intelligence components, and the event bus. Products construct it via the builder pattern and then call `deploy()` to run agents.

### AgenticLoop

Executes the perceive-think-act-record cycle for each `LlmAgent`. This is a framework-provided loop that products do not override. The loop:

1. **Perceive**: Queries the substrate through the agent's Lens, applies temporal decay and re-ranking, and presents results as intrinsic knowledge.
2. **Think**: Sends the assembled context (system prompt + substrate context + task + conversation history) to the LLM via `LlmProvider::chat()` or `chat_stream()`.
3. **Act**: If the LLM returns a tool call, executes the tool and feeds the result back to step 2. If the LLM returns a final response, proceeds to step 4.
4. **Record**: Extracts experiences from the completed session and writes them to the substrate. All other agents perceive these immediately.

For long-running tasks, the loop re-perceives the substrate every N tool calls so the agent sees changes from other concurrent agents.

### WorkflowEngine

Executes `AgentKind::Sequential`, `AgentKind::Parallel`, and `AgentKind::Loop` agents. Workflow agents are deterministic orchestrators that compose sub-agents without LLM overhead.

- **Sequential**: Runs sub-agents one after another. Each sub-agent sees the experiences recorded by all previous sub-agents.
- **Parallel**: Spawns sub-agents concurrently via `tokio::spawn`. All sub-agents share the substrate in real time through the Watch system.
- **Loop**: Repeats a sub-agent until `max_iterations` is reached or the agent signals completion.

Workflow agents can be nested arbitrarily: a Sequential agent can contain Parallel stages, which themselves contain LLM agents.

### IntelligenceLayer

Three components that make PulseHive more than a simple agent orchestrator:

**RelationshipDetector** -- After each experience is recorded, searches for semantically similar experiences and infers relationships (Supports, Contradicts, Elaborates, Supersedes, Implies, RelatedTo). Uses a two-tier approach: pattern matching on ExperienceType pairs for high-confidence classifications, and LLM-based classification for ambiguous cases.

**InsightSynthesizer** -- When the relation graph in a cluster exceeds a density threshold, synthesizes a derived insight using LLM analysis. The insight has its own embedding and acts as a consolidated attractor in future searches.

**ContextOptimizer** -- Assembles the optimal context for a given agent and task within a token budget. Computes temporal decay (`importance * e^(-lambda * elapsed)`), factors in reinforcement (`applications_count`), and prioritizes insights over raw experiences. Presents context as intrinsic knowledge ("You understand that...") rather than retrieved documents.

### EventBus

Broadcasts `HiveEvent` variants to consumers. `HiveMind::deploy()` returns a `Stream<Item = HiveEvent>` that products consume for real-time UI updates, logging, monitoring, and audit trails. Built on the `tracing` crate -- every operation emits structured spans and events that any `tracing-subscriber` can capture.

### WatchIntegration

Bridges PulseDB's Watch system into the runtime. When any agent records an experience, PulseDB emits a `WatchEvent`. WatchIntegration delivers these events to all running agents subscribed to the same collective, enabling mid-task substrate refresh without polling.

---

## 5. Architectural Decisions

### ADR-1: Intelligence lives in PulseHive, not PulseDB

**Decision**: All intelligence algorithms (relationship detection, insight synthesis, context optimization, lens perception, field dynamics) live in PulseHive. PulseDB is a pure storage and retrieval layer.

**Rationale**:
- Intelligence algorithms need LLM access for classification and synthesis. Putting them in the database would create a circular dependency.
- Algorithms can be updated, A/B tested, and improved without database migrations.
- Inference failures are recoverable at the application layer. Database failures are not.
- Clean separation of concerns: PulseDB stores and retrieves. PulseHive thinks.

### ADR-2: SubstrateProvider as the boundary

**Decision**: PulseHive accesses all storage through the `SubstrateProvider` trait, which is defined in and owned by PulseDB.

**Rationale**:
- Decouples PulseHive from any specific storage backend.
- Enables future `PostgresSubstrate` for cloud deployments without changing PulseHive code.
- Makes testing straightforward -- use a mock `SubstrateProvider` in tests.
- Since both codebases are co-owned, the trait can evolve as needs emerge.

### ADR-3: Text-only LLM delivery (no REFRAG)

**Decision**: LLM interaction is text-in/text-out through standard chat completion APIs. No direct embedding injection or KV-cache manipulation.

**Rationale**:
- No LLM provider currently exposes decoder-level APIs for embedding injection.
- Text delivery works with every provider (Anthropic, OpenAI, GLM, vLLM, Ollama).
- REFRAG-style optimization is documented as a future opportunity for when self-hosted models or provider APIs make it practical.
- "You understand that..." framing achieves 80% of the benefit without custom inference infrastructure.

### ADR-4: Observability via tracing crate

**Decision**: All observability is built on the Rust `tracing` crate ecosystem. No proprietary observability platform.

**Rationale**:
- Products choose their own subscriber: stdout logging, OpenTelemetry, Datadog, Jaeger, or custom.
- No vendor lock-in (unlike LangSmith for LangChain).
- Structured spans and events integrate naturally with Rust's async runtime.
- `HiveEvent` enum provides a high-level event stream for product-specific consumption alongside low-level tracing spans for debugging.

### ADR-5: Library deployment, not service

**Decision**: PulseHive is a library crate embedded in the product binary. There is no PulseHive server, daemon, or managed service.

**Rationale**:
- PulseDB opens a local file -- zero infrastructure for simple deployments.
- Products control their own deployment topology (CLI, desktop app, server, container, serverless).
- No network hop between PulseHive and PulseDB -- sub-millisecond storage access.
- Products that need cloud-scale can use a future `PostgresSubstrate` without changing PulseHive's embedding model.

---

## 6. Data Flow Diagrams

### Agent Perception Flow

How an agent perceives the shared substrate through its Lens:

```
┌──────────┐     ┌──────────────┐     ┌─────────────┐     ┌────────────┐
│  Agent's │     │    Lens      │     │  PulseDB    │     │  Context   │
│  Task    │────►│  Transform   │────►│  Search     │────►│  Optimizer │
│          │     │              │     │             │     │            │
│ "Analyze │     │ 1. Compute   │     │ search_     │     │ 1. Decay   │
│  batch   │     │    purpose   │     │ similar()   │     │    calc    │
│  ABC"    │     │    embedding │     │             │     │ 2. Re-rank │
│          │     │ 2. Warp via  │     │ Returns     │     │ 3. Budget  │
│          │     │    domain    │     │ (Exp, sim)  │     │    pack    │
│          │     │    focus     │     │ tuples      │     │ 4. Format  │
│          │     │ 3. Apply     │     │             │     │    as      │
│          │     │    type      │     │ get_context │     │    "You    │
│          │     │    weights   │     │ _candidates │     │    know"   │
└──────────┘     └──────────────┘     └─────────────┘     └─────┬──────┘
                                                                 │
                                                                 ▼
                                                          ┌────────────┐
                                                          │ AgentCtx   │
                                                          │            │
                                                          │ Intrinsic  │
                                                          │ knowledge  │
                                                          │ + activity │
                                                          │ awareness  │
                                                          └────────────┘
```

### Experience Recording Flow

How an agent's work becomes shared knowledge:

```
┌──────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  Agent   │     │  Experience  │     │  PulseDB     │     │  Watch       │
│  Session │────►│  Extractor   │────►│  store_      │────►│  System      │
│  Done    │     │              │     │  experience  │     │              │
│          │     │ Extracts:    │     │              │     │ Emits:       │
│          │     │ - Patterns   │     │ Stores with  │     │ WatchEvent:: │
│          │     │ - Errors     │     │ embedding in │     │ Created      │
│          │     │ - Decisions  │     │ HNSW index   │     │              │
└──────────┘     └──────────────┘     └──────┬───────┘     └──────┬───────┘
                                             │                     │
                                             ▼                     ▼
                                    ┌────────────────┐    ┌────────────────┐
                                    │ Relationship   │    │ All subscribed │
                                    │ Detector       │    │ agents see new │
                                    │                │    │ experience on  │
                                    │ Infers:        │    │ next perceive  │
                                    │ Supports,      │    └────────────────┘
                                    │ Contradicts,   │
                                    │ Elaborates...  │
                                    └───────┬────────┘
                                            │
                                            ▼
                                    ┌────────────────┐
                                    │ Insight        │
                                    │ Synthesizer    │
                                    │                │
                                    │ If cluster     │
                                    │ density >      │
                                    │ threshold:     │
                                    │ synthesize     │
                                    │ derived        │
                                    │ insight        │
                                    └────────────────┘
```

### Multi-Agent Coordination Flow

How parallel agents share consciousness without message passing:

```
          ┌──────────────────────────────────────────────────┐
          │              Shared Substrate (PulseDB)           │
          │                                                    │
          │  Experiences ──── Relations ──── Insights          │
          │       ▲    ▲         ▲              ▲              │
          │       │    │         │              │              │
          └───────┼────┼─────────┼──────────────┼──────────────┘
                  │    │         │              │
        write     │    │ read    │ auto-        │ auto-
                  │    │         │ inferred     │ synthesized
                  │    │         │              │
          ┌───────┘    └──────┐  │              │
          │                   │  │              │
     ┌────┴─────┐       ┌────┴──┴──┐     ┌─────┴──────┐
     │ Agent A  │       │ Agent B  │     │ Agent C    │
     │ (Safety) │       │ (Finance)│     │ (Reporter) │
     │          │       │          │     │            │
     │ Lens:    │       │ Lens:    │     │ Lens:      │
     │ safety,  │       │ finance, │     │ reporting, │
     │ clinical │       │ audit    │     │ summary    │
     └──────────┘       └──────────┘     └────────────┘
          │                   │                 │
          │  all running concurrently via       │
          │  AgentKind::Parallel                │
          │                                     │
          └── Agent A writes "batch anomaly" ───┘
              Agent B perceives it instantly
              (Watch system, no polling)
              Agent C sees both when it perceives
```

The key property is that no agent sends a message to another agent. Agent A writes an experience. The Watch system notifies all subscribers. Agent B's next perception cycle includes the new experience. The Intelligence Layer may also infer a relationship between Agent A's finding and an existing experience, or synthesize an insight if a cluster threshold is reached. All of this happens through the substrate, not through inter-agent communication.

---

## 7. Cross-Cutting Concerns

### Error Handling

PulseHive defines `PulseHiveError` for SDK-level errors and re-exports `PulseDBError` for substrate errors. All public methods return `Result<T, PulseHiveError>`. Errors are structured, not stringly-typed, enabling products to match on variants and implement recovery logic.

### Concurrency Model

PulseHive is built on Tokio. `HiveMind::deploy()` spawns agents as Tokio tasks. Parallel workflow agents use `tokio::JoinSet` for concurrent execution. The substrate (`Box<dyn SubstrateProvider>`) is `Send + Sync` and safe to share across tasks via `Arc`. The Watch system uses crossbeam channels for low-overhead event delivery.

### Testing Strategy

- **Unit tests**: Mock `SubstrateProvider` and `LlmProvider` implementations for deterministic testing of intelligence algorithms, agentic loop logic, and workflow execution.
- **Integration tests**: Real PulseDB instance (in-memory or temp file) with real substrate operations.
- **Provider tests**: Each LLM provider crate has integration tests against live APIs (gated behind feature flags and environment variables).

### Security Model

PulseHive inherits PulseDB's collective isolation model. Experiences in one collective are invisible to agents in another. Products control access by managing which `CollectiveId` values are passed to `HiveMind`. There is no built-in authentication or authorization -- products implement access control at their own layer.

---

*This document describes the architecture as of SPEC v0.4.0. It will be updated as implementation proceeds and decisions are validated.*
