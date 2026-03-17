# PulseHive SDK — Developer Experience (DX) Guidelines

> **Document ID:** DX-PH-006
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** SPEC v0.4.0, PRD-PH-001

---

## 1. Introduction

PulseHive is a library crate, not an application. There is no UI, no frontend, no REST API. The "user interface" is the Rust API surface consumed by developers building multi-agent systems. Every design decision in this document optimizes for one thing: making it effortless for a Rust developer to go from `cargo add pulsehive` to a running multi-agent system with shared consciousness.

This document defines the standards and conventions that govern PulseHive's public API, documentation, error handling, naming, and overall developer experience.

---

## 2. API Ergonomics

### 2.1 Builder Pattern

All complex types with more than three configuration fields use the builder pattern. Builders enforce required fields at compile time where possible and provide sensible defaults for optional fields.

```rust
// Good: builder with required fields and optional configuration
let hive = HiveMind::builder()
    .substrate(substrate)
    .llm_provider("claude", anthropic_provider)
    .build()?;

// Good: agent definition with builder
let agent = AgentDefinition::builder()
    .name("researcher")
    .system_prompt("You are a research agent...")
    .tool(search_tool)
    .tool(summarize_tool)
    .lens(research_lens)
    .llm("claude")
    .build()?;
```

Builder rules:
- Required fields are set via the builder constructor or mandatory setter methods. `build()` returns `Result<T>` to report missing fields rather than panicking.
- Optional fields have defaults documented in rustdoc on the setter method.
- Builders consume `self` (not `&mut self`) to enable method chaining without lifetime issues.
- Builders implement `Clone` when the underlying types support it, allowing template reuse.

### 2.2 Method Chaining

Where method chaining improves readability, methods return `Self` or `&mut Self`. However, chaining is not forced where it would obscure control flow. Async methods that perform I/O return `Result<T>` and break the chain naturally.

```rust
// Good: chaining for configuration
let lens = Lens::new()
    .domain("safety")
    .domain("clinical")
    .type_weight(ExperienceTypeTag::ErrorPattern, 2.0)
    .recency(RecencyCurve::Exponential { half_life_hours: 72.0 })
    .attention_budget(50);

// Good: async operations break the chain — this is expected
let stream = hive.deploy(vec![agent], vec![task]).await?;
```

### 2.3 Sensible Defaults

Every optional configuration field has a documented default that works for the common case. Developers should be able to get a working system with minimal configuration, then tune as needed.

| Configuration | Default | Rationale |
|---------------|---------|-----------|
| `RecencyCurve` | `Exponential { half_life_hours: 168.0 }` (1 week) | Balances recency and long-term memory |
| `attention_budget` | 20 | Fits within typical LLM context windows |
| `max_iterations` (agentic loop) | 10 | Prevents runaway loops without being restrictive |
| `requires_approval` (Tool) | `false` | Most tools do not need human approval |
| `experience_extractor` | Built-in heuristic extractor | Works without custom implementation |
| `context_budget` | 4096 tokens | Conservative default that works for all models |

### 2.4 Conversion Traits

Implement `From`, `Into`, and `TryFrom` generously to reduce ceremony. Accept the widest reasonable input type.

```rust
// Good: accept &str where String is stored
fn domain(mut self, domain: impl Into<String>) -> Self {
    self.domain_focus.push(domain.into());
    self
}

// Good: accept anything that implements Tool
fn tool(mut self, tool: impl Tool + 'static) -> Self {
    self.tools.push(Box::new(tool));
    self
}
```

### 2.5 The 5-Line Hello World

A developer's first experience with PulseHive should take under 15 minutes from `cargo add` to a running agent. The minimal viable code path is:

```rust
use pulsehive::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let hive = HiveMind::builder().substrate(PulseDb::memory()?).build()?;
    let agent = AgentDefinition::llm("assistant", "You are helpful.", "claude");
    let mut stream = hive.deploy(vec![agent], vec![Task::new("Summarize Rust's key features")]).await?;
    while let Some(event) = stream.next().await {
        println!("{event}");
    }
    Ok(())
}
```

This requires:
- A `prelude` module that re-exports the 5 primitives, common builders, and essential traits.
- `PulseDb::memory()` as a zero-config in-memory substrate for getting started.
- `AgentDefinition::llm()` as a convenience constructor for the most common agent type.
- `Task::new()` accepting a simple string.
- `HiveEvent` implementing `Display` for human-readable output.

---

## 3. Error Handling

### 3.1 Typed Error Enum

PulseHive uses a single top-level error type per crate, built with `thiserror`. Library code never panics — all failure modes are expressed through `Result<T, PulseHiveError>`.

```rust
#[derive(Debug, thiserror::Error)]
pub enum PulseHiveError {
    #[error("substrate error: {0}")]
    Substrate(#[from] pulsedb::PulseDbError),

    #[error("LLM provider '{provider}' failed: {message}")]
    LlmProvider { provider: String, message: String },

    #[error("tool '{tool_name}' execution failed: {message}")]
    ToolExecution { tool_name: String, message: String },

    #[error("agent '{agent_name}' exceeded max iterations ({max})")]
    MaxIterationsExceeded { agent_name: String, max: usize },

    #[error("configuration error: {0}")]
    Configuration(String),

    #[error("approval denied for tool '{tool_name}' by handler")]
    ApprovalDenied { tool_name: String },

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("context assembly failed: {0}")]
    ContextAssembly(String),
}
```

### 3.2 Error Message Quality

Every error variant carries enough context for the developer to diagnose the problem without reaching for a debugger. Error messages follow these rules:

- **Include the entity that failed**: agent name, tool name, provider name.
- **Include what was attempted**: "during experience recording", "while calling tool X".
- **Never expose internal implementation details**: no memory addresses, no internal state dumps.
- **Suggest remediation when possible**: via documentation links in error descriptions.

### 3.3 No Panics in Library Code

PulseHive will never call `unwrap()`, `expect()`, `panic!()`, or `unreachable!()` in any code path reachable by SDK consumers. Internal invariants use `debug_assert!()` for development and return errors in release mode. The only exception is truly unreachable code paths protected by the type system, which use `unreachable!()` with a descriptive message.

### 3.4 Result Type Alias

Each crate defines a convenience alias:

```rust
pub type Result<T> = std::result::Result<T, PulseHiveError>;
```

---

## 4. Documentation Standards

### 4.1 Rustdoc on Every Public Item

Every `pub` item (struct, enum, trait, function, method, field, variant) has a rustdoc comment. No exceptions. `cargo doc --no-deps` must produce zero warnings.

```rust
/// Defines how an agent perceives the shared substrate.
///
/// A `Lens` filters and re-ranks experiences from PulseDB based on domain focus,
/// type weights, and temporal decay. Two agents with different lenses see the same
/// substrate differently — a safety agent perceives error patterns prominently,
/// while a planning agent perceives architectural decisions.
///
/// # Examples
///
/// ```rust
/// use pulsehive::Lens;
///
/// let safety_lens = Lens::new()
///     .domain("safety")
///     .type_weight(ExperienceTypeTag::ErrorPattern, 3.0)
///     .recency(RecencyCurve::Exponential { half_life_hours: 48.0 })
///     .attention_budget(30);
/// ```
///
/// See also: [`HiveMind::get_context`], [`AgentDefinition`]
pub struct Lens { ... }
```

### 4.2 Doc Comment Structure

Every public type follows this structure:
1. **One-line summary** (imperative mood): "Defines how an agent perceives..."
2. **Expanded explanation** (1-3 paragraphs): what it does, why it matters, when to use it.
3. **`# Examples`** section with compilable code.
4. **`# Errors`** section on fallible functions listing when each error variant is returned.
5. **`# Panics`** section if the function can panic (should be extremely rare).
6. **Cross-references** with `[`backtick links`]` to related types.

### 4.3 Examples in Doc Comments

Every example in a doc comment must compile and run. Use `cargo test --doc` to verify. Examples demonstrate real use cases, not trivial constructions.

```rust
/// # Examples
///
/// Deploy two agents that share consciousness through the substrate:
///
/// ```rust,no_run
/// # use pulsehive::prelude::*;
/// # async fn example() -> pulsehive::Result<()> {
/// let hive = HiveMind::builder()
///     .substrate(PulseDb::open("./my-project")?)
///     .llm_provider("claude", AnthropicProvider::new(api_key))
///     .build()?;
///
/// let researcher = AgentDefinition::llm("researcher", "Find relevant patterns.", "claude");
/// let implementer = AgentDefinition::llm("implementer", "Apply found patterns.", "claude");
///
/// let stream = hive.deploy(
///     vec![researcher, implementer],
///     vec![Task::new("Refactor the auth module")],
/// ).await?;
/// # Ok(())
/// # }
/// ```
```

### 4.4 Crate-Level Documentation

Each crate's `lib.rs` has a crate-level doc comment (`//!`) that provides:
- What the crate does in one sentence.
- A minimal working example.
- Feature flags and what they enable.
- Links to the other crates in the workspace.

---

## 5. Getting Started Flow

The intended learning path for a new PulseHive developer:

### Step 1: Install (30 seconds)

```toml
[dependencies]
pulsehive = "0.1"
tokio = { version = "1", features = ["full"] }
```

### Step 2: Hello World (5 minutes)

Run a single agent with no tools, in-memory substrate. See events stream to stdout. Understand that `HiveMind` orchestrates, `AgentDefinition` describes an agent, and `Task` defines work.

### Step 3: Add a Tool (10 minutes)

Implement the `Tool` trait for a simple capability (e.g., a calculator). See the agent use it. Understand that tools are how agents interact with the world.

### Step 4: Add a Lens (15 minutes)

Create a `Lens` that focuses the agent on specific experience types. Seed the substrate with sample experiences. See how the lens changes what the agent perceives. Understand that lenses are how agents see the substrate differently.

### Step 5: Multi-Agent System (30 minutes)

Deploy two agents with different lenses and tools sharing the same substrate. See one agent's experiences appear in the other agent's context. Understand that shared consciousness is the core innovation.

### Step 6: Production Patterns (ongoing)

Explore workflow agents, custom experience extractors, approval handlers, intelligence tuning. Reference the SPEC and API docs for advanced configuration.

---

## 6. Code Examples Quality

### 6.1 Every Example Compiles

All code examples in documentation, README, and the `examples/` directory are tested in CI. Doc examples run via `cargo test --doc`. Standalone examples run via `cargo run --example <name>`.

### 6.2 Examples Directory Structure

```
examples/
├── hello_world.rs          # Minimal single-agent example
├── tool_use.rs             # Implementing and using a custom tool
├── multi_agent.rs          # Two agents sharing consciousness
├── workflow_agents.rs      # Sequential/Parallel/Loop composition
├── custom_lens.rs          # Lens configuration and perception filtering
├── event_handling.rs       # Consuming and reacting to HiveEvent stream
└── openai_compatible.rs    # Using GLM/vLLM/Ollama as LLM provider
```

### 6.3 Example Guidelines

- Every example has a file-level doc comment explaining what it demonstrates.
- Examples use `anyhow::Result` for brevity (not `PulseHiveError` — save that for production code).
- Examples print human-readable output so developers can see what is happening.
- Examples that require API keys read from environment variables and print a clear message if missing.
- No example exceeds 80 lines of code. If it does, it should be split or simplified.

---

## 7. Naming Conventions

PulseHive follows standard Rust naming conventions without exception:

| Element | Convention | Examples |
|---------|-----------|----------|
| Types, traits, enums | `CamelCase` | `HiveMind`, `AgentDefinition`, `Tool`, `Lens` |
| Functions, methods | `snake_case` | `deploy()`, `record_experience()`, `get_context()` |
| Constants | `SCREAMING_SNAKE_CASE` | `DEFAULT_ATTENTION_BUDGET`, `MAX_ITERATIONS` |
| Modules | `snake_case` | `pulsehive_core`, `llm_provider` |
| Feature flags | `kebab-case` | `anthropic`, `openai-compat`, `full` |
| Enum variants | `CamelCase` | `HiveEvent::AgentStarted`, `ExperienceType::Solution` |
| Lifetimes | Short, descriptive | `'a`, `'ctx`, `'substrate` |
| Type parameters | Single uppercase or short descriptive | `T`, `E`, `S: SubstrateProvider` |

### 7.1 Domain-Specific Naming

PulseHive's domain vocabulary is consistent across the entire codebase:

| Term | Meaning | Never called |
|------|---------|-------------|
| **Substrate** | The PulseDB storage layer | Database, store, backend, persistence |
| **Experience** | A knowledge unit in the substrate | Memory, document, record, entry |
| **Lens** | An agent's perception filter | Filter, view, scope, projection |
| **Collective** | A group of agents sharing a substrate | Team, group, swarm, cluster |
| **Deploy** | Start agents working on tasks | Run, execute, launch, start |
| **Perceive** | Read substrate through a lens | Query, fetch, retrieve, load |

---

## 8. Prelude Module

The `prelude` module re-exports exactly what a developer needs for the common case. It is intentionally curated — not a dump of everything public.

```rust
pub mod prelude {
    // Core primitives
    pub use crate::{HiveMind, AgentDefinition, AgentKind, Task};
    pub use pulsehive_core::{Tool, ToolContext, ToolResult, Lens, RecencyCurve};
    pub use pulsehive_core::{HiveEvent, ExperienceTypeTag};

    // Essential traits
    pub use pulsehive_core::{LlmProvider, SubstrateProvider};

    // Re-exports for ergonomics
    pub use futures::StreamExt;
    pub use crate::Result;
}
```

Items not in the prelude: intelligence tuning structs, internal event bus types, builder internals, provider-specific configuration. These are importable by path for advanced use cases.

---

## 9. Versioning

### 9.1 Semver Compliance

PulseHive follows Rust's semver conventions strictly:

- **Pre-1.0** (current): Minor versions (0.x.0) may contain breaking changes. Patch versions (0.x.y) are backwards-compatible.
- **Post-1.0**: Breaking changes only at major versions. Deprecation warnings for at least one minor version before removal.

### 9.2 What Counts as Breaking

- Removing or renaming a public type, trait, function, or method.
- Changing a function signature (parameters, return type).
- Adding a required field to a public struct (use `#[non_exhaustive]` to prevent this).
- Removing a trait implementation.
- Changing the behavior of an existing function in a way that violates its documented contract.

### 9.3 Non-Exhaustive Enums and Structs

All public enums and structs that may grow use `#[non_exhaustive]`:

```rust
#[non_exhaustive]
pub enum HiveEvent {
    AgentStarted { agent_id: AgentId, name: String },
    AgentCompleted { agent_id: AgentId, result: String },
    ToolCalled { agent_id: AgentId, tool_name: String },
    // Future variants can be added without breaking downstream code
}
```

This ensures that adding new variants or fields is not a breaking change.

### 9.4 MSRV Policy

The minimum supported Rust version (MSRV) is documented in `Cargo.toml` and tested in CI. MSRV bumps are treated as minor version changes pre-1.0 and major version changes post-1.0.

---

## 10. Diagnostic Output

### 10.1 Tracing Integration

PulseHive uses the `tracing` crate for all diagnostic output. No `println!()` or `eprintln!()` in library code. SDK consumers control verbosity by configuring their own `tracing-subscriber`.

```rust
// Good: structured tracing with relevant fields
tracing::info!(agent = %agent_name, tool = %tool_name, "tool execution started");
tracing::debug!(experiences = results.len(), "substrate search completed");
tracing::warn!(agent = %agent_name, iteration = i, max = max_iter, "approaching iteration limit");
```

### 10.2 Span Structure

Top-level operations create tracing spans so consumers can correlate events:

- `hivemind.deploy` — wraps the entire deployment lifecycle
- `agent.loop` — wraps one agent's perceive-think-act-record cycle
- `agent.perceive` — substrate query and context assembly
- `agent.think` — LLM call
- `agent.act` — tool execution
- `agent.record` — experience extraction and storage

### 10.3 No Secrets in Logs

API keys, substrate file paths, and raw LLM responses are never logged at any level. Tracing fields that might contain sensitive data are redacted or omitted. See the Security document (07-Security.md) for details.
