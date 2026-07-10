# PulseHive SDK API Specification

> **Version:** 0.4.0
> **Status:** Pre-implementation (API design finalized)
> **Last Updated:** March 2026

---

## 1. Overview

This document specifies the public API surface of the PulseHive SDK. PulseHive is a **Rust library crate** -- the "API" consists of public traits, structs, enums, and methods that product code imports and calls. There is no REST API, no RPC protocol, and no server.

Products interact with PulseHive by:
1. Constructing a `HiveMind` via the builder pattern
2. Defining agents, tools, and lenses
3. Calling `HiveMind::deploy()` to run agents
4. Consuming the returned `Stream<Item = HiveEvent>` for real-time events

---

## 2. Public Traits

### 2.1 Agent (informational -- framework-provided)

Agents are not implemented by products. Products provide `AgentDefinition` blueprints; the framework instantiates and runs agents internally. The agentic loop (perceive-think-act-record) is framework-owned.

### 2.2 Tool

Products implement `Tool` for domain-specific capabilities.

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name shown to the LLM for selection.
    fn name(&self) -> &str;

    /// Description the LLM uses to decide when to invoke this tool.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's parameters.
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool with the given parameters.
    async fn execute(
        &self,
        params: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, PulseHiveError>;

    /// Whether this tool requires human approval before execution.
    /// Default: false.
    fn requires_approval(&self) -> bool { false }
}
```

**Usage example:**

```rust
struct FileReader;

#[async_trait]
impl Tool for FileReader {
    fn name(&self) -> &str { "read_file" }

    fn description(&self) -> &str {
        "Read the contents of a file at the given path"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path to read" }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, PulseHiveError> {
        let path = params["path"].as_str()
            .ok_or(PulseHiveError::Validation("path required".into()))?;
        let content = tokio::fs::read_to_string(path).await
            .map_err(|e| PulseHiveError::Tool(e.to_string()))?;
        Ok(ToolResult::text(content))
    }
}
```

### 2.3 LlmProvider

Implemented by provider crates (`pulsehive-anthropic`, `pulsehive-openai`). Products can also implement custom providers.

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat completion request and return the full response.
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<LlmResponse, PulseHiveError>;

    /// Send a chat completion request and stream tokens.
    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk, PulseHiveError>> + Send>>,
               PulseHiveError>;
}
```

**LlmResponse** contains either a text response or a tool call request:

```rust
pub struct LlmResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: TokenUsage,
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

pub struct TokenUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
}
```

### 2.4 ApprovalHandler

Products implement this to define their human-in-the-loop UX.

```rust
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// Called when a tool with requires_approval() == true is invoked.
    /// The handler must return Approved, Denied, or Modified.
    async fn request_approval(
        &self,
        action: &PendingAction,
    ) -> Result<ApprovalResult, PulseHiveError>;
}

pub struct PendingAction {
    pub agent_id: AgentId,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub description: String,
}

pub enum ApprovalResult {
    Approved,
    Denied { reason: String },
    Modified { new_params: serde_json::Value },
}
```

**Usage example:**

```rust
struct CliApproval;

#[async_trait]
impl ApprovalHandler for CliApproval {
    async fn request_approval(
        &self,
        action: &PendingAction,
    ) -> Result<ApprovalResult, PulseHiveError> {
        println!("Agent {} wants to run {} with {:?}",
            action.agent_id, action.tool_name, action.params);
        println!("Approve? [y/n]");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        match input.trim() {
            "y" => Ok(ApprovalResult::Approved),
            _ => Ok(ApprovalResult::Denied { reason: "User rejected".into() }),
        }
    }
}
```

### 2.5 EmbeddingProvider (Phase 2+)

For products that need domain-specific embedding models instead of PulseDB's built-in `all-MiniLM-L6-v2`.

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text string.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, PulseHiveError>;

    /// Embed a batch of text strings.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, PulseHiveError>;

    /// Return the dimensionality of embeddings produced.
    fn dimensions(&self) -> usize;
}
```

### 2.6 SubstrateProvider (owned by PulseDB)

Defined in the `pulsehive-db` crate, re-exported by PulseHive. This is the boundary between PulseHive and storage.

```rust
#[async_trait]
pub trait SubstrateProvider: Send + Sync {
    // Experience operations
    async fn store_experience(&self, exp: NewExperience) -> Result<ExperienceId>;
    async fn get_experience(&self, id: ExperienceId) -> Result<Option<Experience>>;

    // Search operations
    async fn search_similar(
        &self,
        collective: CollectiveId,
        embedding: &[f32],
        k: usize,
    ) -> Result<Vec<(Experience, f32)>>;
    async fn get_recent(
        &self,
        collective: CollectiveId,
        limit: usize,
    ) -> Result<Vec<Experience>>;

    // Relation operations
    async fn store_relation(&self, rel: NewExperienceRelation) -> Result<RelationId>;
    async fn get_related(
        &self,
        exp_id: ExperienceId,
    ) -> Result<Vec<(Experience, ExperienceRelation)>>;

    // Insight operations
    async fn store_insight(&self, insight: NewDerivedInsight) -> Result<InsightId>;
    async fn get_insights(
        &self,
        collective: CollectiveId,
        embedding: &[f32],
        k: usize,
    ) -> Result<Vec<(DerivedInsight, f32)>>;

    // Activity operations
    async fn get_activities(&self, collective: CollectiveId) -> Result<Vec<Activity>>;

    // Context assembly (orchestrates search + insights + relations + activities)
    async fn get_context_candidates(
        &self,
        request: ContextRequest,
    ) -> Result<ContextCandidates>;

    // Real-time watch
    async fn watch(
        &self,
        collective: CollectiveId,
    ) -> Result<Pin<Box<dyn Stream<Item = WatchEvent> + Send>>>;
}
```

Products do not implement this trait directly. They use `PulseDBSubstrate` (the production implementation) or a mock for testing.

### 2.7 StreamingTool (Streaming Tools)

*Since v2.1.0.* An opt-in extension for **streaming tools** — long-running tools
that report live progress instead of a frozen wait. A tool that implements only
`Tool` is still fully supported (the agent loop wraps it so it emits `Started` →
`Completed` with no intermediate events); implement `StreamingTool` when a tool
is long-running and the consumer wants progress bars, partial results, or a log
stream.

`StreamingTool: Tool`, so every streaming tool is also a regular `Tool` and can
be stored as `Arc<dyn Tool>` and registered the same way. A tool exposes its
streaming path by overriding `Tool::as_streaming()` to return `Some(self)`; the
agent loop calls `execute_streaming()` on that path and forwards each pushed
`ToolProgress` as a `HiveEvent::ToolProgress`.

```rust
/// A progress event pushed by a streaming tool during execution.
///
/// Serializes to tagged JSON: `{"kind": "progress", "fraction": 0.5, ...}`.
pub enum ToolProgress {
    /// Emitted automatically by the loop before the tool body runs.
    Started { estimated_duration_ms: Option<u64> },
    /// Fractional progress in `0.0..=1.0`, with an optional label.
    Progress { fraction: f32, message: Option<String> },
    /// A partial result available before the tool completes.
    PartialResult { payload: serde_json::Value },
    /// A log line surfaced to the consumer's session timeline.
    Log { level: LogLevel, message: String },
    /// Emitted automatically by the loop after the tool body returns.
    Completed { duration_ms: u64 },
}

#[async_trait]
pub trait StreamingTool: Tool {
    /// Execute the tool, pushing `ToolProgress` over `progress_tx` as work
    /// proceeds. Implementations SHOULD send `Progress` / `PartialResult` /
    /// `Log`; they MUST NOT send `Started` / `Completed` (the loop emits those
    /// bookends). If the receiver is dropped, `send().await` errors — treat it
    /// as a soft signal, keep computing, and return the result anyway.
    async fn execute_streaming(
        &self,
        params: serde_json::Value,
        context: &ToolContext,
        progress_tx: tokio::sync::mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult, PulseHiveError>;
}
```

**Observing progress.** The agent loop forwards each `ToolProgress` as a
[`HiveEvent::ToolProgress`](#42-hiveevent) `{ agent_id, tool_name, progress }` on
the `HiveMind::deploy()` stream. Because the event bus is a lossy broadcast,
drain the stream **concurrently** with the run (not collect-after):

> **Delivery is best-effort on `deploy()`.** The loop emits the envelope in order
> (`Started` → intermediate progress → `Completed`), but `deploy()` returns a
> `tokio::broadcast` receiver that **drops the oldest events when a subscriber
> lags**. A slow consumer can therefore miss intermediate events **or even
> `Started`/`Completed`** — so do **not** treat `Completed` as a guaranteed
> terminal marker on this stream (e.g. don't hang a progress bar waiting for it).
> The ordered envelope is only guaranteed at the emitter/[`EventExporter`] boundary;
> on `deploy()` it is observability, not a reliable control signal.

```rust
struct ProgressStreamTool;

#[async_trait]
impl Tool for ProgressStreamTool {
    fn name(&self) -> &str { "progress_stream" }
    fn description(&self) -> &str { "Reports live fractional progress" }
    fn parameters(&self) -> serde_json::Value { json!({ "type": "object" }) }
    async fn execute(&self, _p: serde_json::Value, _c: &ToolContext) -> Result<ToolResult> {
        Ok(ToolResult::text("done")) // non-streaming fallback
    }
    fn as_streaming(&self) -> Option<&dyn StreamingTool> { Some(self) }
}

#[async_trait]
impl StreamingTool for ProgressStreamTool {
    async fn execute_streaming(
        &self,
        _params: serde_json::Value,
        _ctx: &ToolContext,
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Result<ToolResult> {
        for step in 1..=5 {
            tokio::time::sleep(Duration::from_millis(800)).await;
            let _ = progress_tx.send(ToolProgress::Progress {
                fraction: step as f32 / 5.0,
                message: Some(format!("step {step}/5")),
            }).await;
        }
        Ok(ToolResult::text("stream complete"))
    }
}

// Consumer side — drain deploy()'s stream concurrently with the run.
let mut stream = hive.deploy(vec![agent], vec![task]).await?;
while let Some(event) = stream.next().await {
    match event {
        HiveEvent::ToolProgress { tool_name, progress, .. } => {
            println!("[{tool_name}] {progress:?}");
        }
        HiveEvent::AgentCompleted { .. } => break,
        _ => {} // HiveEvent is #[non_exhaustive] (v2.1.0)
    }
}
```

A complete, runnable, hermetic version (no API key) lives at
`pulsehive-runtime/examples/streaming_tool.rs`.

---

## 3. Public Structs

### 3.1 HiveMind

The central orchestrator. Constructed via builder, used to deploy agents and manage the substrate.

```rust
pub struct HiveMind {
    // All fields are private. Interaction is through methods only.
}
```

**Key methods:**

```rust
impl HiveMind {
    /// Create a new HiveMind via the builder pattern.
    pub fn builder() -> HiveMindBuilder;

    /// Deploy agents to execute tasks. Returns an event stream.
    pub async fn deploy(
        &self,
        agents: Vec<AgentDefinition>,
        tasks: Vec<Task>,
    ) -> Result<Pin<Box<dyn Stream<Item = HiveEvent> + Send>>, PulseHiveError>;

    /// Record an experience with automatic relationship inference
    /// and insight synthesis.
    pub async fn record_experience(
        &self,
        experience: NewExperience,
    ) -> Result<ExperienceId, PulseHiveError>;

    /// Assemble optimized context for an agent through its lens.
    pub async fn get_context(
        &self,
        lens: &Lens,
        task: &Task,
        budget: ContextBudget,
    ) -> Result<AgentContext, PulseHiveError>;
}
```

### 3.2 HiveMindBuilder

Compile-time validated builder for constructing a `HiveMind`.

```rust
pub struct HiveMindBuilder { /* private */ }

impl HiveMindBuilder {
    /// Set the PulseDB file path. Opens or creates the database.
    pub fn substrate_path(self, path: &str) -> Self;

    /// Set a pre-configured SubstrateProvider (for testing or custom backends).
    pub fn substrate(self, provider: Box<dyn SubstrateProvider>) -> Self;

    /// Register an LLM provider under a name.
    /// Agents reference providers by name in their LlmConfig.
    pub fn llm_provider(self, name: &str, provider: Box<dyn LlmProvider>) -> Self;

    /// Set the approval handler for human-in-the-loop.
    pub fn approval_handler(self, handler: Box<dyn ApprovalHandler>) -> Self;

    /// Set a custom embedding provider (Phase 2+).
    pub fn embedding_provider(self, provider: Box<dyn EmbeddingProvider>) -> Self;

    /// Configure the RelationshipDetector.
    pub fn relationship_config(self, config: RelationshipDetectorConfig) -> Self;

    /// Configure the InsightSynthesizer.
    pub fn insight_config(self, config: InsightSynthesizerConfig) -> Self;

    /// Configure the ContextOptimizer.
    pub fn context_config(self, config: ContextOptimizerConfig) -> Self;

    /// Build the HiveMind. Fails if no substrate is configured.
    pub fn build(self) -> Result<HiveMind, PulseHiveError>;
}
```

**Usage example:**

```rust
use pulsehive::prelude::*;

let hive = HiveMind::builder()
    .substrate_path("./project.db")
    .llm_provider("anthropic", Box::new(AnthropicProvider::new(api_key)))
    .llm_provider("openai", Box::new(OpenAICompatibleProvider::new(openai_config)))
    .approval_handler(Box::new(CliApproval))
    .build()?;
```

### 3.3 AgentDefinition

Blueprint for creating an agent. Not a running agent instance.

```rust
pub struct AgentDefinition {
    pub name: String,
    pub kind: AgentKind,
}
```

### 3.4 LlmAgentConfig

Configuration for an LLM-powered agent:

```rust
pub struct LlmAgentConfig {
    pub system_prompt: String,
    pub tools: Vec<Box<dyn Tool>>,
    pub lens: Lens,
    pub llm_config: LlmConfig,
    pub experience_extractor: Option<Box<dyn ExperienceExtractor>>,
}
```

### 3.5 Lens

Perception filter controlling how an agent sees the substrate:

```rust
pub struct Lens {
    pub domain_focus: Vec<String>,
    pub type_weights: HashMap<ExperienceTypeTag, f32>,
    pub recency_curve: RecencyCurve,
    pub purpose_embedding: Vec<f32>,
    pub attention_budget: usize,
}

impl Lens {
    /// Convenience constructor with domain focus only.
    /// Uses default type weights, Exponential recency, and budget of 20.
    pub fn new(domains: Vec<&str>) -> Self;

    /// Full constructor with all parameters.
    pub fn with_config(
        domains: Vec<String>,
        type_weights: HashMap<ExperienceTypeTag, f32>,
        recency_curve: RecencyCurve,
        purpose_embedding: Vec<f32>,
        attention_budget: usize,
    ) -> Self;
}
```

### 3.6 LlmConfig

Model selection and inference parameters:

```rust
pub struct LlmConfig {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl LlmConfig {
    /// Create a config with provider name and model identifier.
    pub fn new(provider: &str, model: &str) -> Self;
}
```

The `provider` field matches the name passed to `HiveMindBuilder::llm_provider()`.

### 3.7 Task

A unit of work for an agent:

```rust
pub struct Task {
    pub description: String,
    pub collective_id: CollectiveId,
    pub metadata: Option<serde_json::Value>,
}

impl Task {
    pub fn new(description: &str) -> Self;
    pub fn with_collective(description: &str, collective: CollectiveId) -> Self;
}
```

### 3.8 AgentContext

Assembled context returned by `HiveMind::get_context()`:

```rust
pub struct AgentContext {
    pub experiences: Vec<RankedExperience>,
    pub insights: Vec<DerivedInsight>,
    pub activities: Vec<Activity>,
    pub total_tokens: usize,
}

pub struct RankedExperience {
    pub experience: Experience,
    pub relevance_score: f32,    // After lens re-ranking and decay
    pub original_similarity: f32, // Raw cosine similarity from HNSW
}
```

### 3.9 ToolContext

Provided to tools during execution:

```rust
pub struct ToolContext {
    pub agent_id: AgentId,
    pub collective_id: CollectiveId,
    pub substrate: Arc<dyn SubstrateProvider>,
    pub event_emitter: EventEmitter,
}
```

Tools can use `substrate` to query or write experiences directly. This enables tools that interact with the shared consciousness (e.g., a "recall" tool that searches substrate on behalf of the agent).

### 3.10 ToolResult

Output from tool execution:

```rust
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub metadata: Option<serde_json::Value>,
}

impl ToolResult {
    pub fn text(content: impl Into<String>) -> Self;
    pub fn error(message: impl Into<String>) -> Self;
    pub fn with_metadata(self, metadata: serde_json::Value) -> Self;
}
```

### 3.11 ContextBudget

Controls context assembly limits:

```rust
pub struct ContextBudget {
    pub max_tokens: usize,
    pub max_experiences: usize,
    pub max_insights: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_tokens: 4096,
            max_experiences: 20,
            max_insights: 5,
        }
    }
}
```

---

## 4. Public Enums

### 4.1 AgentKind

```rust
pub enum AgentKind {
    /// LLM-powered agent with tools and lens-based perception.
    Llm(LlmAgentConfig),

    /// Runs sub-agents sequentially. Each sees previous agents' experiences.
    Sequential(Vec<AgentDefinition>),

    /// Runs sub-agents in parallel. All share substrate in real-time.
    Parallel(Vec<AgentDefinition>),

    /// Repeats sub-agent until max_iterations or completion.
    Loop {
        agent: Box<AgentDefinition>,
        max_iterations: usize,
    },
}
```

### 4.2 HiveEvent

As of **v2.1.0**, `HiveEvent` is `#[non_exhaustive]` — new variants can be added
in a minor release, so external code that matches on it exhaustively **must**
include a `_ => {}` catch-all arm.

```rust
#[non_exhaustive] // v2.1.0 — external exhaustive matches need a `_ => {}` arm
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

    // Streaming tool progress (v2.1.0) — one per `ToolProgress` pushed by a
    // `StreamingTool`, bracketed by loop-generated Started/Completed bookends.
    ToolProgress { agent_id: AgentId, tool_name: String, progress: ToolProgress },

    // Substrate operations
    ExperienceRecorded { experience_id: ExperienceId, agent_id: AgentId },
    RelationshipInferred { relation_id: RelationId },
    InsightGenerated { insight_id: InsightId, source_count: usize },

    // Perception
    SubstratePerceived { agent_id: AgentId, experience_count: usize, insight_count: usize },
}
```

### 4.3 RecencyCurve

```rust
pub enum RecencyCurve {
    /// Recent experiences weighted heavily, older ones decay exponentially.
    Exponential { half_life_hours: f32 },

    /// All experiences weighted equally regardless of age.
    Uniform,
}
```

### 4.4 PulseHiveError

```rust
pub enum PulseHiveError {
    /// Storage layer errors (from PulseDB).
    Substrate(pulsedb::PulseDBError),

    /// LLM provider errors (network, auth, rate limits).
    Llm { provider: String, message: String },

    /// Tool execution errors.
    Tool(String),

    /// Input validation errors.
    Validation(String),

    /// Configuration errors (missing provider, bad builder state).
    Config(String),

    /// Agent execution errors.
    Agent { agent_id: AgentId, message: String },

    /// Approval was denied.
    ApprovalDenied { tool_name: String, reason: String },
}
```

### 4.5 AgentOutcome

```rust
pub enum AgentOutcome {
    /// Agent completed successfully.
    Success {
        response: String,
        experiences_recorded: usize,
    },

    /// Agent failed.
    Failure {
        error: PulseHiveError,
    },

    /// Agent was cancelled (e.g., by approval denial or timeout).
    Cancelled {
        reason: String,
    },
}
```

---

## 5. Key Method Details

### 5.1 HiveMind::builder()

Creates a `HiveMindBuilder`. At minimum, a substrate must be configured before `build()` is called. LLM providers are required if any `AgentKind::Llm` agents will be deployed.

```rust
let hive = HiveMind::builder()
    .substrate_path("./data.db")
    .llm_provider("anthropic", Box::new(AnthropicProvider::new(key)))
    .build()?;
```

### 5.2 HiveMind::deploy()

The primary entry point for running agents. Accepts agent definitions and tasks, spawns agent execution on the Tokio runtime, and returns a stream of events.

```rust
let researcher = AgentDefinition {
    name: "Researcher".into(),
    kind: AgentKind::Llm(LlmAgentConfig {
        system_prompt: "You are a research analyst.".into(),
        tools: vec![Box::new(WebSearch)],
        lens: Lens::new(vec!["research"]),
        llm_config: LlmConfig::new("anthropic", "claude-sonnet-4-6"),
        experience_extractor: None,
    }),
};

let task = Task::with_collective(
    "Research quantum computing advances",
    collective_id,
);

let mut stream = hive.deploy(vec![researcher], vec![task]).await?;

while let Some(event) = stream.next().await {
    match event {
        HiveEvent::LlmTokenStreamed { token, .. } => print!("{}", token),
        HiveEvent::AgentCompleted { outcome, .. } => {
            match outcome {
                AgentOutcome::Success { response, .. } => {
                    println!("\nResult: {}", response);
                }
                AgentOutcome::Failure { error } => {
                    eprintln!("Error: {}", error);
                }
                _ => {}
            }
        }
        _ => {}
    }
}
```

### 5.3 HiveMind::record_experience()

Records an experience and triggers intelligence processing. This is the method that makes PulseHive more than a wrapper around PulseDB -- it adds relationship detection and insight synthesis.

```rust
let exp_id = hive.record_experience(NewExperience {
    collective_id,
    content: "Auth requires httpOnly cookies with SameSite=Strict".into(),
    experience_type: ExperienceType::ArchitecturalDecision {
        decision: "Use httpOnly cookies for auth".into(),
        rationale: "Prevents XSS token theft".into(),
    },
    importance: 0.8,
    confidence: 0.9,
    domain: vec!["security".into(), "authentication".into()],
    source_agent: "architect".into(),
    ..Default::default()
}).await?;
```

**Internal flow:**
1. Stores the experience in PulseDB (with embedding computed by PulseDB in Builtin mode).
2. Runs `RelationshipDetector::infer_relations()` -- finds semantically similar experiences and classifies relationships.
3. Stores any inferred relations via `store_relation()`.
4. Checks if `InsightSynthesizer::should_synthesize()` -- if a cluster density threshold is reached, synthesizes and stores derived insights.
5. Returns the `ExperienceId`.

### 5.4 HiveMind::get_context()

Assembles optimized context for an agent through its lens. Products can call this directly to inspect what an agent would perceive, or the agentic loop calls it automatically during the perceive phase.

```rust
let lens = Lens::new(vec!["security", "authentication"]);
let budget = ContextBudget {
    max_tokens: 2048,
    max_experiences: 10,
    max_insights: 3,
};

let context = hive.get_context(&lens, &task, budget).await?;

for ranked in &context.experiences {
    println!("{:.2} | {}", ranked.relevance_score, ranked.experience.content);
}
```

---

## 6. Provider Crate APIs

### 6.1 pulsehive-anthropic

```rust
pub struct AnthropicProvider { /* private */ }

impl AnthropicProvider {
    /// Create a provider with an API key.
    pub fn new(api_key: impl Into<String>) -> Self;

    /// Create with custom base URL (for proxies).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self;
}

// Implements LlmProvider
```

### 6.2 pulsehive-openai

```rust
pub struct OpenAICompatibleProvider { /* private */ }

pub struct OpenAIConfig {
    pub api_key: String,
    pub base_url: String,       // Default: "https://api.openai.com/v1"
    pub model: String,
    pub organization: Option<String>,
}

impl OpenAICompatibleProvider {
    /// Create a provider with full configuration.
    pub fn new(config: OpenAIConfig) -> Self;
}

// Implements LlmProvider
```

**Provider examples:**

```rust
// OpenAI
let openai = OpenAICompatibleProvider::new(OpenAIConfig {
    api_key: env::var("OPENAI_API_KEY")?,
    base_url: "https://api.openai.com/v1".into(),
    model: "gpt-4o".into(),
    organization: None,
});

// GLM-5
let glm = OpenAICompatibleProvider::new(OpenAIConfig {
    api_key: env::var("GLM_API_KEY")?,
    base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
    model: "glm-5".into(),
    organization: None,
});

// Ollama (local)
let ollama = OpenAICompatibleProvider::new(OpenAIConfig {
    api_key: "not-needed".into(),
    base_url: "http://localhost:11434/v1".into(),
    model: "llama3.1".into(),
    organization: None,
});
```

---

## 7. Prelude Module

The `pulsehive::prelude` module re-exports the most commonly used types for convenience:

```rust
pub use pulsehive::prelude::*;

// Includes:
// - HiveMind, HiveMindBuilder
// - AgentDefinition, AgentKind, LlmAgentConfig
// - Tool, ToolContext, ToolResult
// - Lens, RecencyCurve
// - LlmProvider, LlmConfig
// - HiveEvent, AgentOutcome
// - ApprovalHandler, ApprovalResult, PendingAction
// - Task, AgentContext, ContextBudget
// - PulseHiveError
// - Re-exports from PulseDB: Experience, NewExperience, ExperienceType,
//   ExperienceId, CollectiveId, SubstrateProvider, etc.
```

---

## 8. Complete Usage Example

A multi-agent workflow with parallel analysis and sequential synthesis:

```rust
use pulsehive::prelude::*;
use pulsehive_anthropic::AnthropicProvider;

#[tokio::main]
async fn main() -> Result<(), PulseHiveError> {
    // 1. Build the HiveMind
    let hive = HiveMind::builder()
        .substrate_path("./research.db")
        .llm_provider("claude", Box::new(
            AnthropicProvider::new(std::env::var("ANTHROPIC_API_KEY").unwrap())
        ))
        .build()?;

    // 2. Define tools
    let search_tool = Box::new(WebSearchTool);
    let analyze_tool = Box::new(DataAnalysisTool);
    let write_tool = Box::new(DocumentWriterTool);

    // 3. Define a multi-agent workflow
    let pipeline = AgentDefinition {
        name: "Research Pipeline".into(),
        kind: AgentKind::Sequential(vec![
            // Phase 1: Parallel research
            AgentDefinition {
                name: "Research Phase".into(),
                kind: AgentKind::Parallel(vec![
                    AgentDefinition {
                        name: "Web Researcher".into(),
                        kind: AgentKind::Llm(LlmAgentConfig {
                            system_prompt: "Search the web for recent papers.".into(),
                            tools: vec![search_tool.clone()],
                            lens: Lens::new(vec!["research", "papers"]),
                            llm_config: LlmConfig::new("claude", "claude-sonnet-4-6"),
                            experience_extractor: None,
                        }),
                    },
                    AgentDefinition {
                        name: "Data Analyst".into(),
                        kind: AgentKind::Llm(LlmAgentConfig {
                            system_prompt: "Analyze provided datasets.".into(),
                            tools: vec![analyze_tool],
                            lens: Lens::new(vec!["data", "statistics"]),
                            llm_config: LlmConfig::new("claude", "claude-sonnet-4-6"),
                            experience_extractor: None,
                        }),
                    },
                ]),
            },
            // Phase 2: Synthesis (automatically sees Phase 1 experiences)
            AgentDefinition {
                name: "Synthesizer".into(),
                kind: AgentKind::Llm(LlmAgentConfig {
                    system_prompt: "Synthesize research findings into a report.".into(),
                    tools: vec![write_tool],
                    lens: Lens::new(vec!["research", "data", "synthesis"]),
                    llm_config: LlmConfig::new("claude", "claude-sonnet-4-6"),
                    experience_extractor: None,
                }),
            },
        ]),
    };

    // 4. Deploy and consume events
    let collective_id = CollectiveId::new();
    let task = Task::with_collective("Research quantum error correction", collective_id);

    let mut stream = hive.deploy(vec![pipeline], vec![task]).await?;

    while let Some(event) = stream.next().await {
        match event {
            HiveEvent::AgentStarted { name, .. } => {
                println!("[STARTED] {}", name);
            }
            HiveEvent::LlmTokenStreamed { agent_id, token, .. } => {
                print!("{}", token);
            }
            HiveEvent::ExperienceRecorded { experience_id, agent_id } => {
                println!("\n[LEARNED] Agent {} recorded {}", agent_id, experience_id);
            }
            HiveEvent::InsightGenerated { insight_id, source_count, .. } => {
                println!("[INSIGHT] {} from {} experiences", insight_id, source_count);
            }
            HiveEvent::AgentCompleted { name, outcome, .. } => {
                match outcome {
                    AgentOutcome::Success { experiences_recorded, .. } => {
                        println!("[DONE] {} ({} experiences)", name, experiences_recorded);
                    }
                    AgentOutcome::Failure { error } => {
                        eprintln!("[FAIL] {}: {}", name, error);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    Ok(())
}
```

---

## 9. Error Handling Patterns

All public methods return `Result<T, PulseHiveError>`. Products should handle errors based on the variant:

```rust
match hive.deploy(agents, tasks).await {
    Ok(stream) => { /* consume events */ }
    Err(PulseHiveError::Config(msg)) => {
        // Builder misconfiguration -- fix at startup
        panic!("Configuration error: {}", msg);
    }
    Err(PulseHiveError::Substrate(db_err)) => {
        // Database error -- file permissions, corruption
        eprintln!("Storage error: {}", db_err);
    }
    Err(PulseHiveError::Llm { provider, message }) => {
        // LLM API error -- network, auth, rate limit
        eprintln!("LLM error ({}): {}", provider, message);
    }
    Err(e) => {
        eprintln!("Unexpected error: {}", e);
    }
}
```

---

*This document specifies the public API as of SPEC v0.4.0. Method signatures and type details will be validated and potentially refined during Phase 1 implementation.*
