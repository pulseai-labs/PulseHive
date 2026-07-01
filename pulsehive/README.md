# PulseHive

**Shared Consciousness SDK for Multi-Agent AI Systems**

PulseHive is a Rust SDK where AI agents share knowledge through a persistent substrate ([PulseDB](https://crates.io/crates/pulsehive-db)) instead of passing messages. Agents perceive each other's experiences through configurable lenses, enabling implicit coordination without explicit communication.

This is the **meta-crate** — it re-exports [`pulsehive-core`](https://crates.io/crates/pulsehive-core) and [`pulsehive-runtime`](https://crates.io/crates/pulsehive-runtime) with optional LLM providers.

## Quick Start

```toml
[dependencies]
pulsehive = { version = "1.0", features = ["openai"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

```rust
use pulsehive::prelude::*;
use pulsehive::{HiveMind, Task};

#[tokio::main]
async fn main() -> Result<()> {
    let hive = HiveMind::builder()
        .substrate_path("my_project.db")
        .llm_provider("openai", my_provider)
        .build()?;

    let agent = AgentDefinition {
        name: "analyzer".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a code analysis expert.".into(),
            tools: vec![],
            lens: Lens::new(["code", "architecture"]),
            llm_config: LlmConfig::new("openai", "gpt-4"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    };

    let mut stream = hive.deploy(vec![agent], vec![Task::new("Analyze codebase")]).await?;
    while let Some(event) = stream.next().await {
        println!("{event:?}");
    }
    Ok(())
}
```

## Feature Flags

| Flag | Enables | Dependency |
|------|---------|------------|
| `openai` | OpenAI, Azure, Ollama, vLLM, Groq, Together | [`pulsehive-openai`](https://crates.io/crates/pulsehive-openai) |
| `anthropic` | Claude Opus, Sonnet, Haiku | [`pulsehive-anthropic`](https://crates.io/crates/pulsehive-anthropic) |

## Core Primitives

| Primitive | Purpose |
|-----------|---------|
| **HiveMind** | Orchestrator — deploys agents, manages substrate |
| **Agent** | LLM-powered or workflow (Sequential/Parallel/Loop) |
| **Tool** | Pluggable capability given to agents |
| **Lens** | Perception filter — how an agent sees the substrate |
| **Experience** | Knowledge unit stored in PulseDB |

## Multi-Agent Workflows

```rust
// Sequential: each agent perceives previous results via substrate
let pipeline = AgentDefinition {
    name: "pipeline".into(),
    kind: AgentKind::Sequential(vec![researcher, summarizer]),
};

// Parallel: agents work concurrently, sharing substrate in real-time
let team = AgentDefinition {
    name: "team".into(),
    kind: AgentKind::Parallel(vec![frontend_reviewer, backend_reviewer]),
};

// Loop: repeat until [LOOP_DONE] or max iterations
let refiner = AgentDefinition {
    name: "refiner".into(),
    kind: AgentKind::Loop { agent: Box::new(writer), max_iterations: 5 },
};
```

## Language Bindings

- **Python**: [`pip install pulsehive`](https://pypi.org/project/pulsehive/) (PyO3)
- **TypeScript**: [`npm install @pulsehive/sdk`](https://www.npmjs.com/package/@pulsehive/sdk) (napi-rs)

## Links

- [GitHub](https://github.com/pulseai-labs/PulseHive)
- [API Docs (docs.rs)](https://docs.rs/pulsehive)
- [SDK Overview](https://github.com/pulseai-labs/PulseHive/blob/main/EXECUTIVE-SUMMARY.md)
- [Getting Started Guide](https://github.com/pulseai-labs/PulseHive/blob/main/docs/getting-started.md)

## License

AGPL-3.0-only
