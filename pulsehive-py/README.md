# PulseHive Python Bindings

Python bindings for the [PulseHive](https://github.com/pulsehive/pulsehive) Rust SDK — shared consciousness for multi-agent AI systems.

## Installation

```bash
# Development install (requires Rust toolchain + maturin)
cd pulsehive-py
python -m venv .venv && source .venv/bin/activate
pip install maturin pytest pytest-asyncio
maturin develop
```

## Quickstart

```python
import asyncio
from pulsehive import (
    HiveMind, AgentDefinition, AgentKind,
    Lens, LlmConfig, Task, openai_provider,
)

async def main():
    # 1. Build HiveMind with substrate and LLM provider
    hive = (
        HiveMind.builder()
        .substrate_path("/tmp/my_project.db")
        .llm_provider("openai", openai_provider("sk-...", "gpt-4"))
        .build()
    )

    # 2. Define an agent
    lens = Lens(["code", "architecture"])
    config = LlmConfig("openai", "gpt-4")
    kind = AgentKind.llm(
        system_prompt="You are a code analysis expert.",
        lens=lens,
        llm_config=config,
    )
    agent = AgentDefinition("analyzer", kind)

    # 3. Deploy and consume events
    stream = await hive.deploy([agent], [Task("Analyze the codebase")])
    async for event in stream:
        print(f"[{event.event_type}] {event.data}")
        if event.event_type == "agent_completed":
            break

asyncio.run(main())
```

## API Overview

### Core Types

| Type | Description |
|------|-------------|
| `LlmConfig(provider, model)` | LLM selection and generation params |
| `Lens(domains)` | Perception filter for substrate access |
| `RecencyCurve.exponential(h)` / `.uniform()` | Temporal decay function |
| `AgentKind.llm(prompt, lens, config)` | LLM-powered agent |
| `AgentKind.sequential(agents)` | Run children in order |
| `AgentKind.parallel(agents)` | Run children concurrently |
| `AgentKind.loop_(agent, n)` | Repeat agent N times |
| `AgentDefinition(name, kind)` | Agent blueprint |
| `Task(description)` | Task for deployment |
| `HiveEvent` | Lifecycle event (`.event_type`, `.data`, `.agent_id`) |

### Provider Factories

```python
openai_provider(api_key, model="gpt-4", base_url=None)
anthropic_provider(api_key)
```

### HiveMind Builder

```python
hive = (
    HiveMind.builder()
    .substrate_path("/tmp/db.db")
    .llm_provider("openai", openai_provider("sk-..."))
    .build()
)
```

## Workflow Agents

```python
# Sequential pipeline
pipeline = AgentDefinition("pipeline", AgentKind.sequential([
    AgentDefinition("step1", AgentKind.llm("Analyze", lens, config)),
    AgentDefinition("step2", AgentKind.llm("Summarize", lens, config)),
]))

# Parallel execution
parallel = AgentDefinition("team", AgentKind.parallel([agent1, agent2]))

# Loop with early exit
loop = AgentDefinition("iterator", AgentKind.loop_(agent, max_iterations=5))
```

## Current Limitations (Sprint 10)

- **No custom Tools from Python** — tools must be defined in Rust. Python Tool interface coming in Sprint 11.
- **No custom LlmProvider from Python** — use built-in `openai_provider()` or `anthropic_provider()`. Custom providers coming in Sprint 11.
- **No custom ApprovalHandler from Python** — uses AutoApprove. Custom approval coming in Sprint 11.

## Running Tests

```bash
cd /path/to/PulseHive
source pulsehive-py/.venv/bin/activate
pytest pulsehive-py/tests/ -v
```
