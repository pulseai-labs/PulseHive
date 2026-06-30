# PulseHive

**Shared Consciousness SDK for Multi-Agent AI Systems**

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)

PulseHive is a Rust SDK for building multi-agent AI systems where agents share consciousness through a persistent substrate instead of passing messages. When one agent learns something, all agents in that collective immediately perceive it — no coordination protocol, no message queue, no explicit sharing.

## Why PulseHive?

Traditional multi-agent frameworks force agents to communicate through message-passing or in-memory shared state. This leads to:

- **Coordination overhead** that scales O(n^2) with agent count
- **Context loss** as rich understanding gets compressed into text messages
- **Inconsistent state** causing 36.9% of multi-agent failures ([O'Reilly 2025](https://www.oreilly.com/radar/))
- **No persistent learning** — every session starts from zero

PulseHive eliminates all of this. Agents don't communicate — they **share consciousness** through [PulseDB](https://crates.io/crates/pulsehive-db), a persistent embedded database with real-time change propagation.

## Key Features

- **Shared Consciousness** — Agents perceive a shared substrate (PulseDB) instead of exchanging messages. Write once, perceived by all.
- **Lens-Based Perception** — Each agent sees the substrate differently based on its role. A safety analyst and a financial reviewer perceive the same data through different lenses.
- **Intelligence Layer** — Automatic relationship detection between experiences, cross-experience insight synthesis, and context optimization with temporal decay.
- **Workflow Agents** — Compose `Sequential`, `Parallel`, and `Loop` orchestration patterns without LLM overhead.
- **Provider Agnostic** — Works with any LLM: Claude, GPT, GLM, Ollama, vLLM, or any OpenAI-compatible API.
- **Observable by Default** — Every operation emits structured events via the `tracing` crate. No vendor lock-in.
- **Zero Infrastructure** — PulseDB is embedded (like SQLite). No server, no connection string. Just a file path.

## Quick Start

```rust
use pulsehive::prelude::*;
use pulsehive::{HiveMind, Task};

#[tokio::main]
async fn main() -> Result<(), PulseHiveError> {
    // Create a hive mind with PulseDB substrate (auto-downloads embedding model)
    let hive = HiveMind::builder()
        .substrate_path("./my_project.db")
        .llm_provider("openai", my_openai_provider)
        .build()?;

    // Define an agent with tools and a perception lens
    let agent = AgentDefinition {
        name: "Researcher".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a research analyst.".into(),
            tools: vec![Box::new(WebSearch)],
            lens: Lens::new(["research", "analysis"]),
            llm_config: LlmConfig::new("openai", "gpt-4o"),
            experience_extractor: None, // Uses default extractor
        })),
    };

    // Deploy — agent perceives substrate, thinks, acts, records experiences
    let mut stream = hive.deploy(vec![agent], vec![Task::new("Analyze the data")]).await?;
    while let Some(event) = stream.next().await {
        println!("{event:?}");
    }

    Ok(())
}
```

> See [`examples/cli_agent.rs`](pulsehive-runtime/examples/cli_agent.rs) for a complete runnable example.

## Multi-Agent Example

```rust
// Parallel analysis → sequential synthesis → report
let pipeline = AgentDefinition {
    name: "Analysis Pipeline".into(),
    kind: AgentKind::Sequential(vec![
        AgentDefinition {
            name: "Parallel Analysis".into(),
            kind: AgentKind::Parallel(vec![
                safety_analyst,
                literature_reviewer,
                statistical_analyst,
            ]),
        },
        medical_reviewer,   // Sees ALL analysis via substrate
        report_generator,   // Synthesizes final output
    ]),
};

let mut stream = hive.deploy(vec![pipeline], vec![task]).await?;
```

All agents share the same substrate. When the safety analyst discovers a signal, the literature reviewer already sees it on its next perception cycle. No message passing required.

## Architecture

```
Products (Your Application)
    |
    v
PulseHive SDK
    ├── pulsehive-core      — Traits: Agent, Tool, Lens, LlmProvider
    ├── pulsehive-runtime   — HiveMind, agentic loop, intelligence layer
    ├── pulsehive-openai    — OpenAI-compatible provider (GPT, GLM, vLLM, Ollama)
    ├── pulsehive-anthropic — Claude provider
    └── pulsehive           — Meta-crate with feature flags
    |
    v
PulseDB (pulsehive-db)
    — Embedded storage substrate
    — HNSW vector search (384d)
    — Real-time Watch system
    — Experience graph with relations and insights
```

### Five Core Primitives

| Primitive | Purpose |
|-----------|---------|
| **HiveMind** | Orchestrator — deploys agents, manages substrate, runs intelligence |
| **Agent** | `LlmAgent` (reasoning + tools) or `WorkflowAgent` (Sequential / Parallel / Loop) |
| **Tool** | Pluggable capability — you implement for your domain |
| **Lens** | Perception filter — how an agent sees the substrate |
| **Experience** | Knowledge unit — stored in PulseDB, shared across agents |

## How It Works

1. **Perceive** — Agent queries the substrate through its Lens, receiving relevant experiences as intrinsic knowledge
2. **Think** — LLM reasons with the perceived context + task description
3. **Act** — LLM calls tools; results feed back into the conversation
4. **Record** — Learnings are extracted and written to the substrate

Other agents immediately perceive the new experiences via PulseDB's Watch system. Intelligence algorithms automatically detect relationships between experiences and synthesize cross-agent insights.

## vs Other Frameworks

| Capability | LangGraph | Google ADK | CrewAI | **PulseHive** |
|---|---|---|---|---|
| Shared state | In-memory checkpoints | Session dict | Message passing | **Database-native (PulseDB)** |
| Real-time cross-agent | Superstep boundaries | Shared dict | No | **Watch system (instant)** |
| Semantic search over history | No | No | Basic | **HNSW native** |
| Pre-computed reasoning | No | No | No | **InsightSynthesizer** |
| Per-agent perception | No | No | No | **Lens system** |
| Persistence | Checkpoint snapshots | Session service | SQLite | **Continuous (PulseDB)** |
| Language | Python | Python | Python | **Rust** (Python/TS bindings planned) |

## Installation

```toml
[dependencies]
pulsehive = { version = "2.0", features = ["openai"] }
# or
pulsehive = { version = "2.0", features = ["anthropic"] }
# or both
pulsehive = { version = "2.0", features = ["openai", "anthropic"] }
```

## Documentation

- [**SDK Specification**](SPEC.md) — Full architecture, primitives, intelligence layer, development phases
- [**Product Requirements**](docs/01-PRD.md) — Features, personas, success metrics
- [**System Requirements**](docs/02-SRS.md) — Functional and non-functional requirements
- [**Architecture**](docs/03-Architecture.md) — C4 model, data flows, architecture decisions
- [**Data Model**](docs/04-Data-Model.md) — PulseDB entities and relationships
- [**API Specification**](docs/05-API-Spec.md) — Public traits, structs, and methods
- [**Getting Started**](docs/getting-started.md) — Setup and first steps for Rust, Python, and TypeScript
- [**Testing Strategy**](docs/08-Testing.md) — Unit, integration, property-based, and benchmarks
- [**Performance Benchmarks**](docs/benchmarks.md) — Latency results at 1K–10K experience scales
- [**PulseDB API Reference**](docs/pulsedb-api-reference.md) — Storage substrate API surface
- [**Contributing**](CONTRIBUTING.md) — Development setup, code quality, and PR process

## Project Status

PulseHive **v1.0.0** is released — production-ready with Rust, Python, and TypeScript support.

| Phase | Status | Deliverable |
|-------|--------|-------------|
| Phase 1: Foundation | Complete | Single agent + tools + substrate persistence |
| Phase 2: Multi-Agent | Complete | Parallel agents + shared consciousness + intelligence |
| Phase 3: Python Bindings | Complete | `pip install pulsehive` |
| Phase 4: Ecosystem | Complete | TypeScript bindings + EmbeddingProvider + AttractorDynamics + v1.0 |

## Related Projects

- [**PulseDB**](https://crates.io/crates/pulsehive-db) — The embedded storage substrate powering PulseHive's shared consciousness
- The first vertical product built on PulseHive (details to be announced)

## Contributing

PulseHive is open source under the AGPL-3.0 license. Contributions are welcome.

```bash
git clone https://github.com/pulseai-labs/PulseHive.git
cd PulseHive
cargo build --workspace
cargo test --workspace
```

See [Team Onboarding](docs/14-Team-Onboarding.md) for development setup and contribution guidelines.

## License

PulseHive is licensed under the [GNU Affero General Public License v3.0](LICENSE). A commercial license is also available for use cases that AGPL-3.0 does not suit — see [LICENSING.md](./LICENSING.md) for details.
