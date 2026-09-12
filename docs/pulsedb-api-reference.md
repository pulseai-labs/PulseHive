# PulseDB API Reference for PulseHive

> **Crate**: `pulsehive-db` (import as `use pulsedb::...`)
> **Version**: 0.7.0 (the version PulseHive pins since the r1.s4 upgrade)
> **docs.rs**: https://docs.rs/pulsehive-db

This is a concise reference of PulseDB's public API surface relevant to PulseHive development, verified against `pulsehive-db` 0.7.0. For full documentation, see docs.rs. Shapes are the 0.7 construction shapes — the value types PulseHive re-exports changed in the 0.5 → 0.7 move (see [ADR-012](adr/012-pulsedb-0-7-migration.md) and the 2.1.0 changelog); there is no shim.

---

## SubstrateProvider Trait

The async interface PulseHive uses to interact with PulseDB. `HiveMind` holds `Arc<dyn SubstrateProvider>` (built from `PulseDBSubstrate`).

```rust
#[async_trait]
pub trait SubstrateProvider: Send + Sync {
    // Experience operations
    async fn store_experience(&self, exp: NewExperience) -> Result<ExperienceId, PulseDBError>;
    async fn get_experience(&self, id: ExperienceId) -> Result<Option<Experience>, PulseDBError>;
    async fn reinforce_experience(&self, id: ExperienceId) -> Result<u32, PulseDBError>; // defaulted: Err(Internal)
    async fn energy(&self, id: ExperienceId) -> Result<f32, PulseDBError>;              // defaulted: Err(Internal)

    // Search operations
    async fn search_similar(&self, collective: CollectiveId, embedding: &[f32], k: usize) -> Result<Vec<(Experience, f32)>, PulseDBError>;
    async fn get_recent(&self, collective: CollectiveId, limit: usize) -> Result<Vec<Experience>, PulseDBError>;

    // Relation operations
    async fn store_relation(&self, rel: NewExperienceRelation) -> Result<RelationId, PulseDBError>;
    async fn get_related(&self, exp_id: ExperienceId) -> Result<Vec<(Experience, ExperienceRelation)>, PulseDBError>; // both directions

    // Insight operations
    async fn store_insight(&self, insight: NewDerivedInsight) -> Result<InsightId, PulseDBError>;
    async fn get_insights(&self, collective: CollectiveId, embedding: &[f32], k: usize) -> Result<Vec<(DerivedInsight, f32)>, PulseDBError>;

    // Activity operations
    async fn get_activities(&self, collective: CollectiveId) -> Result<Vec<Activity>, PulseDBError>;

    // Context assembly (orchestrates all above)
    async fn get_context_candidates(&self, request: ContextRequest) -> Result<ContextCandidates, PulseDBError>;

    // Real-time watch
    async fn watch(&self, collective: CollectiveId) -> Result<Pin<Box<dyn Stream<Item = WatchEvent> + Send>>, PulseDBError>;

    // Collective lifecycle
    async fn create_collective(&self, name: &str) -> Result<CollectiveId, PulseDBError>;          // fails on duplicate name
    async fn get_or_create_collective(&self, name: &str) -> Result<CollectiveId, PulseDBError>;   // idempotent, recommended
    async fn list_collectives(&self) -> Result<Vec<Collective>, PulseDBError>;

    // Pagination / maintenance (defaulted: empty vecs unless overridden)
    async fn list_experiences(&self, collective: CollectiveId, limit: usize, offset: usize) -> Result<Vec<Experience>, PulseDBError>;
    async fn list_relations(&self, collective: CollectiveId, limit: usize, offset: usize) -> Result<Vec<ExperienceRelation>, PulseDBError>;
    async fn list_insights(&self, collective: CollectiveId, limit: usize, offset: usize) -> Result<Vec<DerivedInsight>, PulseDBError>;
    async fn list_cold_experiences(&self, collective: CollectiveId, below: f32, limit: usize) -> Result<Vec<(ExperienceId, f32)>, PulseDBError>; // defaulted: Err(Internal)
}
```

`PulseDBSubstrate` implements every method over `Arc<PulseDB>` (sync storage ops behind `spawn_blocking`).

### PulseDBSubstrate (production implementation)

```rust
// What HiveMindBuilder::build() does for a substrate_path(...) hive without
// an embedding provider (Builtin mode):
let db = PulseDB::open(&path, Config::with_builtin_embeddings())?;
let substrate = PulseDBSubstrate::from_db(db);           // owns the PulseDB
// or: PulseDBSubstrate::new(Arc::new(db))               // shares an existing one
```

---

## Core Types

### Experience

```rust
pub struct Experience {
    pub id: ExperienceId,
    pub collective_id: CollectiveId,
    pub content: String,
    pub embedding: Vec<f32>,                 // 384-dimensional in Builtin mode
    pub experience_type: ExperienceType,
    pub importance: f32,                     // 0.0 - 1.0
    pub confidence: f32,                     // 0.0 - 1.0
    pub applications: BTreeMap<InstanceId, u32>,  // per-instance counters; total via .applications()
    pub domain: Vec<String>,                 // categorical tags, max 50
    pub tags: BTreeMap<String, String>,      // key-value tags for structured filtering (new in 0.7)
    pub related_files: Vec<String>,
    pub source_agent: AgentId,
    pub source_task: Option<TaskId>,
    pub timestamp: Timestamp,                // set by the storage layer
    pub last_reinforced: Timestamp,
    pub archived: bool,                      // soft delete; excluded from search
}
```

### NewExperience (input for store_experience)

```rust
pub struct NewExperience {
    pub collective_id: CollectiveId,     // required
    pub content: String,                 // required, non-empty, max 100KB
    pub experience_type: ExperienceType, // default: Generic { category: None }
    pub embedding: Option<Vec<f32>>,     // Some(_) only in External mode (see errors below)
    pub importance: f32,                 // default: 0.5, range [0.0, 1.0]
    pub confidence: f32,                 // default: 0.5, range [0.0, 1.0]
    pub domain: Vec<String>,             // max 50 tags, 100 chars each
    pub tags: BTreeMap<String, String>,  // key-value tags; Default::default() for none (new in 0.7)
    pub related_files: Vec<String>,
    pub source_agent: AgentId,           // default: "anonymous"
    pub source_task: Option<TaskId>,
}
```

`id`, `timestamp`, `last_reinforced`, `applications`, and `archived` are set by the storage layer; they are not part of the input struct.

> **Construction note.** PulseHive call sites construct `NewExperience` with explicit
> struct literals listing all twelve fields, using `tags: Default::default()` when no
> structured tags apply (a `Default` impl also exists upstream). Passing
> `embedding: Some(vec)` while the store runs a managed embedder (Builtin mode) is
> refused with `PulseDBError::ManagedEmbedderPresent`.

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

`Severity` is `Low | Medium | High | Critical`. The variant set is unchanged from earlier
versions; only the surrounding record shapes moved.

### SearchResult & SearchFilter

```rust
pub struct SearchResult {
    pub experience: Experience,
    pub similarity: f32,   // raw cosine; typically [0.0, 1.0], theoretical range [-1.0, 1.0]
}

pub struct SearchFilter {
    pub domains: Option<Vec<String>>,              // None = no filter; Some(vec![]) matches nothing
    pub tags_all: Option<BTreeMap<String, String>>, // exact-match subset on key-value tags (new in 0.7)
    pub experience_types: Option<Vec<ExperienceType>>, // matches on the type discriminant only
    pub min_importance: Option<f32>,
    pub min_confidence: Option<f32>,
    pub since: Option<Timestamp>,
    pub exclude_archived: bool,                    // default: true
}
```

### ContextRequest & ContextCandidates

```rust
pub struct ContextRequest {
    pub collective_id: CollectiveId,
    pub query_embedding: Vec<f32>,      // must match the collective's dimension
    pub max_similar: usize,             // default: 20
    pub max_recent: usize,              // default: 10
    pub include_insights: bool,         // default: true
    pub include_relations: bool,        // default: true
    pub include_active_agents: bool,    // default: true
    pub filter: SearchFilter,
}

pub struct ContextCandidates {
    pub similar_experiences: Vec<SearchResult>,  // similarity-descending under legacy recall
    pub recent_experiences: Vec<Experience>,     // timestamp-descending
    pub insights: Vec<DerivedInsight>,
    pub relations: Vec<ExperienceRelation>,      // deduplicated by RelationId
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

pub struct ExperienceRelation {  // the stored record
    pub id: RelationId,
    pub source_id: ExperienceId,
    pub target_id: ExperienceId,
    pub relation_type: RelationType,
    pub strength: f32,
    pub metadata: Option<String>,  // JSON, max 10KB
}
```

PulseHive's `RelationshipDetector` classifies pairs into `Supports` (Difficulty ↔ Solution),
`Supersedes` (ErrorPattern ↔ ErrorPattern), `Implies` (ArchitecturalDecision ↔ TechInsight),
and `RelatedTo` otherwise.

---

## Watch Types

```rust
pub struct WatchEvent {
    pub experience_id: ExperienceId,
    pub collective_id: CollectiveId,
    pub event_type: WatchEventType,
    pub timestamp: Timestamp,
    pub experience: Option<Experience>,  // enriched payload for in-process Created/Updated events (new in 0.7)
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

`HiveMind::deploy` subscribes via `watch(collective)` in a background task and forwards each
event as `HiveEvent::WatchNotification`; a failed subscription degrades gracefully (agents
still run).

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
| `InstanceId` | Keys the per-instance `applications` counters on `Experience` |

---

## Configuration & Embedding Identity

```rust
// External mode (default): caller supplies embeddings via NewExperience.embedding
let config = Config::new();

// Builtin mode: PulseDB embeds internally with the bundled all-MiniLM-L6-v2 (384d)
let config = Config::with_builtin_embeddings();

let db = PulseDB::open(path, config)?;              // first writable open migrates a 0.5.x database
let identity = db.provider_identity()?;             // ProviderIdentity { provider, model_id }
```

0.7 stamps the embedding provider identity (`provider`, `model_id`) into the store on first
write. Reopening a Builtin-mode store with the same bundled model succeeds and reports the
normalized `builtin-onnx/onnx-<sha256>` identity; a different managed identity is refused
with `PulseDBError::EmbeddingProviderMismatch`. PulseHive chooses the mode automatically:
`HiveMindBuilder::embedding_provider(...)` sets External mode (PulseHive precomputes vectors),
otherwise `build()` opens with `Config::with_builtin_embeddings()`.

Migration and rollback behavior for existing 0.5.x collectives — first writable open, the
`.pre-substrate.bak` / `.pre-v4.bak` sidecars, and the restore-before-downgrade order — is
specified in [ADR-012](adr/012-pulsedb-0-7-migration.md) and the deployment runbook.

---

## Error Types

```rust
pub enum PulseDBError {
    ReadOnly,                                  // write attempted on a read-only open
    Storage(StorageError),                     // IO, corruption, transactions
    Validation(ValidationError),               // input validation failures
    Config { reason: String },                 // configuration errors
    NotFound(NotFoundError),                   // entity not found
    Io(std::io::Error),                        // general IO
    Embedding(String),                         // embedding generation/validation
    EmbeddingProviderMismatch { persisted: ProviderIdentity, requested: ProviderIdentity },
    ManagedEmbedderPresent { /* .. */ },       // embedding: Some(vec) against a managed embedder
    Vector(String),                            // vector index errors
    Watch(String),                             // watch system errors
    Internal(String),                          // unsupported operation on this provider
    Sync(SyncError),                           // sync-peer errors
}
```

PulseHive propagates these unchanged as `PulseHiveError::Substrate` (`#[from] PulseDBError`);
the migration and identity-refusal errors above reach callers through that path.
