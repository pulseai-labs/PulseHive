# PulseHive SDK — System Requirements Specification

> **Document ID:** SRS-PH-001
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Approved for Phase 1 Development
> **Reference:** PRD-PH-001, SPEC v0.4.0
> **Standard:** IEEE 830-1998 (adapted)

---

## 1. Introduction

### 1.1 Purpose

This System Requirements Specification defines the functional and non-functional requirements for PulseHive SDK v1.0. It serves as the authoritative reference for implementation, testing, and acceptance across all four development phases. Every requirement is traceable to a feature in the PRD (PRD-PH-001).

### 1.2 Scope

PulseHive is a Rust SDK (library crate) for building multi-agent AI systems with shared consciousness through PulseDB. The SDK provides five core primitives (HiveMind, Agent, Tool, Lens, Experience), an intelligence layer (RelationshipDetector, InsightSynthesizer, ContextOptimizer), LLM provider abstractions, and observability infrastructure.

PulseHive is NOT a product. It is a framework on which products (DevStudio, pharmacovigilance systems, research engines) are built.

### 1.3 Definitions and Abbreviations

| Term | Definition |
|------|-----------|
| **Substrate** | The PulseDB storage layer that holds all shared consciousness data |
| **Collective** | An isolated namespace in PulseDB (maps to a project or tenant) |
| **Experience** | A unit of knowledge stored in the substrate (PulseDB type) |
| **Attractor** | An experience viewed as a point in embedding space with gravitational dynamics |
| **Lens** | A perception filter that shapes how an agent sees the substrate |
| **Agentic loop** | The Perceive-Think-Act-Record cycle executed by each LLM agent |
| **Watch system** | PulseDB's real-time event notification for substrate changes |
| **HNSW** | Hierarchical Navigable Small World — the vector index algorithm in PulseDB |

---

## 2. Functional Requirements

### 2.1 HiveMind Orchestrator

**FR-001: HiveMind Builder Pattern**
- The system SHALL provide a `HiveMind::builder()` method returning a `HiveMindBuilder`
- The builder SHALL accept substrate configuration (path string or `Box<dyn SubstrateProvider>`)
- The builder SHALL accept one or more LLM providers keyed by name (e.g., "anthropic", "openai")
- The builder SHALL reject invalid configurations at compile time where possible and at runtime via `Result<HiveMind>` for dynamic checks
- **Traces to:** PRD F-01, FR-001

**FR-002: Substrate Integration**
- HiveMind SHALL hold a `Box<dyn SubstrateProvider>` as its storage backend
- The convenience method `substrate_path(path)` SHALL create a `PulseDBSubstrate` from the given file path
- The system SHALL re-export all PulseDB types needed by consumers: `Experience`, `NewExperience`, `ExperienceType`, `ExperienceId`, `CollectiveId`, `SearchFilter`, `ContextRequest`, `ContextCandidates`
- **Traces to:** PRD F-05, FR-002

**FR-003: Agent Deployment**
- `HiveMind::deploy()` SHALL accept `Vec<AgentDefinition>` and `Vec<Task>`
- It SHALL return `Result<impl Stream<Item = HiveEvent>>`
- Each agent SHALL execute asynchronously on the Tokio runtime
- Workflow agents SHALL be expanded recursively (Sequential runs children in order; Parallel spawns children concurrently; Loop repeats child)
- **Traces to:** PRD F-01, F-07, FR-003

### 2.2 LLM Agent and Agentic Loop

**FR-004: Agentic Loop Execution**
- Each LlmAgent SHALL execute a four-phase loop:
  1. **Perceive:** Query substrate through the agent's lens to build context
  2. **Think:** Send system prompt + substrate context + task + conversation history to LLM
  3. **Act:** If LLM returns tool call(s), execute tool(s) and add results to history, then return to Think. If LLM returns final response, proceed to Record
  4. **Record:** Extract experiences from the session and store in substrate
- The loop SHALL terminate when the LLM produces a final response (no tool calls) or a configurable max_iterations limit is reached
- **Traces to:** PRD F-02, FR-004

**FR-005: Agent Configuration**
- `AgentDefinition` SHALL contain `name: String` and `kind: AgentKind`
- `AgentKind::Llm(LlmAgentConfig)` SHALL contain: `system_prompt`, `tools: Vec<Box<dyn Tool>>`, `lens: Lens`, `llm_config: LlmConfig`, `experience_extractor: Option<Box<dyn ExperienceExtractor>>`
- `LlmConfig` SHALL contain: `model: String`, `temperature: f32`, `max_tokens: u32`
- The system SHALL provide a default `ExperienceExtractor` that extracts learnings from agent conversation history
- **Traces to:** PRD F-02, F-03, F-04, FR-005

### 2.3 Tool Trait

**FR-006: Tool Interface**
- The `Tool` trait SHALL define: `fn name() -> &str`, `fn description() -> &str`, `fn parameters() -> serde_json::Value` (JSON Schema), `async fn execute(params, context) -> Result<ToolResult>`
- `Tool` SHALL be `Send + Sync` to support concurrent execution
- `ToolResult` SHALL support text content, structured JSON, and error responses
- **Traces to:** PRD F-03, FR-006

**FR-007: Tool Context**
- `ToolContext` SHALL provide: `agent_id: AgentId`, `collective_id: CollectiveId`, `substrate: Arc<dyn SubstrateProvider>`, `event_emitter: EventEmitter`
- Tools SHALL be able to read from and write to the substrate during execution
- Tools SHALL be able to emit custom events via the event emitter
- **Traces to:** PRD F-03, FR-007

### 2.4 Lens Perception

**FR-008: Lens Configuration**
- `Lens` SHALL contain: `domain_focus: Vec<String>`, `type_weights: HashMap<ExperienceTypeTag, f32>`, `recency_curve: RecencyCurve`, `purpose_embedding: Vec<f32>`, `attention_budget: usize`
- `RecencyCurve` SHALL support `Exponential { half_life_hours: f32 }` and `Uniform` variants
- `Lens::new(domains)` SHALL provide a convenience constructor with sensible defaults
- **Traces to:** PRD F-04, FR-008

**FR-009: Pre-Search Embedding Warping**
- Before calling `search_similar()`, the system SHALL transform the query embedding through the lens
- Transformation SHALL amplify dimensions aligned with `purpose_embedding` and `domain_focus`
- The warped embedding SHALL be passed to PulseDB's `search_similar()` for HNSW search
- **Traces to:** PRD F-04, F-09, FR-009

**FR-010: Post-Search Re-Ranking**
- After PulseDB returns search results, the system SHALL re-rank by:
  1. Domain relevance: multiply similarity score by domain match weight
  2. Temporal decay: `importance * e^(-lambda * elapsed_hours) * (1 + applications_count * reinforcement_boost)`
  3. Type weight: multiply by the lens's type_weight for the experience type
- Results SHALL be sorted by composite score descending and truncated to `attention_budget`
- The final context SHALL be presented as intrinsic knowledge ("You understand that..."), not as retrieved documents
- **Traces to:** PRD F-09, FR-010

### 2.5 Experience Management

**FR-011: Experience Recording**
- `HiveMind::record_experience()` SHALL call `substrate.store_experience()` and then trigger relationship inference
- If PulseDB is in Builtin embedding mode, the embedding SHALL be computed automatically by PulseDB
- The operation SHALL emit `HiveEvent::ExperienceRecorded`
- **Traces to:** PRD F-08, FR-011

### 2.6 LLM Provider Abstraction

**FR-012: LlmProvider Trait**
- The `LlmProvider` trait SHALL define: `async fn chat(messages, tools, config) -> Result<LlmResponse>` and `async fn chat_stream(messages, tools, config) -> Result<impl Stream<Item = Result<LlmChunk>>>`
- `LlmResponse` SHALL contain: response text, optional tool calls (name + JSON params), usage statistics (prompt_tokens, completion_tokens)
- `LlmChunk` SHALL support text deltas and tool call deltas for streaming
- **Traces to:** PRD F-06, FR-012

**FR-013: OpenAI-Compatible Provider**
- `pulsehive-openai` SHALL implement `LlmProvider` for any OpenAI-compatible API
- Configuration SHALL accept: `api_key`, `base_url`, `model` (defaults to OpenAI endpoint)
- The provider SHALL handle the OpenAI chat completions API format including tool use (function calling)
- Tested targets: OpenAI GPT-4, GLM-5 (BigModel), vLLM, LM Studio, Ollama
- **Traces to:** PRD F-06, FR-013

### 2.7 Event System

**FR-014: HiveEvent Enum**
- `HiveEvent` SHALL contain variants for:
  - Agent lifecycle: `AgentStarted`, `AgentCompleted`
  - LLM interactions: `LlmCallStarted`, `LlmCallCompleted`, `LlmTokenStreamed`
  - Tool execution: `ToolCallStarted`, `ToolCallCompleted`, `ToolApprovalRequested`
  - Substrate operations: `ExperienceRecorded`, `RelationshipInferred`, `InsightGenerated`
  - Perception: `SubstratePerceived`
- Each variant SHALL include `agent_id` where applicable and a timestamp
- Events SHALL be emittable via `tracing` crate spans/events for subscriber integration
- **Traces to:** PRD F-07, FR-014

### 2.8 Workflow Agents

**FR-015: Sequential Workflow**
- `AgentKind::Sequential(Vec<AgentDefinition>)` SHALL execute child agents in order
- Each child SHALL start only after the previous child completes
- Each child agent SHALL perceive experiences written by all previous agents via the substrate
- **Traces to:** PRD F-10, FR-015

**FR-016: Parallel Workflow**
- `AgentKind::Parallel(Vec<AgentDefinition>)` SHALL spawn all child agents concurrently on the Tokio runtime
- All children SHALL share the same substrate and collective
- Children SHALL perceive each other's experiences in real-time via the Watch system
- The parallel workflow SHALL complete when all children complete
- **Traces to:** PRD F-10, FR-016

**FR-017: Loop Workflow**
- `AgentKind::Loop { agent, max_iterations }` SHALL repeat the child agent up to `max_iterations` times
- The loop SHALL terminate early if the child agent signals completion (via a designated output or tool call)
- Each iteration SHALL perceive cumulative experiences from all prior iterations
- **Traces to:** PRD F-10, FR-017

### 2.9 Intelligence Layer

**FR-018: RelationshipDetector**
- SHALL find semantically similar experiences (top 20) when a new experience is recorded
- For pairs with similarity > `auto_threshold` (default 0.85), SHALL automatically create a relation
- Relation type SHALL be inferred from ExperienceType pair heuristics:
  - Difficulty + Solution -> Supports
  - ErrorPattern + ErrorPattern (similar signature) -> Supersedes
  - ArchitecturalDecision + TechInsight -> Implies
  - Same domain, opposing content -> Contradicts
- For pairs in the `suggest_threshold` (default 0.65) to `auto_threshold` range, MAY use LLM classification if `use_llm_classification` is enabled
- SHALL emit `HiveEvent::RelationshipInferred` for each created relation
- **Traces to:** PRD F-12, FR-018

**FR-019: InsightSynthesizer**
- SHALL monitor relation cluster density and trigger synthesis when cluster size exceeds `relation_density_threshold` (default 5)
- Synthesis SHALL use the LLM to generate a `NewDerivedInsight` summarizing the cluster
- The insight SHALL have its own embedding, enabling it to appear in future searches as a consolidated attractor
- Synthesis SHALL be debounced (default 60 seconds) to avoid redundant LLM calls
- SHALL emit `HiveEvent::InsightGenerated` for each created insight
- **Traces to:** PRD F-13, FR-019

**FR-020: ContextOptimizer**
- SHALL compute decayed importance: `importance * 0.5^(elapsed_hours / half_life) * (1 + applications_count * reinforcement_boost)`
- Default `decay_half_life_hours`: 72.0; default `reinforcement_boost`: 0.1
- Context assembly SHALL prioritize: insights > high-importance experiences > recent experiences
- SHALL pack context within a configurable `ContextBudget` (token limit)
- SHALL include activity awareness: "You're aware that agent X is working on Y"
- **Traces to:** PRD F-14, FR-020

### 2.10 Human-in-the-Loop

**FR-021: Approval Handler**
- Tools SHALL declare `requires_approval() -> bool`
- When the agentic loop encounters a tool requiring approval, it SHALL call the registered `ApprovalHandler::request_approval(action)`
- `ApprovalResult` SHALL support: `Approved`, `Denied { reason }`, `Modified { new_params }`
- On `Denied`, the system SHALL inform the LLM and allow it to choose an alternative action
- On `Modified`, the system SHALL execute the tool with the modified parameters
- SHALL emit `HiveEvent::ToolApprovalRequested` when approval is needed
- **Traces to:** PRD F-17, FR-021

---

## 3. Non-Functional Requirements

### 3.1 Performance Requirements

| ID | Requirement | Target | Measurement |
|----|-------------|--------|-------------|
| NFR-001 | Substrate search latency (1K experiences, k=20) | < 1ms (p99) | Benchmark: `search_similar()` end-to-end |
| NFR-002 | Context assembly latency (1K experiences) | < 10ms (p99) | Benchmark: lens warp + search + re-rank + format |
| NFR-003 | Experience recording (no LLM classification) | < 15ms (p99) | Benchmark: store + heuristic relationship inference |
| NFR-004 | Agent deployment overhead | < 5ms | Time from `deploy()` to first `AgentStarted` event |
| NFR-005 | Event stream latency | < 1ms | Time from event emission to consumer receipt |
| NFR-006 | Memory usage per agent | < 50 MB baseline | Excludes LLM response buffers |
| NFR-007 | Concurrent agents | >= 10 agents per HiveMind | On 8-core machine, no degradation |

### 3.2 Reliability Requirements

| ID | Requirement |
|----|-------------|
| NFR-008 | Agent failure SHALL NOT crash HiveMind or other agents |
| NFR-009 | LLM provider timeout SHALL result in a retriable error, not a panic |
| NFR-010 | Substrate write failure SHALL be reported via HiveEvent and Result, with partial recording attempted |
| NFR-011 | The agentic loop SHALL enforce max_iterations to prevent infinite tool-call cycles |
| NFR-012 | All public API methods SHALL return `Result<T>` with typed errors (no panics in library code) |

### 3.3 Security Requirements

| ID | Requirement |
|----|-------------|
| NFR-013 | API keys SHALL NOT be logged, stored in substrate, or included in HiveEvents |
| NFR-014 | Collective isolation SHALL prevent cross-collective data access at the substrate level |
| NFR-015 | Tool execution SHALL be sandboxed to the ToolContext provided (no ambient substrate access) |

### 3.4 Compatibility Requirements

| ID | Requirement |
|----|-------------|
| NFR-016 | SHALL compile on stable Rust 2024 edition (no nightly features required) |
| NFR-017 | SHALL support macOS (Apple Silicon), Linux (x86_64), and Windows (x86_64) |
| NFR-018 | SHALL be compatible with Tokio 1.x multi-threaded runtime |
| NFR-019 | SHALL not introduce dependency conflicts with common Rust crate ecosystem |

### 3.5 Maintainability Requirements

| ID | Requirement |
|----|-------------|
| NFR-020 | All public types and traits SHALL have rustdoc documentation |
| NFR-021 | Integration tests SHALL cover all FR requirements |
| NFR-022 | Each crate SHALL have independent unit tests runnable via `cargo test` |
| NFR-023 | CI pipeline SHALL enforce `cargo clippy` with no warnings and `cargo fmt` compliance |

---

## 4. Use Cases

### 4.1 UC-001: Deploy Single LLM Agent

**Actors:** Product developer (SDK consumer)
**Preconditions:** PulseDB substrate file exists or path is provided for creation
**Trigger:** Developer calls `hive.deploy(vec![agent], vec![task])`

**Main flow:**
1. Developer creates `HiveMind` via builder with substrate path and LLM provider
2. Developer defines `AgentDefinition` with `AgentKind::Llm(config)` including system prompt, tools, lens, and LLM config
3. Developer creates a `Task` with description
4. Developer calls `hive.deploy(vec![agent_def], vec![task])` and receives event stream
5. System creates agent runtime, assigns `AgentId`
6. System emits `HiveEvent::AgentStarted`
7. Agentic loop executes: Perceive (substrate query through lens) -> Think (LLM call) -> Act (tool calls or final response) -> Record (extract and store experiences)
8. System emits `HiveEvent::AgentCompleted` with outcome
9. Developer consumes events from stream

**Postconditions:** Experiences from the session are stored in substrate. Event stream is exhausted.

**Alternative flows:**
- 7a. LLM returns tool call: execute tool, emit `ToolCallStarted`/`ToolCallCompleted`, add result to history, return to Think
- 7b. LLM provider error: emit error event, retry per configuration, or terminate agent with error outcome
- 7c. Max iterations reached: terminate agent with `AgentOutcome::MaxIterations`

### 4.2 UC-002: Deploy Multi-Agent Swarm

**Actors:** Product developer
**Preconditions:** HiveMind configured with substrate and LLM providers
**Trigger:** Developer calls `hive.deploy()` with a Sequential workflow containing Parallel sub-agents

**Main flow:**
1. Developer defines a Sequential workflow with Parallel children (e.g., 3 analysts in parallel, then a synthesizer)
2. Developer calls `hive.deploy(vec![workflow], vec![task])`
3. System expands workflow: starts Phase 1 (Parallel — 3 analysts spawned concurrently)
4. Each analyst perceives substrate through its own lens, reasons with its LLM, uses its tools
5. Analysts write experiences to shared substrate; all analysts perceive each other's experiences via Watch system
6. When all Phase 1 agents complete, system starts Phase 2 (synthesizer)
7. Synthesizer perceives all experiences from Phase 1 through its lens
8. Synthesizer produces final output, records experiences
9. System emits `AgentCompleted` for each agent and for the workflow

**Postconditions:** Substrate contains experiences from all agents. Relationships and insights may have been generated.

### 4.3 UC-003: Record Experience with Relationship Inference

**Actors:** Agentic loop (internal), or product developer (direct API call)
**Preconditions:** Substrate has existing experiences
**Trigger:** `hive.record_experience(new_experience)` is called

**Main flow:**
1. System calls `substrate.store_experience(experience)` — PulseDB stores with embedding
2. System runs `RelationshipDetector.infer_relations(experience, substrate)`
3. Detector searches for top 20 similar experiences
4. For each pair above `auto_threshold`, detector creates a typed relation
5. System stores each relation via `substrate.store_relation()`
6. System checks if `InsightSynthesizer.should_synthesize()` threshold is met
7. If yes, synthesizer generates insight(s) via LLM and stores via `substrate.store_insight()`
8. System emits `ExperienceRecorded`, `RelationshipInferred` (per relation), `InsightGenerated` (if applicable)

**Postconditions:** Experience stored. Relations inferred and stored. Insight generated if cluster density threshold met.

### 4.4 UC-004: Perceive Substrate Through Lens

**Actors:** Agentic loop (internal)
**Preconditions:** Agent has a configured Lens; substrate has experiences
**Trigger:** Perceive phase of agentic loop

**Main flow:**
1. System computes query embedding from the current task description
2. System warps query embedding through lens (amplify purpose-aligned dimensions)
3. System constructs `ContextRequest` with warped embedding, `SearchFilter` from lens domain_focus, and budget from attention_budget
4. System calls `substrate.get_context_candidates(request)`
5. PulseDB returns `ContextCandidates` with similar experiences, recent experiences, insights, relations, active agents
6. System applies post-search re-ranking: domain relevance weighting, temporal decay, type weighting
7. System packs results into `AgentContext` within token budget, prioritizing insights > high-importance > recent
8. System formats context as intrinsic knowledge for LLM consumption
9. System emits `HiveEvent::SubstratePerceived` with counts

**Postconditions:** Agent has a context representation ready for the Think phase.

---

## 5. Data Requirements

All persistent data types are defined by PulseDB and re-exported by PulseHive. PulseHive adds runtime-only types.

### 5.1 Persistent Types (PulseDB-owned)

| Type | Key Fields | Storage |
|------|-----------|---------|
| `Experience` | id, collective_id, content, experience_type, embedding (384d), importance, confidence, domain, source_agent, applications_count, created_at | PulseDB B-tree + HNSW index |
| `ExperienceRelation` | source_id, target_id, relation_type, strength, metadata | PulseDB relation store |
| `DerivedInsight` | id, collective_id, content, embedding, source_experience_ids, insight_type | PulseDB insight store |
| `Activity` | agent_id, collective_id, status, last_seen | PulseDB activity tracking |

### 5.2 Runtime Types (PulseHive-owned)

| Type | Purpose | Lifetime |
|------|---------|----------|
| `HiveMind` | Orchestrator holding substrate + intelligence + LLM router | Application lifetime |
| `AgentDefinition` | Blueprint for agent creation | Consumed at deploy time |
| `Lens` | Perception filter configuration | Per-agent, immutable during execution |
| `AgentContext` | Assembled context for a single Think phase | Per-perception, ephemeral |
| `HiveEvent` | Observable event from any subsystem | Emitted and consumed via stream |
| `ToolContext` | Execution environment for a tool call | Per-tool-execution, ephemeral |
| `ContextBudget` | Token limit for context assembly | Per-perception, configurable |
| `AttractorDynamics` | Computed strength/radius/warp for an experience | Computed at query time, ephemeral |

### 5.3 ID Types

All IDs use UUID v7 (time-ordered) via PulseDB:

| Type | Format | Example |
|------|--------|---------|
| `CollectiveId` | UUID v7 | `018e4a3f-...` |
| `ExperienceId` | UUID v7 | `018e4a40-...` |
| `InsightId` | UUID v7 | `018e4a41-...` |
| `RelationId` | UUID v7 | `018e4a42-...` |
| `AgentId` | String | `"safety-analyst-01"` |
| `TaskId` | String | `"analyze-batch-abc"` |

---

## 6. Interface Requirements

### 6.1 SubstrateProvider Trait (PulseDB-defined, PulseHive-consumed)

```rust
#[async_trait]
pub trait SubstrateProvider: Send + Sync {
    async fn store_experience(&self, exp: NewExperience) -> Result<ExperienceId>;
    async fn get_experience(&self, id: ExperienceId) -> Result<Option<Experience>>;
    async fn search_similar(&self, collective: CollectiveId, embedding: &[f32], k: usize)
        -> Result<Vec<(Experience, f32)>>;
    async fn get_recent(&self, collective: CollectiveId, limit: usize) -> Result<Vec<Experience>>;
    async fn store_relation(&self, rel: NewExperienceRelation) -> Result<RelationId>;
    async fn get_related(&self, exp_id: ExperienceId)
        -> Result<Vec<(Experience, ExperienceRelation)>>;
    async fn store_insight(&self, insight: NewDerivedInsight) -> Result<InsightId>;
    async fn get_insights(&self, collective: CollectiveId, embedding: &[f32], k: usize)
        -> Result<Vec<(DerivedInsight, f32)>>;
    async fn get_activities(&self, collective: CollectiveId) -> Result<Vec<Activity>>;
    async fn get_context_candidates(&self, request: ContextRequest) -> Result<ContextCandidates>;
    async fn watch(&self, collective: CollectiveId)
        -> Result<Pin<Box<dyn Stream<Item = WatchEvent> + Send>>>;
}
```

This trait is the sole interface between PulseHive and storage. All substrate access goes through it.

### 6.2 LlmProvider Trait (PulseHive-defined)

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolDefinition>, config: &LlmConfig)
        -> Result<LlmResponse>;
    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolDefinition>, config: &LlmConfig)
        -> Result<impl Stream<Item = Result<LlmChunk>>>;
}
```

Implementations: `pulsehive-openai` (Phase 1), `pulsehive-anthropic` (Phase 2).

### 6.3 Tool Trait (PulseHive-defined, product-implemented)

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    async fn execute(&self, params: serde_json::Value, context: &ToolContext) -> Result<ToolResult>;
    fn requires_approval(&self) -> bool { false }
}
```

Products implement this trait for domain-specific capabilities. PulseHive provides no built-in tools.

### 6.4 ApprovalHandler Trait (PulseHive-defined, product-implemented)

```rust
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn request_approval(&self, action: &PendingAction) -> Result<ApprovalResult>;
}
```

Products implement this for their approval UX (CLI prompt, Slack notification, webhook, auto-approve).

---

## 7. Traceability Matrix

| SRS Requirement | PRD Feature | PRD FR | Phase |
|----------------|-------------|--------|-------|
| FR-001 | F-01 | FR-001 | 1 |
| FR-002 | F-05 | FR-002 | 1 |
| FR-003 | F-01, F-07 | FR-003 | 1 |
| FR-004 | F-02 | FR-004 | 1 |
| FR-005 | F-02, F-03, F-04 | FR-005 | 1 |
| FR-006 | F-03 | FR-006 | 1 |
| FR-007 | F-03 | FR-007 | 1 |
| FR-008 | F-04 | FR-008 | 1 |
| FR-009 | F-04, F-09 | FR-009 | 1 |
| FR-010 | F-09 | FR-010 | 1 |
| FR-011 | F-08 | FR-011 | 1 |
| FR-012 | F-06 | FR-012 | 1 |
| FR-013 | F-06 | FR-013 | 1 |
| FR-014 | F-07 | FR-014 | 1 |
| FR-015 | F-10 | FR-015 | 2 |
| FR-016 | F-10 | FR-016 | 2 |
| FR-017 | F-10 | FR-017 | 2 |
| FR-018 | F-12 | FR-018 | 2 |
| FR-019 | F-13 | FR-019 | 2 |
| FR-020 | F-14 | FR-020 | 2 |
| FR-021 | F-17 | FR-021 | 3 |
| NFR-001 through NFR-007 | Performance | — | 1-2 |
| NFR-008 through NFR-012 | Reliability | — | 1-2 |
| NFR-013 through NFR-015 | Security | — | 1 |
| NFR-016 through NFR-019 | Compatibility | — | 1 |
| NFR-020 through NFR-023 | Maintainability | — | 1-4 |

---

## 8. Acceptance Criteria Summary

### Phase 1 Acceptance

1. `HiveMind::builder().substrate_path("test.db").llm_provider("openai", provider).build()` succeeds
2. Single LlmAgent completes a task using a custom tool and records at least one experience
3. Event stream delivers `AgentStarted`, `LlmCallStarted`, `ToolCallStarted`, `ExperienceRecorded`, `AgentCompleted`
4. Lens filters substrate query by domain focus
5. Context assembly returns experiences re-ranked by lens weights and temporal decay
6. `pulsehive-openai` successfully communicates with OpenAI API and at least one OpenAI-compatible endpoint
7. All unit and integration tests pass on macOS and Linux

### Phase 2 Acceptance

1. Sequential workflow runs 3 agents in order; later agents perceive earlier agents' experiences
2. Parallel workflow runs 3 agents concurrently; each perceives others' experiences via Watch
3. Loop workflow repeats agent and terminates on completion signal or max iterations
4. RelationshipDetector creates at least one Supports and one Contradicts relation in test scenario
5. InsightSynthesizer generates an insight from a cluster of 5+ related experiences
6. ContextOptimizer correctly decays a 72-hour-old experience to ~50% importance
7. `pulsehive-anthropic` successfully calls Claude API with tool use

---

*This is a living document. Updated as requirements are refined through implementation.*
