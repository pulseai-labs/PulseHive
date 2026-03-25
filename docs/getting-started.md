# Getting Started with PulseHive

PulseHive is a Rust SDK for building multi-agent AI systems where agents share consciousness through a persistent substrate instead of passing messages. This guide covers setup and first steps for **Rust**, **Python**, and **TypeScript**.

## Prerequisites

| Language | Requirement |
|----------|-------------|
| Rust | Stable toolchain via [rustup](https://rustup.rs/) |
| Python | Python 3.9+ with pip |
| TypeScript | Node.js 18+ with npm |

## Installation

### Rust

```toml
# Cargo.toml
[dependencies]
pulsehive = { version = "1.0", features = ["openai"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

### Python

```bash
pip install pulsehive
```

### TypeScript

```bash
npm install @pulsehive/sdk
```

## Hello World — Single Agent

### Rust

```rust
use std::sync::Arc;
use pulsehive::prelude::*;
use pulsehive::{HiveMind, Task};

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Build HiveMind with substrate and LLM provider
    let hive = HiveMind::builder()
        .substrate_path("my_project.db")
        .llm_provider("openai", my_openai_provider)
        .build()?;

    // 2. Define an agent with a perception lens
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

    // 3. Deploy and consume events
    let mut stream = hive.deploy(vec![agent], vec![Task::new("Analyze the codebase")]).await?;
    while let Some(event) = stream.next().await {
        println!("{event:?}");
    }
    Ok(())
}
```

See [`pulsehive-runtime/examples/cli_agent.rs`](../pulsehive-runtime/examples/cli_agent.rs) for a full runnable example with a mock LLM.

### Python

```python
import asyncio
from pulsehive import (
    HiveMind, AgentDefinition, AgentKind,
    Lens, LlmConfig, Task, openai_provider,
)

async def main():
    hive = (
        HiveMind.builder()
        .substrate_path("/tmp/my_project.db")
        .llm_provider("openai", openai_provider("sk-...", "gpt-4"))
        .build()
    )

    agent = AgentDefinition("analyzer", AgentKind.llm(
        system_prompt="You are a code analysis expert.",
        lens=Lens(["code", "architecture"]),
        llm_config=LlmConfig("openai", "gpt-4"),
    ))

    stream = await hive.deploy([agent], [Task("Analyze the codebase")])
    async for event in stream:
        print(f"[{event.event_type}] {event.data}")

asyncio.run(main())
```

See [`pulsehive-py/examples/getting_started.py`](../pulsehive-py/examples/getting_started.py) for a full example.

### TypeScript

```typescript
const {
  HiveMind, Task,
  JsAgentKind: AgentKind,
  JsAgentDefinition: AgentDefinition,
  JsLens: Lens, JsLlmConfig: LlmConfig,
  openaiProvider,
} = require("@pulsehive/sdk");

async function main() {
  const hive = HiveMind.builder()
    .substratePath("/tmp/my_project.db")
    .llmProvider("openai", openaiProvider("sk-...", "gpt-4"))
    .build();

  const agent = new AgentDefinition("analyzer", AgentKind.llm(
    "You are a code analysis expert.",
    new Lens(["code", "architecture"]),
    new LlmConfig("openai", "gpt-4"),
  ));

  const stream = await hive.deploy([agent], [new Task("Analyze the codebase")]);
  for await (const event of stream) {
    console.log(`[${event.eventType}] ${JSON.stringify(event.data)}`);
  }
  hive.shutdown();
}

main().catch(console.error);
```

See [`pulsehive-js/examples/getting-started.ts`](../pulsehive-js/examples/getting-started.ts) for a full example.

## Custom Tools

Tools give agents capabilities. Implement the `Tool` trait (Rust), a duck-typed class (Python), or use `defineTool()` (TypeScript).

### Rust

```rust
struct Calculator;

#[async_trait::async_trait]
impl Tool for Calculator {
    fn name(&self) -> &str { "calculator" }
    fn description(&self) -> &str { "Basic arithmetic" }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "number" },
                "b": { "type": "number" },
                "op": { "type": "string", "enum": ["add", "multiply"] }
            },
            "required": ["a", "b", "op"]
        })
    }
    async fn execute(&self, params: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let a = params["a"].as_f64().unwrap_or(0.0);
        let b = params["b"].as_f64().unwrap_or(0.0);
        let result = match params["op"].as_str().unwrap_or("add") {
            "multiply" => a * b,
            _ => a + b,
        };
        Ok(ToolResult::text(format!("{result}")))
    }
}

// Attach to agent
let agent = AgentDefinition {
    name: "calc-agent".into(),
    kind: AgentKind::Llm(Box::new(LlmAgentConfig {
        tools: vec![Arc::new(Calculator)],
        // ... other fields
    })),
};
```

See [`pulsehive-runtime/examples/custom_tool.rs`](../pulsehive-runtime/examples/custom_tool.rs) for calculator, word counter, and approval-required tool examples.

### Python

```python
class Calculator:
    def name(self): return "calculator"
    def description(self): return "Basic arithmetic"
    def parameters(self): return {
        "type": "object",
        "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
        "required": ["a", "b"],
    }
    def execute(self, params, context):
        return str(params["a"] + params["b"])

agent = AgentDefinition("calc", AgentKind.llm("Use tools.", Lens([]), config, tools=[Calculator()]))
```

### TypeScript

```typescript
const { defineTool } = require("@pulsehive/sdk");

const calculator = defineTool({
  name: "calculator",
  description: "Basic arithmetic",
  parameters: {
    type: "object",
    properties: { a: { type: "number" }, b: { type: "number" } },
    required: ["a", "b"],
  },
  execute: async (params, context) => String(params.a + params.b),
});
```

## Multi-Agent Workflows

PulseHive supports three workflow patterns. In all cases, agents share a substrate — the second agent doesn't receive the first's output as a message; it **perceives** it through its Lens.

### Sequential — Agents Run in Order

```rust
let pipeline = AgentDefinition {
    name: "pipeline".into(),
    kind: AgentKind::Sequential(vec![
        researcher,   // Writes findings to substrate
        summarizer,   // Perceives findings through its Lens
    ]),
};
```

### Parallel — Agents Run Concurrently

```rust
let team = AgentDefinition {
    name: "team".into(),
    kind: AgentKind::Parallel(vec![frontend_reviewer, backend_reviewer]),
};
```

### Loop — Repeat Until Done

```rust
let loop_agent = AgentDefinition {
    name: "refiner".into(),
    kind: AgentKind::Loop {
        agent: Box::new(writer),
        max_iterations: 5,  // Exits early if response contains [LOOP_DONE]
    },
};
```

See [`pulsehive-runtime/examples/multi_agent_workflow.rs`](../pulsehive-runtime/examples/multi_agent_workflow.rs) for nested workflows combining all three patterns.

## Using Real LLM Providers

### OpenAI (and compatible: Azure, Ollama, vLLM)

```rust
// Rust
use pulsehive_openai::{OpenAIConfig, OpenAICompatibleProvider};

let config = OpenAIConfig::new("sk-...", "gpt-4");
let provider = OpenAICompatibleProvider::new(config);
let hive = HiveMind::builder()
    .llm_provider("openai", provider)
    // ...
```

```python
# Python
hive = HiveMind.builder()
    .llm_provider("openai", openai_provider("sk-...", "gpt-4"))
    .build()
```

```typescript
// TypeScript
const hive = HiveMind.builder()
    .llmProvider("openai", openaiProvider("sk-...", "gpt-4"))
    .build();
```

### Anthropic Claude

```rust
// Rust
use pulsehive_anthropic::AnthropicProvider;

let provider = AnthropicProvider::new("sk-ant-...");
let hive = HiveMind::builder()
    .llm_provider("anthropic", provider)
    // ...
```

```python
# Python
hive = HiveMind.builder()
    .llm_provider("anthropic", anthropic_provider("sk-ant-..."))
    .build()
```

## Next Steps

- **API Documentation**: `cargo doc --no-deps --workspace --open`
- **Architecture**: [`SPEC.md`](../SPEC.md) — full SDK specification
- **Examples**: [`pulsehive-runtime/examples/`](../pulsehive-runtime/examples/), [`pulsehive-py/examples/`](../pulsehive-py/examples/), [`pulsehive-js/examples/`](../pulsehive-js/examples/)
- **Contributing**: [`CONTRIBUTING.md`](../CONTRIBUTING.md)
- **PulseDB**: [`docs/pulsedb-api-reference.md`](pulsedb-api-reference.md) — storage substrate API
