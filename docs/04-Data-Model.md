# PulseHive Data Model

> **Version:** 0.4.0
> **Status:** Pre-implementation (model finalized)
> **Last Updated:** March 2026

---

## 1. Overview

PulseHive's data model spans two layers: **PulseDB entities** (persistent storage) and **PulseHive runtime entities** (in-memory during execution). PulseDB owns all persistent types. PulseHive re-exports them and adds runtime-only types for agent orchestration, LLM interaction, and event streaming.

This document covers both layers, their relationships, the collective isolation model, and the embedding space.

---

## 2. Entity Relationship Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          PULSEDB LAYER (Persistent)                      │
│                                                                          │
│                         ┌──────────────┐                                │
│                         │  Collective   │                                │
│                         │──────────────│                                │
│                         │ id: UUID v7   │                                │
│                         └──────┬───────┘                                │
│                                │                                        │
│              ┌─────────────────┼─────────────────┐                      │
│              │ 1:N             │ 1:N              │ 1:N                  │
│              ▼                 ▼                  ▼                      │
│  ┌───────────────────┐ ┌─────────────┐ ┌─────────────────┐            │
│  │   Experience       │ │  Activity   │ │ DerivedInsight   │            │
│  │───────────────────│ │─────────────│ │─────────────────│            │
│  │ id: ExperienceId   │ │ agent_id    │ │ id: InsightId    │            │
│  │ collective_id      │ │ collective  │ │ collective_id    │            │
│  │ content: String    │ │ status      │ │ content: String  │            │
│  │ experience_type    │ │ task_desc   │ │ insight_type     │            │
│  │ embedding: [f32]   │ │ started_at  │ │ embedding: [f32] │            │
│  │ importance: f32    │ │ updated_at  │ │ source_exp_ids   │            │
│  │ confidence: f32    │ └─────────────┘ │ importance: f32  │            │
│  │ domain: [String]   │                  │ created_at       │            │
│  │ source_agent       │                  └─────────────────┘            │
│  │ source_task        │                                                 │
│  │ related_files      │                                                 │
│  │ applications_count │                                                 │
│  │ created_at         │                                                 │
│  │ updated_at         │                                                 │
│  │ archived: bool     │                                                 │
│  └─────────┬──────────┘                                                 │
│            │                                                            │
│            │ M:N (via ExperienceRelation)                               │
│            ▼                                                            │
│  ┌───────────────────────┐                                             │
│  │ ExperienceRelation     │                                             │
│  │───────────────────────│                                             │
│  │ id: RelationId         │                                             │
│  │ source_id: ExpId       │                                             │
│  │ target_id: ExpId       │                                             │
│  │ relation_type          │                                             │
│  │ strength: f32          │                                             │
│  │ metadata: Option<Str>  │                                             │
│  └────────────────────────┘                                             │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                       PULSEHIVE LAYER (Runtime)                          │
│                                                                          │
│  ┌──────────────────┐  ┌───────────────┐  ┌────────────────┐           │
│  │ AgentDefinition   │  │ Task          │  │ HiveEvent      │           │
│  │──────────────────│  │───────────────│  │────────────────│           │
│  │ name: String      │  │ description   │  │ AgentStarted   │           │
│  │ kind: AgentKind   │  │ collective_id │  │ AgentCompleted │           │
│  └────────┬─────────┘  │ metadata      │  │ LlmCall*       │           │
│           │             └───────────────┘  │ ToolCall*      │           │
│    ┌──────┴───────┐                        │ Experience*    │           │
│    │  AgentKind   │                        │ Relationship*  │           │
│    │──────────────│                        │ Insight*       │           │
│    │ Llm(Config)  │                        │ Substrate*     │           │
│    │ Sequential   │                        └────────────────┘           │
│    │ Parallel     │                                                     │
│    │ Loop         │     ┌───────────────┐  ┌────────────────┐          │
│    └──────┬───────┘     │ AgentContext   │  │ ToolContext     │          │
│           │             │───────────────│  │────────────────│          │
│    ┌──────▼──────┐      │ experiences   │  │ agent_id       │          │
│    │LlmAgentConf │      │ insights      │  │ collective_id  │          │
│    │─────────────│      │ activities    │  │ substrate: Arc │          │
│    │ system_prmpt│      │ token_count   │  │ event_emitter  │          │
│    │ tools       │      └───────────────┘  └────────────────┘          │
│    │ lens: Lens   │                                                     │
│    │ llm_config   │     ┌───────────────┐  ┌────────────────┐          │
│    │ exp_extractor│     │ ToolResult    │  │ ContextBudget  │          │
│    └──────┬───────┘     │───────────────│  │────────────────│          │
│           │             │ content       │  │ max_tokens     │          │
│    ┌──────▼──────┐      │ is_error      │  │ max_experiences│          │
│    │   Lens       │     │ metadata      │  │ max_insights   │          │
│    │─────────────│      └───────────────┘  └────────────────┘          │
│    │ domain_focus │                                                     │
│    │ type_weights │                                                     │
│    │recency_curve│                                                     │
│    │purpose_embed│                                                     │
│    │attn_budget  │                                                     │
│    └─────────────┘                                                     │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 3. PulseDB Entities (Persistent)

### 3.1 Experience

The fundamental unit of shared knowledge. Each experience is a fact, pattern, decision, or observation recorded by an agent and stored in PulseDB with a 384-dimensional embedding for semantic search.

| Field | Type | Description |
|---|---|---|
| `id` | `ExperienceId` (UUID v7) | Time-ordered unique identifier |
| `collective_id` | `CollectiveId` (UUID v7) | Namespace isolation key |
| `content` | `String` | Human-readable content (max 100KB) |
| `experience_type` | `ExperienceType` | Typed variant with structured metadata |
| `embedding` | `Vec<f32>` | 384-dimensional vector in HNSW index |
| `importance` | `f32` | Weight in [0.0, 1.0], default 0.5 |
| `confidence` | `f32` | Certainty in [0.0, 1.0], default 0.5 |
| `domain` | `Vec<String>` | Domain tags (max 50 tags, 100 chars each) |
| `source_agent` | `AgentId` (String) | Which agent created this experience |
| `source_task` | `Option<TaskId>` | Which task it was recorded during |
| `related_files` | `Vec<String>` | File paths relevant to this experience |
| `applications_count` | `u32` | Reinforcement counter (incremented on use) |
| `created_at` | `Timestamp` | Creation time |
| `updated_at` | `Timestamp` | Last modification time |
| `archived` | `bool` | Soft-delete flag |

### 3.2 ExperienceType

A Rust enum with 9 variants, each carrying structured metadata:

| Variant | Key Fields | Use Case |
|---|---|---|
| `Difficulty` | `description`, `severity` | Problems encountered |
| `Solution` | `problem_ref`, `approach`, `worked` | How problems were solved |
| `ErrorPattern` | `signature`, `fix`, `prevention` | Recurring errors and fixes |
| `SuccessPattern` | `task_type`, `approach`, `quality` | What works well |
| `UserPreference` | `category`, `preference`, `strength` | User-specific preferences |
| `ArchitecturalDecision` | `decision`, `rationale` | Design choices and reasoning |
| `TechInsight` | `technology`, `insight` | Technology-specific knowledge |
| `Fact` | `statement`, `source` | Verified facts with provenance |
| `Generic` | `category` (optional) | Uncategorized knowledge |

### 3.3 ExperienceRelation

A directed edge between two experiences in the knowledge graph.

| Field | Type | Description |
|---|---|---|
| `id` | `RelationId` (UUID v7) | Unique identifier |
| `source_id` | `ExperienceId` | Origin experience |
| `target_id` | `ExperienceId` | Destination experience |
| `relation_type` | `RelationType` | Kind of relationship |
| `strength` | `f32` | Confidence in [0.0, 1.0] |
| `metadata` | `Option<String>` | Free-form context about the relation |

**RelationType enum:**

| Variant | Semantics |
|---|---|
| `Supports` | Source reinforces target |
| `Contradicts` | Source opposes target (creates tension zone) |
| `Elaborates` | Source adds detail to target |
| `Supersedes` | Source replaces target (target may be stale) |
| `Implies` | Source suggests target |
| `RelatedTo` | General association |

Relations are directional. `get_related()` can query by `RelationDirection::Outgoing`, `Incoming`, or `Both`.

### 3.4 DerivedInsight

A synthesized knowledge unit derived from multiple experiences. Created by PulseHive's InsightSynthesizer when experience clusters exceed a density threshold.

| Field | Type | Description |
|---|---|---|
| `id` | `InsightId` (UUID v7) | Unique identifier |
| `collective_id` | `CollectiveId` | Namespace |
| `content` | `String` | Synthesized insight text |
| `insight_type` | `InsightType` | Category of insight |
| `embedding` | `Vec<f32>` | 384d vector (appears in future searches) |
| `source_experience_ids` | `Vec<ExperienceId>` | Experiences that contributed |
| `importance` | `f32` | Computed from source experiences |
| `created_at` | `Timestamp` | When synthesized |

### 3.5 Activity

Tracks which agents are currently active in a collective. Used for activity awareness in context assembly.

| Field | Type | Description |
|---|---|---|
| `agent_id` | `AgentId` | Which agent is active |
| `collective_id` | `CollectiveId` | Which namespace |
| `status` | Status enum | Current state (active, idle, completed) |
| `task_description` | `String` | What the agent is working on |
| `started_at` | `Timestamp` | When the agent started |
| `updated_at` | `Timestamp` | Last heartbeat |

---

## 4. PulseHive Runtime Entities

These types exist only during execution. They are not persisted in PulseDB.

### 4.1 AgentDefinition and AgentKind

```rust
pub struct AgentDefinition {
    pub name: String,
    pub kind: AgentKind,
}

pub enum AgentKind {
    Llm(LlmAgentConfig),
    Sequential(Vec<AgentDefinition>),
    Parallel(Vec<AgentDefinition>),
    Loop { agent: Box<AgentDefinition>, max_iterations: usize },
}
```

`AgentDefinition` is a blueprint, not a running agent. It describes what kind of agent to create and how to configure it. The runtime instantiates actual agent tasks from these definitions when `deploy()` is called.

### 4.2 LlmAgentConfig

Configuration for an LLM-powered agent:

| Field | Type | Description |
|---|---|---|
| `system_prompt` | `String` | Specialization prompt for the agent |
| `tools` | `Vec<Box<dyn Tool>>` | Available tool implementations |
| `lens` | `Lens` | Perception filter for substrate queries |
| `llm_config` | `LlmConfig` | Model selection and parameters |
| `experience_extractor` | `Option<Box<dyn ExperienceExtractor>>` | Custom extraction logic (default if None) |

### 4.3 Lens

Defines how an agent perceives the substrate. Different agents with different lenses see different subsets and rankings of the same underlying data.

| Field | Type | Description |
|---|---|---|
| `domain_focus` | `Vec<String>` | Domain tags to attend to |
| `type_weights` | `HashMap<ExperienceTypeTag, f32>` | Attention weights per experience type |
| `recency_curve` | `RecencyCurve` | Time weighting function |
| `purpose_embedding` | `Vec<f32>` | Semantic focus vector for query warping |
| `attention_budget` | `usize` | Max experiences to perceive |

The `RecencyCurve` enum:
- `Exponential { half_life_hours: f32 }` -- Recent experiences weighted heavily, older ones decay.
- `Uniform` -- All experiences weighted equally regardless of age.

### 4.4 LlmConfig

| Field | Type | Description |
|---|---|---|
| `model` | `String` | Model identifier (e.g., "claude-sonnet-4-6", "glm-5") |
| `temperature` | `f32` | Sampling temperature |
| `max_tokens` | `u32` | Maximum response tokens |

### 4.5 Task

```rust
pub struct Task {
    pub description: String,
    pub collective_id: CollectiveId,
    pub metadata: Option<serde_json::Value>,
}
```

### 4.6 AgentContext

The assembled context presented to an agent after perception and optimization. Contains prioritized experiences, insights, and activity awareness, all within a token budget.

### 4.7 ToolContext and ToolResult

`ToolContext` is provided to tools during execution, giving them access to agent identity, collective scope, the substrate (for tools that need to query or write experiences), and an event emitter.

`ToolResult` encapsulates the output of a tool execution, including content, error status, and optional metadata.

### 4.8 ContextBudget

Controls how much context the ContextOptimizer assembles:

| Field | Type | Description |
|---|---|---|
| `max_tokens` | `usize` | Total token budget for context |
| `max_experiences` | `usize` | Maximum number of experiences to include |
| `max_insights` | `usize` | Maximum number of insights to include |

### 4.9 HiveEvent

A comprehensive enum covering all observable events in the system. See Section 6 for the full variant list.

### 4.10 AgentOutcome

The result of an agent's execution:

```rust
pub enum AgentOutcome {
    Success { response: String, experiences_recorded: usize },
    Failure { error: PulseHiveError },
    Cancelled { reason: String },
}
```

---

## 5. Collective Isolation Model

PulseDB uses `CollectiveId` as a namespace isolation key. Every persistent entity belongs to exactly one collective. This provides multi-tenancy at the data level.

```
┌──────────────────────────────────────────────────────────────┐
│  PulseDB Instance (single file)                              │
│                                                              │
│  ┌────────────────────────┐  ┌────────────────────────┐     │
│  │  Collective A           │  │  Collective B           │     │
│  │  (Project Alpha)        │  │  (Project Beta)         │     │
│  │                         │  │                         │     │
│  │  312 experiences        │  │  47 experiences         │     │
│  │  45 insights            │  │  12 insights            │     │
│  │  89 relations           │  │  23 relations           │     │
│  │  3 active agents        │  │  1 active agent         │     │
│  │                         │  │                         │     │
│  │  INVISIBLE to B's       │  │  INVISIBLE to A's       │     │
│  │  agents                 │  │  agents                 │     │
│  └────────────────────────┘  └────────────────────────┘     │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

**Properties of collective isolation:**

- All queries are scoped by `CollectiveId`. An agent in Collective A cannot retrieve experiences from Collective B.
- `search_similar()`, `get_recent()`, `get_insights()`, and `get_activities()` all require `CollectiveId` as a parameter.
- The HNSW vector index is shared across collectives for storage efficiency, but search results are filtered by collective.
- Cross-collective knowledge sharing is planned as a post-MVP feature via Wisdom Abstraction, which extracts general patterns without project-specific details.

**Why isolation matters:** Raw cross-collective details cause hallucination. An agent might reference file paths, variable names, or error messages from another project that do not exist in the current project. Isolation prevents this class of errors entirely.

---

## 6. HiveEvent Variants

The full `HiveEvent` enum covers four categories:

| Category | Variants | Purpose |
|---|---|---|
| Agent lifecycle | `AgentStarted`, `AgentCompleted` | Track agent execution |
| LLM interactions | `LlmCallStarted`, `LlmCallCompleted`, `LlmTokenStreamed` | Monitor inference |
| Tool execution | `ToolCallStarted`, `ToolCallCompleted`, `ToolApprovalRequested` | Track tool usage |
| Substrate operations | `ExperienceRecorded`, `RelationshipInferred`, `InsightGenerated`, `SubstratePerceived` | Observe knowledge growth |

Each variant carries relevant context (agent IDs, model names, durations, counts) as struct fields for structured logging and monitoring.

---

## 7. Embedding Space

All semantic search in PulseHive operates within a 384-dimensional embedding space.

**Model**: `all-MiniLM-L6-v2` (shipped with PulseDB via ONNX runtime)

**Index**: HNSW (Hierarchical Navigable Small World) graph with configurable parameters:
- `ef_construction`: Build-time quality (higher = better recall, slower build)
- `ef_search`: Query-time quality (higher = better recall, slower search)
- `m`: Number of connections per node (higher = better recall, more memory)

**Similarity metric**: Cosine similarity. Scores range from 0.0 (orthogonal) to 1.0 (identical).

**Embedding ownership**: In MVP (Builtin mode), PulseDB computes embeddings internally when `store_experience()` is called with `embedding: None`. In future External mode, PulseHive computes embeddings via the `EmbeddingProvider` trait and passes vectors to PulseDB.

**Performance at 1K experiences:**

| Operation | Latency |
|---|---|
| `store_experience` | 5.5 ms |
| `search_similar` (k=20) | 95 us |
| `get_context_candidates` | 189 us |
| `get_experience` by ID | 1.3 us |

**Targets at 100K experiences**: store < 10ms, search < 50ms, context < 100ms.

---

## 8. ID System

All persistent IDs use UUID v7 (time-ordered). This provides globally unique identifiers that sort chronologically, enabling efficient time-range queries without a separate timestamp index.

| Type | Rust Type | Purpose |
|---|---|---|
| `CollectiveId` | UUID v7 | Namespace isolation |
| `ExperienceId` | UUID v7 | Experience identity |
| `InsightId` | UUID v7 | Derived insight identity |
| `RelationId` | UUID v7 | Experience relation identity |
| `AgentId` | `String` | Agent identity (product-defined) |
| `TaskId` | `String` | Task identity (product-defined) |
| `UserId` | `String` | User identity (product-defined) |

UUID v7 types are generated by PulseDB on write. String-based IDs (`AgentId`, `TaskId`, `UserId`) are product-defined and passed through as opaque identifiers.

---

## 9. Search and Filter Types

### SearchFilter

Used by PulseDB to narrow searches before results reach PulseHive:

| Field | Type | Description |
|---|---|---|
| `domains` | `Option<Vec<String>>` | Filter to specific domain tags |
| `experience_types` | `Option<Vec<ExperienceType>>` | Filter to specific types |
| `min_importance` | `Option<f32>` | Minimum importance threshold |
| `min_confidence` | `Option<f32>` | Minimum confidence threshold |
| `since` | `Option<Timestamp>` | Only experiences after this time |
| `exclude_archived` | `bool` | Skip archived experiences (default: true) |

### ContextRequest and ContextCandidates

`ContextRequest` drives PulseDB's unified retrieval API, assembling all relevant data for a single perception cycle:

```rust
pub struct ContextRequest {
    pub collective_id: CollectiveId,
    pub query_embedding: Vec<f32>,     // Warped through Lens by PulseHive
    pub max_similar: usize,            // Default: 20
    pub max_recent: usize,             // Default: 10
    pub include_insights: bool,        // Default: true
    pub include_relations: bool,       // Default: true
    pub include_active_agents: bool,   // Default: true
    pub filter: SearchFilter,
}

pub struct ContextCandidates {
    pub similar_experiences: Vec<SearchResult>,  // Sorted by similarity DESC
    pub recent_experiences: Vec<Experience>,      // Sorted by timestamp DESC
    pub insights: Vec<DerivedInsight>,
    pub relations: Vec<ExperienceRelation>,
    pub active_agents: Vec<Activity>,
}
```

PulseDB returns raw candidates. PulseHive's ContextOptimizer applies temporal decay, lens re-ranking, and token budget packing to produce the final `AgentContext`.

---

## 10. Watch System Types

PulseDB's real-time notification system for experience changes:

```rust
pub struct WatchEvent {
    pub experience_id: ExperienceId,
    pub collective_id: CollectiveId,
    pub event_type: WatchEventType,
    pub timestamp: Timestamp,
}

pub enum WatchEventType {
    Created,
    Updated,
    Archived,
    Deleted,
}

pub struct WatchFilter {
    pub domains: Option<Vec<String>>,
    pub experience_types: Option<Vec<ExperienceType>>,
    pub min_importance: Option<f32>,
}
```

The Watch system delivers events via crossbeam channels with sub-100ns overhead. Agents subscribe via `SubstrateProvider::watch(collective_id)`, which returns a `Stream<Item = WatchEvent>`.

---

*This document describes the data model as of SPEC v0.4.0. Types and fields will be validated during Phase 1 implementation.*
