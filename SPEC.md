# PulseHive: Shared Consciousness SDK for Multi-Agent AI Systems

> **Version:** 0.4.0-spec
> **Status:** Active Specification
> **Created:** January 2026
> **Last Updated:** March 2026

---

## Executive Summary

PulseHive is a Rust SDK for building multi-agent AI systems where agents share consciousness through a persistent substrate instead of passing messages. When one agent learns something, all agents in that collective immediately perceive it — no coordination protocol, no message queue, no explicit sharing.

The key insight: **agents don't communicate — they share consciousness.** PulseHive makes the shared-state model (validated by LangGraph, Google ADK) database-native through PulseDB, giving it real-time reactivity, semantic queryability, and persistence that in-memory approaches can't match.

Products built on PulseHive include dev automation tools, pharmacovigilance systems, personal assistants, research engines, and any domain requiring multiple AI agents to collaborate with shared understanding.

---

## Table of Contents

1. [Vision & Core Innovation](#vision--core-innovation)
2. [Architecture Overview](#architecture-overview)
3. [Core Primitives](#core-primitives)
4. [Intelligence Layer](#intelligence-layer)
5. [Field Dynamics](#field-dynamics)
6. [LLM Provider Abstraction](#llm-provider-abstraction)
7. [Embedding System](#embedding-system)
8. [Observability & Events](#observability--events)
9. [Streaming](#streaming)
10. [Human-in-the-Loop](#human-in-the-loop)
11. [Memory & State Architecture](#memory--state-architecture)
12. [Substrate Integration](#substrate-integration)
13. [Crate Structure](#crate-structure)
14. [Deployment Patterns](#deployment-patterns)
15. [SDK Consumer API](#sdk-consumer-api)
16. [Development Phases](#development-phases)
17. [Anti-Patterns](#anti-patterns)
18. [Open Questions](#open-questions)

---

## Vision & Core Innovation

### The Problem with Current Multi-Agent Systems

Traditional multi-agent architectures suffer from fundamental coordination overhead:

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│  TRADITIONAL MULTI-AGENT (Message Passing)                                       │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│  Agent A              Agent B              Agent C                               │
│  ┌──────┐            ┌──────┐            ┌──────┐                               │
│  │      │───"I found │      │───"What    │      │                               │
│  │      │   X"──────►│      │  is X?"───►│      │                               │
│  │      │◄──"It's    │      │◄──"In      │      │                               │
│  │      │   Y"───────│      │   Z"───────│      │                               │
│  └──────┘            └──────┘            └──────┘                               │
│                                                                                  │
│  Problems:                                                                       │
│  • Latency at every message exchange                                             │
│  • Context lost in translation (rich understanding → text → reconstructed)      │
│  • Coordination overhead scales O(n²) with agent count                           │
│  • No true shared understanding — each agent has a partial view                  │
│  • 36.9% of multi-agent failures from inconsistent state (O'Reilly 2025)        │
│                                                                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### The Shared Consciousness Model

PulseHive takes inspiration from a symbiote hive mind — all agents are connected to a single consciousness. They don't communicate; they simply **know** what the collective knows.

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│  PULSEHIVE (Shared Substrate)                                                    │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│                    ┌─────────────────────────────────────┐                      │
│                    │          SUBSTRATE (PulseDB)         │                      │
│                    │                                      │                      │
│                    │  Experience: "Anomaly detected in    │                      │
│                    │              batch ABC-123, cross-   │                      │
│                    │              referenced with prior   │                      │
│                    │              pattern from batch 122" │                      │
│                    │                                      │                      │
│                    │  [Written once, instantly visible    │                      │
│                    │   to all agents reading substrate]   │                      │
│                    │                                      │                      │
│                    └─────────────────────────────────────┘                      │
│                              ▲     ▲     ▲                                      │
│                              │     │     │                                      │
│                         read │     │     │ read                                 │
│                              │   write   │                                      │
│                              │     │     │                                      │
│                    ┌─────┐   │   ┌─────┐ │   ┌─────┐                            │
│                    │  A  │───┘   │  B  │─┴───│  C  │                            │
│                    └─────┘       └─────┘     └─────┘                            │
│                                                                                  │
│  Agent A discovers anomaly → Writes to substrate                                │
│  Agent B (analyzing related data) → Already sees it in next perception          │
│  Agent C (generating report) → Already knows what to include                    │
│                                                                                  │
│  NO MESSAGES. They all read from the same consciousness.                         │
│                                                                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### Key Innovations

1. **Zero Coordination Overhead**: No message passing, no protocol negotiation, no routing
2. **Instantaneous Knowledge Sharing**: Write once, perceived by all through Watch system
3. **Compounding Intelligence**: Every task makes the collective smarter — experiences persist
4. **Memory Isolation**: Per-collective substrate prevents cross-project contamination
5. **Database-Native State**: Unlike LangGraph (in-memory checkpoints) or CrewAI (message-passing), state is persistent, queryable, and real-time from the ground up
6. **Lens-Based Perception**: Agents perceive the same substrate differently based on their role — no agent sees everything, each sees what's relevant

### PulseHive vs Existing Frameworks

| Capability | LangGraph | Google ADK | CrewAI | PulseHive |
|---|---|---|---|---|
| Shared state model | Typed state (in-memory) | Session state (in-memory) | Message passing | Database-native (PulseDB) |
| Cross-agent real-time | Superstep boundaries | Shared dict | No | Watch system (instant) |
| Semantic search over history | No | No | ChromaDB (basic) | HNSW native |
| Pre-computed reasoning | No | No | No | InsightSynthesizer |
| Per-agent perception | No | No | No | Lens system |
| Temporal decay | No | No | Basic | Exponential + reinforcement |
| Persistence | Checkpoint snapshots | Session service | SQLite | Continuous (PulseDB) |
| Language | Python | Python | Python | Rust (with Python/TS bindings planned) |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                           PRODUCTS (Built on PulseHive)                          │
│                                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐        │
│  │  DevStudio   │  │  PV Agent    │  │  Personal    │  │  Research    │        │
│  │  (builds     │  │  System      │  │  Assistant   │  │  Engine      │        │
│  │  software)   │  │  (pharma)    │  │  (tasks)     │  │  (analysis)  │        │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘        │
│         └──────────────────┴─────────────────┴─────────────────┘                │
│                                     │                                            │
│                          uses PulseHive SDK                                      │
│                                     │                                            │
├─────────────────────────────────────┼────────────────────────────────────────────┤
│                                     ▼                                            │
│  ┌───────────────────────────────────────────────────────────────────────────┐  │
│  │                         PULSEHIVE SDK                                      │  │
│  │                                                                            │  │
│  │  pulsehive-core         pulsehive-runtime       pulsehive-anthropic       │  │
│  │  ─────────────          ─────────────────       ──────────────────        │  │
│  │  • Agent trait          • HiveMind              • Claude API impl         │  │
│  │  • Tool trait           • Agentic loop engine   pulsehive-openai          │  │
│  │  • Lens struct          • Workflow agents        ──────────────────        │  │
│  │  • LlmProvider trait    • Intelligence layer    • OpenAI-compatible       │  │
│  │  • HiveEvent enum       • ContextOptimizer        (OpenAI, GLM, vLLM,    │  │
│  │  • ApprovalHandler      • RelationshipDetector    LM Studio, Ollama)     │  │
│  │  • EmbeddingProvider    • InsightSynthesizer                              │  │
│  │                                                                            │  │
│  └───────────────────────────────────┬───────────────────────────────────────┘  │
│                                      │                                           │
│                           uses SubstrateProvider                                 │
│                                      │                                           │
│                                      ▼                                           │
│  ┌───────────────────────────────────────────────────────────────────────────┐  │
│  │                            PULSEDB (v0.1.1)                                │  │
│  │                     Storage Substrate Layer                                 │  │
│  │                                                                            │  │
│  │  • Experience CRUD          • HNSW vector search (384d)                   │  │
│  │  • Relationship storage     • Insight storage                              │  │
│  │  • Activity tracking        • Real-time Watch system                       │  │
│  │  • Context assembly         • SubstrateProvider trait (owned by PulseDB)   │  │
│  │                                                                            │  │
│  └───────────────────────────────────────────────────────────────────────────┘  │
│                                                                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

**Architecture boundary**: PulseDB stores and retrieves. PulseHive thinks. All intelligence algorithms (relationship detection, insight synthesis, context optimization, lens perception, field dynamics) live in PulseHive. PulseDB is a pure storage layer.

---

## Core Primitives

PulseHive has exactly **five** core primitives. Products built on the SDK compose these to create domain-specific agent systems.

### 1. HiveMind — The Orchestrator

The central coordinator that deploys agents, manages the substrate, and runs intelligence algorithms.

```rust
pub struct HiveMind {
    substrate: Box<dyn SubstrateProvider>,
    relationship_detector: RelationshipDetector,
    insight_synthesizer: InsightSynthesizer,
    context_optimizer: ContextOptimizer,
    llm_router: LlmRouter,
    event_bus: EventBus,
}

impl HiveMind {
    /// Builder pattern with compile-time validation
    pub fn builder() -> HiveMindBuilder { ... }

    /// Deploy agents for tasks, returns event stream
    pub async fn deploy(
        &self,
        agents: Vec<AgentDefinition>,
        tasks: Vec<Task>,
    ) -> Result<impl Stream<Item = HiveEvent>> { ... }

    /// Record experience with automatic relationship inference
    pub async fn record_experience(&self, experience: NewExperience) -> Result<ExperienceId> {
        // 1. Store in substrate
        let id = self.substrate.store_experience(experience).await?;

        // 2. INTELLIGENCE: Detect relationships
        let relations = self.relationship_detector
            .infer_relations(&stored, &self.substrate).await?;
        for rel in relations {
            self.substrate.store_relation(rel).await?;
        }

        // 3. INTELLIGENCE: Synthesize insights if threshold met
        if self.insight_synthesizer.should_synthesize(&stored) {
            let insights = self.insight_synthesizer
                .synthesize(&self.substrate, stored.collective_id).await?;
            for insight in insights {
                self.substrate.store_insight(insight).await?;
            }
        }

        Ok(id)
    }

    /// Assemble context for an agent through its lens
    pub async fn get_context(
        &self,
        lens: &Lens,
        task: &Task,
        budget: ContextBudget,
    ) -> Result<AgentContext> { ... }
}
```

### 2. Agent — LLM or Workflow

Agents come in two varieties: **LLM agents** (powered by language models, dynamic reasoning) and **Workflow agents** (deterministic orchestration, no LLM overhead).

```rust
/// Blueprint for creating an agent (not a running agent)
pub struct AgentDefinition {
    pub name: String,
    pub kind: AgentKind,
}

pub enum AgentKind {
    /// LLM-powered agent with tools and lens-based perception
    Llm(LlmAgentConfig),

    /// Runs sub-agents sequentially — each sees previous agents' experiences
    Sequential(Vec<AgentDefinition>),

    /// Runs sub-agents in parallel — all share substrate in real-time
    Parallel(Vec<AgentDefinition>),

    /// Repeats sub-agent until max_iterations or completion
    Loop {
        agent: Box<AgentDefinition>,
        max_iterations: usize,
    },
}

pub struct LlmAgentConfig {
    /// System prompt that specializes this agent
    pub system_prompt: String,

    /// Tools this agent can use
    pub tools: Vec<Box<dyn Tool>>,

    /// How this agent perceives the substrate
    pub lens: Lens,

    /// Which LLM to use
    pub llm_config: LlmConfig,

    /// Optional: override default experience extraction
    pub experience_extractor: Option<Box<dyn ExperienceExtractor>>,
}
```

**The Agentic Loop** (framework-provided, runs for each LlmAgent):

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                         AGENTIC LOOP (per LlmAgent)                              │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│  1. PERCEIVE                                                                     │
│     Query substrate through agent's lens                                         │
│     Apply temporal decay and re-ranking                                          │
│     Present as intrinsic knowledge ("You understand that...")                    │
│           │                                                                      │
│           ▼                                                                      │
│  2. THINK                                                                        │
│     Send (system prompt + substrate context + task + history) to LLM            │
│           │                                                                      │
│           ▼                                                                      │
│  3. ACT                                                                          │
│     LLM returns either:                                                          │
│     ┌─ Tool call → execute tool → add result to history → go to THINK           │
│     └─ Final response → go to RECORD                                            │
│           │                                                                      │
│           ▼                                                                      │
│  4. RECORD                                                                       │
│     Extract experiences from the session (success patterns, errors, insights)   │
│     Write to substrate → all other agents perceive instantly                    │
│                                                                                  │
│  Substrate refresh: For long tasks, re-perceive every N tool calls              │
│  so the agent sees changes from other agents mid-task.                           │
│                                                                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

**Workflow agents** compose without LLM overhead:

```rust
// A phased workflow: explore in parallel, then plan, then implement in parallel
let workflow = AgentDefinition {
    name: "Feature Pipeline".into(),
    kind: AgentKind::Sequential(vec![
        AgentDefinition {
            name: "Exploration".into(),
            kind: AgentKind::Parallel(vec![explorer_a, explorer_b, explorer_c]),
        },
        planner_agent,
        AgentDefinition {
            name: "Implementation".into(),
            kind: AgentKind::Parallel(vec![worker_a, worker_b]),
        },
        reviewer_agent,
    ]),
};
```

### 3. Tool — Pluggable Capabilities

Products implement the `Tool` trait for their domain-specific capabilities.

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (shown to LLM for selection)
    fn name(&self) -> &str;

    /// Description (LLM uses this to decide when to invoke)
    fn description(&self) -> &str;

    /// JSON Schema for parameters
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool
    async fn execute(
        &self,
        params: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult>;

    /// Whether this tool requires human approval before execution
    fn requires_approval(&self) -> bool { false }
}

/// Context available to tools during execution
pub struct ToolContext {
    pub agent_id: AgentId,
    pub collective_id: CollectiveId,
    pub substrate: Arc<dyn SubstrateProvider>,
    pub event_emitter: EventEmitter,
}
```

### 4. Lens — Perception Abstraction

A Lens defines how an agent perceives the substrate. Different agents see the same substrate differently based on their role.

```rust
pub struct Lens {
    /// Which domains this agent attends to (e.g., ["safety", "clinical"])
    pub domain_focus: Vec<String>,

    /// Attention weights for different experience types
    pub type_weights: HashMap<ExperienceTypeTag, f32>,

    /// Time decay function (how much to weight recent vs old)
    pub recency_curve: RecencyCurve,

    /// Semantic focus (embedding of agent's current purpose)
    pub purpose_embedding: Vec<f32>,

    /// Maximum experiences to perceive (attention budget)
    pub attention_budget: usize,
}

pub enum RecencyCurve {
    /// Recent experiences weighted heavily, old ones decay
    Exponential { half_life_hours: f32 },
    /// All experiences equally weighted
    Uniform,
}
```

**How a Lens works:**

```
1. Agent's query embedding (raw purpose)
       │
       ▼
2. Lens transformation (PulseHive)
   - Warp embedding through lens focus
   - Stretches relevant dimensions, compresses irrelevant ones
       │
       ▼
3. search_similar(collective, warped_embedding, k)  (PulseDB)
   - HNSW approximate nearest neighbor search
   - Returns (Experience, similarity_score) tuples
       │
       ▼
4. Post-search re-ranking (PulseHive)
   - Apply domain-specific weight multipliers
   - Factor in temporal decay
   - Factor in attractor strength (importance × confidence)
       │
       ▼
5. Intrinsic knowledge presentation
   - "You understand that..." not "Retrieved docs say..."
   - Knowledge woven into agent identity, not presented as external
```

### 5. Experience — Substrate Knowledge Unit

Experiences are the atoms of shared consciousness. Defined in PulseDB, re-exported by PulseHive.

```rust
// Re-exported from pulsedb crate
pub use pulsedb::{Experience, NewExperience, ExperienceType, ExperienceId};

pub enum ExperienceType {
    Difficulty { description: String, severity: Severity },
    Solution { problem_ref: Option<ExperienceId>, approach: String, worked: bool },
    ErrorPattern { signature: String, fix: String, prevention: String },
    SuccessPattern { task_type: String, approach: String, quality: f32 },
    UserPreference { category: String, preference: String, strength: f32 },
    ArchitecturalDecision { decision: String, rationale: String },
    TechInsight { technology: String, insight: String },
    Fact { statement: String, source: String },
    Generic { category: Option<String> },
}
```

**Experience flow:**

```
Agent completes task → PulseHive extracts learnings → store_experience()
    → PulseDB stores with embedding in HNSW index
    → Watch system emits WatchEvent::Created
    → All subscribed agents perceive the new experience on next substrate read
    → RelationshipDetector infers connections to existing experiences
    → InsightSynthesizer generates cross-experience insights if threshold met
```

---

## Intelligence Layer

The intelligence that makes PulseHive special lives here — NOT in the database.

> **Architecture Decision**: All intelligence algorithms live in PulseHive. PulseDB is a pure storage layer.
> This was formalized in PulseDB SPEC v0.4.0.

### RelationshipDetector

Automatically infers relationships between experiences.

**Algorithm**: Embedding Similarity + LLM Classification

```rust
impl RelationshipDetector {
    pub struct Config {
        pub auto_threshold: f32,      // Auto-create if similarity > this (default: 0.85)
        pub suggest_threshold: f32,   // Suggest if similarity > this (default: 0.65)
        pub use_llm_classification: bool,
    }

    pub async fn infer_relations(
        &self,
        new_experience: &Experience,
        substrate: &dyn SubstrateProvider,
    ) -> Result<Vec<NewExperienceRelation>> {
        // 1. Find semantically similar experiences
        let similar = substrate.search_similar(
            new_experience.collective_id,
            &new_experience.embedding,
            20,
        ).await?;

        // 2. For high-similarity pairs, classify relationship type
        // Pattern matching on ExperienceType pairs:
        //   Difficulty + Solution → Fixes
        //   ErrorPattern + ErrorPattern (similar signature) → Supersedes
        //   ArchitecturalDecision + TechInsight → Implies
        // If inconclusive, use LLM classification
        ...
    }
}
```

### InsightSynthesizer

Generates derived insights from clusters of related experiences.

**Algorithm**: Threshold-Triggered + Debounced Synthesis

```rust
impl InsightSynthesizer {
    pub struct Config {
        pub relation_density_threshold: usize,  // Trigger when N relations in cluster (default: 5)
        pub debounce_seconds: u64,
        pub min_importance: f32,
    }

    /// Synthesize insights from related experience clusters
    pub async fn synthesize(
        &self,
        substrate: &dyn SubstrateProvider,
        collective_id: CollectiveId,
    ) -> Result<Vec<NewDerivedInsight>> {
        // 1. Find clusters of related experiences via graph traversal
        // 2. For each cluster above threshold, use LLM to synthesize
        // 3. Return insights to be stored via substrate.store_insight()
        //
        // Example: 3 experiences about auth patterns →
        //   Insight: "Auth requires httpOnly cookies + adapter pattern + middleware guards"
        ...
    }
}
```

### ContextOptimizer

Assembles optimal context for agent tasks within a token budget.

```rust
impl ContextOptimizer {
    pub struct Config {
        pub decay_half_life_hours: f32,    // default: 72.0
        pub reinforcement_boost: f32,      // default: 0.1 per application
    }

    /// Compute decayed importance for an experience
    pub fn compute_decayed_importance(&self, experience: &Experience, now: Timestamp) -> f32 {
        let age_hours = (now - experience.created_at).as_hours();
        let decay_factor = 0.5_f32.powf(age_hours / self.config.decay_half_life_hours);
        let reinforcement = 1.0 + (experience.applications_count as f32 * self.config.reinforcement_boost);
        experience.importance * decay_factor * reinforcement
    }

    /// Assemble context as intrinsic knowledge
    pub fn assemble(
        &self,
        experiences: Vec<Experience>,
        insights: Vec<DerivedInsight>,
        activities: Vec<Activity>,
        budget: ContextBudget,
    ) -> AgentContext {
        // 1. Compute decayed importance for all experiences
        // 2. Prioritize: insights > high-importance > recent
        // 3. Pack within token budget
        // 4. Present as intrinsic knowledge ("You understand that...")
        // 5. Include activity awareness ("You're aware that agent X is working on Y")
        ...
    }
}
```

### Why Intelligence Lives in PulseHive (Not PulseDB)

| Factor | Why PulseHive Wins |
|--------|-------------------|
| **Speed** | Closer to LLM providers — no DB→LLM round-trips for inference |
| **Reliability** | Inference can be retried, algorithms updated without DB migrations |
| **Testability** | A/B test strategies, adapt as LLMs improve |
| **Separation** | PulseDB stays focused on efficient storage/retrieval |

---

## Field Dynamics

The substrate is not static storage — it's a living embedding manifold where experiences act as attractors that influence perception.

### Attractor Model

Each experience is an attractor in embedding space:

```rust
pub struct AttractorDynamics {
    pub experience_id: ExperienceId,
    pub strength: f32,        // importance × confidence × reinforcement
    pub radius: f32,          // influence radius in embedding space
    pub warp_factor: f32,     // how strongly it pulls nearby queries
}

impl AttractorDynamics {
    pub fn influence_at(&self, query_embedding: &[f32], experience_embedding: &[f32]) -> f32 {
        let distance = cosine_distance(query_embedding, experience_embedding);
        if distance > self.radius { return 0.0; }
        self.strength * (1.0 - distance / self.radius) * self.warp_factor
    }
}
```

### Dynamics Summary

| Concept | Implementation |
|---------|---------------|
| **Decay** | Experience importance decreases over time (configurable half-life) |
| **Reinforcement** | Using an experience increases its importance (`applications_count`) |
| **Propagation** | New experiences strengthen semantically related existing ones |
| **Warping** | High-importance experiences attract queries toward them |

### Post-MVP Gaps (computed at query time)

1. **Temporal decay**: Computed from `(importance, timestamp)` at query time — no native PulseDB decay
2. **Weighted search**: Lens warps query embedding pre-search; PulseDB does standard cosine similarity
3. **Attractor energy lifecycle**: `applications_count` is a proxy — dedicated lifecycle needs real-world data

---

## LLM Provider Abstraction

PulseHive is provider-agnostic. Products choose which LLM for which agent.

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<LlmResponse>;

    /// Stream tokens for real-time output
    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<impl Stream<Item = Result<LlmChunk>>>;
}

pub struct LlmConfig {
    pub model: String,           // "claude-sonnet-4-6", "glm-5", etc.
    pub temperature: f32,
    pub max_tokens: u32,
}
```

### Built-in Providers

| Crate | Covers | Config |
|-------|--------|--------|
| `pulsehive-anthropic` | Claude models (Opus, Sonnet, Haiku) | API key + model name |
| `pulsehive-openai` | Any OpenAI-compatible API: OpenAI, GLM, vLLM, LM Studio, Ollama, Together, Groq | API key + base_url + model name |

```rust
// Example: GLM-5 via OpenAI-compatible provider
let glm = OpenAICompatibleProvider::new(OpenAIConfig {
    api_key: env::var("GLM_API_KEY")?,
    base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
    model: "glm-5".into(),
});
```

> **Future Optimization (REFRAG-style)**: Current LLM APIs are text-in/text-out black boxes.
> If providers add support for direct embedding/KV cache injection (or we self-host models),
> we could skip the text roundtrip and feed pre-computed embeddings directly to the decoder
> for ~30x faster time-to-first-token. See `discussion.md` for the full REFRAG analysis.

---

## Embedding System

**MVP**: PulseDB's Builtin mode handles embeddings. PulseHive doesn't touch them.

PulseDB ships with an ONNX runtime and `all-MiniLM-L6-v2` (384d). When `store_experience()` is called with `embedding: None`, PulseDB computes it internally.

**Future (Phase 2+)**: `EmbeddingProvider` trait for domain-specific models:

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}
```

When set, PulseHive computes embeddings via the provider and passes vectors to PulseDB in External mode. Products that need medical, code, or multilingual embeddings opt in. Products that don't care get PulseDB's default automatically.

---

## Observability & Events

Observability is a core architectural primitive, not a bolt-on.

```rust
pub enum HiveEvent {
    // Agent lifecycle
    AgentStarted { agent_id: AgentId, name: String, kind: AgentKindTag },
    AgentCompleted { agent_id: AgentId, outcome: AgentOutcome },

    // LLM interactions
    LlmCallStarted { agent_id: AgentId, model: String, token_count: usize },
    LlmCallCompleted { agent_id: AgentId, model: String, duration_ms: u64 },
    LlmTokenStreamed { agent_id: AgentId, token: String },

    // Tool execution
    ToolCallStarted { agent_id: AgentId, tool_name: String },
    ToolCallCompleted { agent_id: AgentId, tool_name: String, duration_ms: u64 },
    ToolApprovalRequested { agent_id: AgentId, tool_name: String, action: PendingAction },

    // Substrate operations
    ExperienceRecorded { experience_id: ExperienceId, agent_id: AgentId },
    RelationshipInferred { relation_id: RelationId },
    InsightGenerated { insight_id: InsightId, source_count: usize },

    // Perception
    SubstratePerceived { agent_id: AgentId, experience_count: usize, insight_count: usize },
}
```

Built on the `tracing` crate. Every operation emits structured spans and events. Products plug in any `tracing-subscriber`:

- `tracing-subscriber::fmt` — stdout/stderr logging
- `tracing-opentelemetry` — OpenTelemetry exporters (Jaeger, Datadog, etc.)
- Custom subscribers for product-specific dashboards

No vendor lock-in. No proprietary observability platform required.

---

## Streaming

`HiveMind::deploy()` returns a `Stream<Item = HiveEvent>`. Products consume this for real-time UI updates, monitoring, and logging.

```rust
let mut stream = hive.deploy(agents, tasks).await?;

while let Some(event) = stream.next().await {
    match event {
        HiveEvent::LlmTokenStreamed { token, .. } => {
            // Real-time chat UI
            print!("{}", token);
        }
        HiveEvent::AgentCompleted { outcome, .. } => {
            // Dashboard update
            update_ui(outcome);
        }
        HiveEvent::ExperienceRecorded { experience_id, .. } => {
            // Audit log
            log::info!("New experience: {}", experience_id);
        }
        _ => {}
    }
}
```

The substrate's Watch system also feeds real-time events to running agents, enabling mid-task perception of changes from other agents.

---

## Human-in-the-Loop

The framework provides primitives. Products define policies.

```rust
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn request_approval(&self, action: &PendingAction) -> Result<ApprovalResult>;
}

pub enum ApprovalResult {
    Approved,
    Denied { reason: String },
    Modified { new_params: serde_json::Value },
}

pub struct PendingAction {
    pub agent_id: AgentId,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub description: String,
}
```

Tools declare `requires_approval()`. When the agentic loop encounters such a tool, it calls the `ApprovalHandler` before execution. What requires approval and how it's handled is entirely product-defined:

```rust
// Product implements their approval UX
struct CLIApproval;
struct AutoApprove;
struct SlackApproval { channel: String }
struct WebhookApproval { url: String }
```

---

## Memory & State Architecture

### Collective Isolation

Each project/tenant gets an isolated **collective** — a namespace in PulseDB. Experiences in one collective are invisible to agents in another.

```
User: Draco
├── Collective A (Project Alpha)     ← Full detail, isolated
│   └── 312 experiences, 45 insights, 89 relations
├── Collective B (Project Beta)      ← Full detail, isolated
│   └── 47 experiences, 12 insights, 23 relations
└── User-Level Wisdom (Abstracted)   ← Cross-project patterns only
    └── "Prefers functional components", "Uses httpOnly cookies"
```

### Why Isolation Matters

Raw cross-collective details cause hallucination. An agent might reference a file path from Project A that doesn't exist in Project B. Isolation prevents this. Cross-project knowledge sharing happens only through **wisdom abstraction** — extracting general patterns ("prefer middleware-based auth guards") without project-specific details (file paths, variable names, error messages from that project).

### Wisdom Abstraction (Post-MVP)

```rust
impl WisdomAbstractor {
    /// Extract patterns from a collective's experiences for cross-project sharing
    /// Algorithm: Hierarchical clustering + confidence decay
    /// Requires: N occurrences across M collectives before promoting
    pub async fn abstract_wisdom(
        &self,
        substrate: &dyn SubstrateProvider,
        collective_id: CollectiveId,
    ) -> Result<Vec<AbstractedWisdom>> { ... }
}
```

---

## Substrate Integration

PulseHive accesses storage exclusively through the `SubstrateProvider` trait.

```rust
// Defined in PulseDB, re-exported by PulseHive
pub use pulsedb::SubstrateProvider;

#[async_trait]
pub trait SubstrateProvider: Send + Sync {
    // Experience operations
    async fn store_experience(&self, exp: NewExperience) -> Result<ExperienceId>;
    async fn get_experience(&self, id: ExperienceId) -> Result<Option<Experience>>;

    // Search operations
    async fn search_similar(&self, collective: CollectiveId, embedding: &[f32], k: usize)
        -> Result<Vec<(Experience, f32)>>;
    async fn get_recent(&self, collective: CollectiveId, limit: usize)
        -> Result<Vec<Experience>>;

    // Relation & insight storage
    async fn store_relation(&self, rel: NewExperienceRelation) -> Result<RelationId>;
    async fn get_related(&self, exp_id: ExperienceId)
        -> Result<Vec<(Experience, ExperienceRelation)>>;
    async fn store_insight(&self, insight: NewDerivedInsight) -> Result<InsightId>;
    async fn get_insights(&self, collective: CollectiveId, embedding: &[f32], k: usize)
        -> Result<Vec<(DerivedInsight, f32)>>;

    // Activity tracking
    async fn get_activities(&self, collective: CollectiveId) -> Result<Vec<Activity>>;

    // Context assembly
    async fn get_context_candidates(&self, request: ContextRequest)
        -> Result<ContextCandidates>;

    // Real-time watch
    async fn watch(&self, collective: CollectiveId)
        -> Result<Pin<Box<dyn Stream<Item = WatchEvent> + Send>>>;
}
```

**MVP**: `PulseDBSubstrate` is the only implementation. PulseDB v0.1.1 on crates.io.

**Future**: `PostgresSubstrate` for cloud deployment (Supabase, Neon, RDS). Added when the deployment model requires it. Since we own both codebases, the trait boundary can evolve as needs emerge.

---

## Crate Structure

```
pulsehive/
├── pulsehive-core/            ← Traits and types (zero provider dependencies)
│   ├── src/
│   │   ├── agent.rs           ← Agent, AgentDefinition, AgentKind
│   │   ├── tool.rs            ← Tool trait, ToolContext, ToolResult
│   │   ├── lens.rs            ← Lens, RecencyCurve
│   │   ├── llm.rs             ← LlmProvider trait, LlmConfig, Message
│   │   ├── event.rs           ← HiveEvent enum
│   │   ├── approval.rs        ← ApprovalHandler trait
│   │   ├── embedding.rs       ← EmbeddingProvider trait (future)
│   │   └── error.rs           ← PulseHiveError enum
│   └── Cargo.toml             ← depends on: pulsedb (for re-exports), serde, async-trait
│
├── pulsehive-runtime/         ← HiveMind, execution engine, intelligence
│   ├── src/
│   │   ├── hivemind.rs        ← HiveMind struct, builder, deploy()
│   │   ├── loop.rs            ← Agentic loop engine (perceive→think→act→record)
│   │   ├── workflow.rs        ← Sequential, Parallel, Loop agents
│   │   ├── intelligence/
│   │   │   ├── relationship.rs ← RelationshipDetector
│   │   │   ├── insight.rs      ← InsightSynthesizer
│   │   │   └── context.rs      ← ContextOptimizer
│   │   ├── field.rs           ← AttractorDynamics, decay, propagation
│   │   └── stream.rs          ← Event streaming, Watch integration
│   └── Cargo.toml             ← depends on: pulsehive-core, pulsedb, tokio, tracing
│
├── pulsehive-anthropic/       ← Claude LlmProvider implementation
│   └── Cargo.toml             ← depends on: pulsehive-core, anthropic-sdk
│
├── pulsehive-openai/          ← OpenAI-compatible LlmProvider implementation
│   └── Cargo.toml             ← depends on: pulsehive-core, reqwest
│
└── pulsehive/                 ← Meta-crate, re-exports everything
    └── Cargo.toml             ← depends on: pulsehive-core, pulsehive-runtime
                                  features: ["anthropic", "openai"]
```

Usage: `cargo add pulsehive --features anthropic`

---

## Deployment Patterns

PulseHive is a library crate. It runs wherever the product embeds it.

| Product Type | Embedding Pattern | PulseDB Location |
|---|---|---|
| CLI tool | Single binary | `./project.db` (local file) |
| Desktop app (Tauri) | Linked into app binary | App data directory |
| Server process | Library in web server | Server filesystem or mounted volume |
| Cloud containers (K8s) | Library in container image | Persistent volume |
| Serverless function | Library in function package | Ephemeral (or external PostgresSubstrate) |

PulseDB opens a local file (like SQLite) — zero infrastructure for simple deployments. No server to manage, no connection string, no network hop.

---

## SDK Consumer API

### Simple Example: Single Agent

```rust
use pulsehive::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Create hive mind with PulseDB substrate
    let hive = HiveMind::builder()
        .substrate_path("./my_project.db")
        .llm_provider("anthropic", AnthropicProvider::new(api_key))
        .build()?;

    // 2. Define a tool
    struct WebSearch;
    #[async_trait]
    impl Tool for WebSearch {
        fn name(&self) -> &str { "web_search" }
        fn description(&self) -> &str { "Search the web for information" }
        fn parameters(&self) -> serde_json::Value { json!({"query": "string"}) }
        async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
            let query = params["query"].as_str().unwrap();
            // ... perform search ...
            Ok(ToolResult::text(results))
        }
    }

    // 3. Define an agent
    let researcher = AgentDefinition {
        name: "Researcher".into(),
        kind: AgentKind::Llm(LlmAgentConfig {
            system_prompt: "You are a research analyst. Find and synthesize information.".into(),
            tools: vec![Box::new(WebSearch)],
            lens: Lens::new(vec!["research", "analysis"]),
            llm_config: LlmConfig::new("anthropic", "claude-sonnet-4-6"),
            experience_extractor: None,
        }),
    };

    // 4. Deploy and consume events
    let mut stream = hive.deploy(
        vec![researcher],
        vec![Task::new("Research the latest advances in quantum computing")],
    ).await?;

    while let Some(event) = stream.next().await {
        if let HiveEvent::AgentCompleted { outcome, .. } = event {
            println!("Result: {}", outcome.response);
        }
    }

    Ok(())
}
```

### Advanced Example: Multi-Agent with Workflow

```rust
// Pharmacovigilance: parallel analysis → synthesis → report
let system = AgentDefinition {
    name: "PV Analysis Pipeline".into(),
    kind: AgentKind::Sequential(vec![
        // Phase 1: Parallel analysis
        AgentDefinition {
            name: "Signal Detection".into(),
            kind: AgentKind::Parallel(vec![
                agent("Safety Analyst", safety_prompt, vec![search_faers()], vec!["adverse_events"]),
                agent("Literature Reviewer", lit_prompt, vec![search_pubmed()], vec!["clinical_data"]),
                agent("Statistical Analyst", stats_prompt, vec![run_analysis()], vec!["statistics"]),
            ]),
        },
        // Phase 2: Synthesis (sees all Phase 1 experiences via substrate)
        agent("Medical Reviewer", review_prompt, vec![review_case()], vec!["clinical_assessment"]),
        // Phase 3: Report generation
        agent("Report Generator", report_prompt, vec![generate_report()], vec!["reporting"]),
    ]),
};

let mut stream = hive.deploy(vec![system], vec![task]).await?;
```

---

## Development Phases

### Phase 1: Foundation (Weeks 1-4)

**Goal**: Core traits, single LlmAgent, PulseDB integration

- `pulsehive-core`: Agent, Tool, Lens, LlmProvider, HiveEvent traits/types
- `pulsehive-runtime`: HiveMind (builder + deploy), agentic loop for single LlmAgent
- `pulsehive-openai`: OpenAI-compatible provider (for GLM-5 testing)
- PulseDB integration via SubstrateProvider (Builtin embedding mode)
- Basic event streaming
- CLI example for testing

**Deliverable**: Deploy one agent, have it use tools, see experiences in substrate

### Phase 2: Multi-Agent & Intelligence (Weeks 5-8)

**Goal**: Parallel agents sharing consciousness, intelligence layer

- Workflow agents (Sequential, Parallel, Loop)
- Multi-agent deployment with shared substrate
- RelationshipDetector + InsightSynthesizer
- ContextOptimizer with temporal decay
- Lens-based perception (pre-query warping + post-search re-ranking)
- Watch system integration (mid-task substrate refresh)
- `pulsehive-anthropic` provider

**Deliverable**: Deploy multi-agent swarm, watch shared learning, see derived insights

### Phase 3: Polish & Python Bindings (Weeks 9-12)

**Goal**: Production-readiness, Python ecosystem access

- Python bindings via PyO3 (`pulsehive-py`)
- Human-in-the-loop (ApprovalHandler)
- Observability polish (tracing integration, structured events)
- Error recovery (partial experience recording, agent restart)
- Documentation, examples, API reference
- Performance benchmarking

**Deliverable**: Python developers can build products on PulseHive

### Phase 4: Ecosystem Expansion (Weeks 13-16)

**Goal**: TypeScript bindings, advanced features

- TypeScript bindings via napi-rs (`pulsehive-js`)
- WisdomAbstractor (cross-collective pattern sharing)
- EmbeddingProvider trait (domain-specific models)
- Advanced field dynamics
- Performance optimization
- Community examples and templates

**Deliverable**: Full SDK ecosystem ready for external developers

---

## Anti-Patterns

Lessons from LangChain, LangGraph, ADK, and CrewAI — what we explicitly avoid:

1. **Over-abstraction** (LangChain's #1 criticism): 5 core primitives, not 50. No abstraction without a demonstrated use case.
2. **Stringly-typed interfaces**: Use Rust enums for routing, state keys, and control flow. Illegal states are unrepresentable at compile time.
3. **Operator overloading magic** (LangChain LCEL): Explicit `.pipe()`, `.then()`, `.parallel()` — no surprise behavior.
4. **Moving-target APIs** (LangChain pre-1.0): Stabilize trait boundaries before shipping. Breaking changes only at major versions.
5. **Vendor lock-in for observability** (LangSmith): Use `tracing` crate — any subscriber works. No proprietary platform required.
6. **Dead code / speculative features** (YAGNI): Don't carry placeholder types for future capabilities. Add them when needed.
7. **Black-box failures** (CrewAI): Every operation emits structured events. No opaque agent failures.

---

## Open Questions

### Technical

1. **Substrate Scaling**: How does PulseDB perform at 100K+ experiences? Need benchmarks with real workloads.
2. **Agent Parallelism Limits**: How many concurrent agents can effectively share substrate before diminishing returns?
3. **Experience Graph Computation**: When should relationship inference run — at experience write time, background job, or on-demand?
4. **Context Window Management**: Optimal token budget allocation between insights, experiences, and activity awareness.

### Product

1. **Python API Design**: How Pythonic should the PyO3 bindings be? Thin wrapper vs idiomatic Python?
2. **Default Experience Extraction**: What heuristics work best for automatically extracting learnings from agent sessions?

### Future

1. **REFRAG-style Optimization**: Revisit when LLM providers add decoder-level APIs or self-hosting becomes practical. See `discussion.md`.
2. **PostgresSubstrate**: Build when cloud deployment is needed. The trait boundary is ready.

---

## Changelog

### v0.4.0 (Current)
- **Complete rewrite as SDK/framework specification** (was product specification for dev automation tool)
- PulseHive is now an SDK like LangChain/LangGraph/ADK — products are built ON it
- Product-specific content (sprint kanban, chat UI, agent terminals, specific agent types) moved to `PulseHive-DevStudio` repo
- Added 5 core primitives: HiveMind, Agent (Llm + Workflow), Tool, Lens, Experience
- Added WorkflowAgent variants (Sequential, Parallel, Loop) inspired by Google ADK
- Added observability system (HiveEvent enum, tracing crate integration)
- Added streaming (deploy returns Stream<Item = HiveEvent>)
- Added LlmProvider abstraction with Anthropic + OpenAI-compatible providers
- Added Human-in-the-Loop primitives (ApprovalHandler trait)
- Added crate structure (pulsehive-core, pulsehive-runtime, provider crates)
- Added deployment patterns section (SDK is a library, runs anywhere)
- Added SDK consumer API examples (simple + advanced)
- Added anti-patterns section (lessons from competitor research)
- Incorporated competitor research: LangChain, LangGraph, Google ADK, CrewAI
- All 6 architectural decisions reflected: REFRAG deferred, PulseDB-only backend, generic agentic loop, Rust/Tokio/Serde/tracing stack, library deployment, PulseDB Builtin embeddings

### v0.3.1
- Removed REFRAG implementation details (KVCacheState, EmbeddingMode, HybridMode)
- Changed MVP backend from PostgreSQL to PulseDB
- Removed Qdrant backend option

### v0.3.0
- Clarified SubstrateProvider trait ownership (PulseDB defines, PulseHive re-exports)
- Added intelligence algorithm documentation
- Added decay computation algorithm

### v0.2.0
- Initial spec with substrate architecture
- Experience Graph concepts introduced

---

*This spec is a living document. Updated as decisions are made and learnings emerge.*
