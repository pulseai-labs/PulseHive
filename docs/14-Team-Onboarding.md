# PulseHive SDK — Contributor & Developer Onboarding

> **Document ID:** OPS-PH-014
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** SPEC v0.4.0

---

## 1. Welcome

PulseHive is a Rust SDK for building multi-agent AI systems with shared consciousness. This document gets you from zero to productive — whether you are contributing to PulseHive itself or building a product on top of it.

**What PulseHive is:** A library crate (like LangChain or LangGraph, but in Rust) that products embed as a dependency. It is not a server, not a web app, not a platform.

**What PulseDB is:** The storage substrate. An embedded Rust database that PulseHive uses for experience storage, vector search, knowledge graphs, and real-time watch. PulseDB stores and retrieves. PulseHive thinks.

---

## 2. Prerequisites

### Required

- **Rust stable toolchain** (1.83+). Install via [rustup](https://rustup.rs/).
- **Git** for version control.
- **Basic async Rust knowledge**: understanding of `async/await`, `tokio::spawn`, `Stream`, and why blocking in async context is bad.
- **An LLM API key** for running examples: Anthropic (Claude) or any OpenAI-compatible provider (OpenAI, GLM, Ollama, etc.).

### Helpful but Not Required

- Familiarity with the `tracing` crate (PulseHive's observability layer).
- Understanding of vector embeddings and approximate nearest neighbor search.
- Experience with multi-agent AI concepts (if you have used LangGraph, CrewAI, or Google ADK, many concepts will feel familiar).

### Development Tools

```bash
# Install Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install useful cargo extensions
cargo install cargo-watch    # Auto-rebuild on file changes
cargo install cargo-nextest  # Faster test runner
cargo install cargo-expand   # Macro expansion viewer
```

---

## 3. Building from Source

```bash
# Clone the repository
git clone https://github.com/pulseai-labs/PulseHive.git
cd pulsehive

# Build the entire workspace
cargo build

# Run all tests
cargo test --workspace

# Build documentation locally
cargo doc --workspace --no-deps --open
```

If the build succeeds, your environment is correctly configured. The first build will take longer (downloading and compiling dependencies); subsequent incremental builds complete in under 10 seconds.

---

## 4. Repository Structure

```
pulsehive/
├── pulsehive-core/             # Traits and types — the public API contract
│   └── src/
│       ├── agent.rs            # Agent, AgentDefinition, AgentKind, LlmAgentConfig
│       ├── tool.rs             # Tool trait, ToolContext, ToolResult
│       ├── lens.rs             # Lens struct, RecencyCurve
│       ├── llm.rs              # LlmProvider trait, LlmConfig, Message
│       ├── event.rs            # HiveEvent enum (all observable events)
│       ├── approval.rs         # ApprovalHandler trait, PendingAction
│       ├── embedding.rs        # EmbeddingProvider trait (future)
│       └── error.rs            # PulseHiveError enum
│
├── pulsehive-runtime/          # The engine — where the intelligence lives
│   └── src/
│       ├── hivemind.rs         # HiveMind struct, builder pattern, deploy()
│       ├── loop.rs             # Agentic loop: perceive → think → act → record
│       ├── workflow.rs         # Sequential, Parallel, Loop agent execution
│       ├── intelligence/
│       │   ├── relationship.rs # RelationshipDetector
│       │   ├── insight.rs      # InsightSynthesizer
│       │   └── context.rs      # ContextOptimizer (decay, budget, assembly)
│       ├── field.rs            # AttractorDynamics, embedding space warping
│       └── stream.rs           # Event streaming, Watch system integration
│
├── pulsehive-anthropic/        # Claude LlmProvider implementation
│   └── src/lib.rs              # AnthropicProvider: chat(), chat_stream()
│
├── pulsehive-openai/           # OpenAI-compatible LlmProvider implementation
│   └── src/lib.rs              # OpenAICompatibleProvider: works with OpenAI, GLM, vLLM, Ollama
│
├── pulsehive/                  # Meta-crate — what users cargo add
│   └── src/lib.rs              # Re-exports + feature flags (anthropic, openai)
│
├── examples/                   # Runnable examples
├── EXECUTIVE-SUMMARY.md        # Executive spec — vision, architecture, scope
├── CHANGELOG.md                # Release history
└── docs/                       # Operations and design documents
```

### Reading Order for New Contributors

1. **EXECUTIVE-SUMMARY.md**, then **docs/03-Architecture.md**: Vision, Architecture, Core Primitives, Intelligence.
2. **pulsehive-core/src/**: Read every file. This is the entire public API surface. It is small.
3. **pulsehive-runtime/src/hivemind.rs**: The orchestrator. Understand the builder and deploy flow.
4. **pulsehive-runtime/src/loop.rs**: The agentic loop. This is the heart of the system.
5. **An example**: Run one, read the code, modify it, observe behavior.

---

## 5. The Five Primitives — Quick Reference

### HiveMind

The orchestrator. Creates a shared consciousness environment, deploys agents, manages the substrate.

```rust
let hive = HiveMind::builder()
    .substrate_path("./project.db")
    .llm_provider("anthropic", AnthropicProvider::new(api_key))
    .build()?;

let mut stream = hive.deploy(agents, tasks).await?;
```

### Agent

Either an LLM-powered agent (reasoning + tools + lens) or a workflow agent (Sequential, Parallel, Loop). Products define agents; the framework runs them.

```rust
let agent = AgentDefinition {
    name: "Researcher".into(),
    kind: AgentKind::Llm(LlmAgentConfig {
        system_prompt: "You are a research analyst.".into(),
        tools: vec![Box::new(WebSearch)],
        lens: Lens::new(vec!["research"]),
        llm_config: LlmConfig::new("anthropic", "claude-sonnet-4-6"),
        experience_extractor: None,
    }),
};
```

### Tool

A pluggable capability that agents can invoke. Products implement the `Tool` trait for their domain.

```rust
#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str { "Search the web" }
    fn parameters(&self) -> serde_json::Value { json!({"query": "string"}) }
    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult> {
        // ... implementation ...
        Ok(ToolResult::text(results))
    }
}
```

### Lens

Defines how an agent perceives the substrate. Domain focus, attention weights, recency curve, attention budget.

```rust
let lens = Lens {
    domain_focus: vec!["safety".into(), "clinical".into()],
    type_weights: HashMap::from([
        (ExperienceTypeTag::ErrorPattern, 1.5),
        (ExperienceTypeTag::Solution, 1.2),
    ]),
    recency_curve: RecencyCurve::Exponential { half_life_hours: 48.0 },
    purpose_embedding: vec![],  // Computed at query time
    attention_budget: 50,
};
```

### Experience

The atom of shared consciousness. Stored in PulseDB, shared across all agents in the collective. Types include Difficulty, Solution, ErrorPattern, SuccessPattern, ArchitecturalDecision, TechInsight, Fact, and more.

---

## 6. Common Development Tasks

### Adding a New Tool

1. Create a struct implementing the `Tool` trait in your product code (not in PulseHive itself — tools are product-specific).
2. Implement `name()`, `description()`, `parameters()`, and `execute()`.
3. Add the tool to an agent's `LlmAgentConfig.tools` vector.
4. If the tool performs dangerous operations, override `requires_approval()` to return `true`.

### Implementing a Custom LlmProvider

1. Create a new crate (e.g., `pulsehive-mistral`).
2. Depend on `pulsehive-core` for the `LlmProvider` trait.
3. Implement `chat()` and `chat_stream()`.
4. Handle the `Message`, `ToolDefinition`, and `LlmConfig` types from pulsehive-core.
5. Add the crate as an optional dependency in the meta-crate with a feature flag.

### Creating a Custom Lens

Lenses are data (not traits), so creating a custom lens is just constructing a `Lens` struct with the desired parameters. Experiment with `domain_focus`, `type_weights`, `recency_curve`, and `attention_budget` to control what an agent perceives.

### Running a Specific Test

```bash
# Run all tests in a specific crate
cargo test -p pulsehive-runtime

# Run a specific test by name
cargo test -p pulsehive-runtime test_agentic_loop

# Run tests with output visible
cargo test -p pulsehive-runtime -- --nocapture
```

---

## 7. Code Style

### Formatting and Linting

```bash
# Format code (must pass before merge)
cargo fmt --all

# Lint (must pass with zero warnings)
cargo clippy --workspace --all-features -- -D warnings
```

### Naming Conventions

| Item | Convention | Example |
|------|-----------|---------|
| Crate names | `pulsehive-{name}` | `pulsehive-core`, `pulsehive-runtime` |
| Module files | `snake_case.rs` | `hivemind.rs`, `context.rs` |
| Types/Traits | `PascalCase` | `HiveMind`, `LlmProvider`, `AgentKind` |
| Functions/Methods | `snake_case` | `deploy()`, `search_similar()`, `store_experience()` |
| Constants | `SCREAMING_SNAKE_CASE` | `DEFAULT_HALF_LIFE_HOURS` |
| Feature flags | `lowercase` | `anthropic`, `openai` |

### Documentation Comments

Every public item gets a doc comment. Use `///` for items, `//!` for module-level documentation. Include examples in doc comments where practical — they are compiled and tested by `cargo test`.

```rust
/// Assemble context for an agent through its lens.
///
/// Queries the substrate, applies temporal decay, ranks by lens weights,
/// and packs results within the token budget.
///
/// # Errors
///
/// Returns `PulseHiveError::SubstrateError` if the substrate query fails.
pub async fn get_context(
    &self,
    lens: &Lens,
    task: &Task,
    budget: ContextBudget,
) -> Result<AgentContext> { ... }
```

---

## 8. Pull Request Process

### Branch Naming

```
feature/workflow-agents
fix/context-optimizer-panic
docs/observability-guide
refactor/event-bus-channels
```

### PR Requirements

```
[ ] Branch is up to date with main
[ ] cargo fmt --check passes
[ ] cargo clippy --workspace --all-features -- -D warnings passes
[ ] cargo test --workspace passes
[ ] New public API items have doc comments
[ ] Breaking changes documented in PR description
[ ] CHANGELOG.md updated if this is a user-facing change
```

### Review Process

Currently a solo developer + Claude Code workflow. As the project grows:

1. Open a PR with a clear description of what and why.
2. CI must be green before review.
3. At least one approval required before merge.
4. Squash merge to main (clean history).
5. Delete the feature branch after merge.

---

## 9. Where to Find Help

| Resource | What It Contains |
|----------|-----------------|
| `EXECUTIVE-SUMMARY.md` + `docs/03-Architecture.md` | The SDK spec — architecture, primitives, intelligence, phases |
| `docs/` | Operations documents (this file, deployment, observability, performance, maintenance) |
| `docs/pulsedb-api-reference.md` | PulseDB API surface — types, traits, methods |
| `docs/vision-mapping.md` | How PulseHive concepts map to PulseDB storage primitives |
| `discussion.md` | Design discussions — attractors, field dynamics, lenses, REFRAG analysis |
| `examples/` | Runnable code examples |
| GitHub Issues | Bug reports, feature requests, questions |

**If you are stuck:** Read EXECUTIVE-SUMMARY.md first, then docs/03-Architecture.md. If they do not answer your question, open a GitHub Issue.

---

*This document is maintained alongside the SDK. Updated as the repository structure and contribution process evolve.*
