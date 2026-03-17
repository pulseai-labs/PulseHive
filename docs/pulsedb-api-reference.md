# PulseDB API Reference for PulseHive

> **Crate**: `pulsehive-db` (import as `use pulsedb::...`)
> **Version**: 0.1.1
> **docs.rs**: https://docs.rs/pulsehive-db

This is a concise reference of PulseDB's public API surface relevant to PulseHive development. For full documentation, see docs.rs.

---

## SubstrateProvider Trait

The async interface PulseHive uses to interact with PulseDB. HiveMind holds `Box<dyn SubstrateProvider>`.

```rust
#[async_trait]
pub trait SubstrateProvider: Send + Sync {
    // Experience operations
    async fn store_experience(&self, exp: NewExperience) -> Result<ExperienceId>;
    async fn get_experience(&self, id: ExperienceId) -> Result<Option<Experience>>;

    // Search operations
    async fn search_similar(&self, collective: CollectiveId, embedding: &[f32], k: usize) -> Result<Vec<(Experience, f32)>>;
    async fn get_recent(&self, collective: CollectiveId, limit: usize) -> Result<Vec<Experience>>;

    // Relation operations
    async fn store_relation(&self, rel: NewExperienceRelation) -> Result<RelationId>;
    async fn get_related(&self, exp_id: ExperienceId) -> Result<Vec<(Experience, ExperienceRelation)>>;

    // Insight operations
    async fn store_insight(&self, insight: NewDerivedInsight) -> Result<InsightId>;
    async fn get_insights(&self, collective: CollectiveId, embedding: &[f32], k: usize) -> Result<Vec<(DerivedInsight, f32)>>;

    // Activity operations
    async fn get_activities(&self, collective: CollectiveId) -> Result<Vec<Activity>>;

    // Context assembly (orchestrates all above)
    async fn get_context_candidates(&self, request: ContextRequest) -> Result<ContextCandidates>;

    // Real-time watch
    async fn watch(&self, collective: CollectiveId) -> Result<Pin<Box<dyn Stream<Item = WatchEvent> + Send>>>;
}
```

### PulseDBSubstrate (production implementation)

```rust
use std::sync::Arc;
let db = Arc::new(PulseDB::open("path.db", Config::default())?);
let substrate = PulseDBSubstrate::new(db);
let provider: Box<dyn SubstrateProvider> = Box::new(substrate);
```

---

## Core Types

### Experience

```rust
pub struct Experience {
    pub id: ExperienceId,
    pub collective_id: CollectiveId,
    pub content: String,
    pub experience_type: ExperienceType,
    pub embedding: Vec<f32>,          // 384-dimensional by default
    pub importance: f32,              // 0.0 - 1.0
    pub confidence: f32,              // 0.0 - 1.0
    pub domain: Vec<String>,
    pub source_agent: AgentId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub archived: bool,
    pub applications_count: u32,      // reinforcement proxy
}
```

### NewExperience (input for store_experience)

```rust
pub struct NewExperience {
    pub collective_id: CollectiveId,   // required
    pub content: String,               // required, non-empty, max 100KB
    pub experience_type: ExperienceType, // default: Generic
    pub embedding: Option<Vec<f32>>,   // required for External provider
    pub importance: f32,               // default: 0.5, range [0.0, 1.0]
    pub confidence: f32,               // default: 0.5, range [0.0, 1.0]
    pub domain: Vec<String>,           // max 50 tags, 100 chars each
    pub source_agent: AgentId,         // default: "anonymous"
    pub source_task: Option<TaskId>,
    pub related_files: Vec<String>,
}
```

### ExperienceType (9 variants)

```rust
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

### SearchResult & SearchFilter

```rust
pub struct SearchResult {
    pub experience: Experience,
    pub similarity: f32,  // 1.0 = identical, typically [0.0, 1.0]
}

pub struct SearchFilter {
    pub domains: Option<Vec<String>>,
    pub experience_types: Option<Vec<ExperienceType>>,
    pub min_importance: Option<f32>,
    pub min_confidence: Option<f32>,
    pub since: Option<Timestamp>,
    pub exclude_archived: bool,  // default: true
}
```

### ContextRequest & ContextCandidates

```rust
pub struct ContextRequest {
    pub collective_id: CollectiveId,
    pub query_embedding: Vec<f32>,
    pub max_similar: usize,           // default: 20
    pub max_recent: usize,            // default: 10
    pub include_insights: bool,       // default: true
    pub include_relations: bool,      // default: true
    pub include_active_agents: bool,  // default: true
    pub filter: SearchFilter,
}

pub struct ContextCandidates {
    pub similar_experiences: Vec<SearchResult>,  // sorted by similarity DESC
    pub recent_experiences: Vec<Experience>,     // sorted by timestamp DESC
    pub insights: Vec<DerivedInsight>,
    pub relations: Vec<ExperienceRelation>,
    pub active_agents: Vec<Activity>,
}
```

---

## Relation Types

```rust
pub enum RelationType {
    Supports,     // source reinforces target
    Contradicts,  // source opposes target
    Elaborates,   // source adds detail to target
    Supersedes,   // source replaces target
    Implies,      // source suggests target
    RelatedTo,    // general association
}

pub enum RelationDirection {
    Outgoing,  // from this experience
    Incoming,  // to this experience
    Both,
}

pub struct NewExperienceRelation {
    pub source_id: ExperienceId,
    pub target_id: ExperienceId,
    pub relation_type: RelationType,
    pub strength: f32,           // [0.0, 1.0]
    pub metadata: Option<String>,
}
```

---

## Watch Types

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

---

## ID Types

All ID types use UUID v7 (time-ordered):

| Type | Purpose |
|------|---------|
| `CollectiveId` | Isolates projects/namespaces |
| `ExperienceId` | Identifies a single experience |
| `InsightId` | Identifies a derived insight |
| `RelationId` | Identifies an experience relation |
| `AgentId` | String identifier for agents |
| `TaskId` | String identifier for tasks |
| `UserId` | String identifier for users |

---

## Configuration

```rust
let config = Config::default();
// Default: External embeddings (384d), no ONNX, standard HNSW params
// Override: Config { embedding_provider: EmbeddingProvider::Builtin, ... }
```

Key config options: `EmbeddingProvider` (External/Builtin), `EmbeddingDimension` (384d default), `HnswConfig` (ef_construction, ef_search, m), `WatchConfig` (in_process, poll_interval_ms, buffer_size).

---

## Performance (measured at 1K experiences)

| Operation | Latency |
|-----------|---------|
| `store_experience` | 5.5 ms |
| `search_similar` (k=20) | 95 us |
| `get_context_candidates` | 189 us |
| `get_experience` by ID | 1.3 us |

Targets at 100K: record < 10ms, search < 50ms, context < 100ms.

---

## Error Types

```rust
pub enum PulseDBError {
    Storage(StorageError),       // IO, corruption, transactions
    Validation(ValidationError), // Input validation failures
    NotFound(NotFoundError),     // Entity not found
    Config { reason: String },   // Configuration errors
    Io(std::io::Error),          // General IO
}
```
