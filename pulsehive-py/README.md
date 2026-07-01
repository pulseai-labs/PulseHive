# PulseHive Python Bindings

Python bindings for the [PulseHive](https://github.com/pulseai-labs/PulseHive) Rust SDK — shared consciousness for multi-agent AI systems.

Get Rust-native performance with Python ergonomics. Define agents, tools, and workflows in Python while the runtime executes in Rust.

## Installation

```bash
pip install pulsehive
```

For development:

```bash
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
    # Build HiveMind with substrate and LLM provider
    hive = (
        HiveMind.builder()
        .substrate_path("/tmp/my_project.db")
        .llm_provider("openai", openai_provider("sk-...", "gpt-4"))
        .build()
    )

    # Define an agent with perception lens
    agent = AgentDefinition("analyzer", AgentKind.llm(
        system_prompt="You are a code analysis expert.",
        lens=Lens(["code", "architecture"]),
        llm_config=LlmConfig("openai", "gpt-4"),
    ))

    # Deploy and consume events
    stream = await hive.deploy([agent], [Task("Analyze the codebase")])
    async for event in stream:
        print(f"[{event.event_type}] {event.data}")
        if event.event_type == "agent_completed":
            break

asyncio.run(main())
```

## Custom Tools

Define tools as plain Python classes — no base class needed (duck-typing protocol):

```python
class SearchTool:
    def name(self): return "search"
    def description(self): return "Search the web for information"
    def parameters(self): return {
        "type": "object",
        "properties": {"query": {"type": "string"}},
        "required": ["query"],
    }
    def execute(self, params, context):
        # context.agent_id and context.collective_id available
        return f"Results for: {params['query']}"

# Attach tools to agents
agent = AgentDefinition("researcher", AgentKind.llm(
    "You research topics.", Lens([]), LlmConfig("openai", "gpt-4"),
    tools=[SearchTool()],
))
```

Tools must implement `name()`, `description()`, `parameters()`, and `execute(params, context)`. Optional: `requires_approval()` returning `True` for sensitive operations.

## Workflow Agents

```python
# Sequential pipeline — each agent perceives previous results
pipeline = AgentDefinition("pipeline", AgentKind.sequential([
    AgentDefinition("step1", AgentKind.llm("Analyze", lens, config)),
    AgentDefinition("step2", AgentKind.llm("Summarize", lens, config)),
]))

# Parallel execution — agents work concurrently, sharing substrate
team = AgentDefinition("team", AgentKind.parallel([agent1, agent2]))

# Loop — repeat until [LOOP_DONE] or max iterations
loop = AgentDefinition("iterator", AgentKind.loop_(agent, max_iterations=5))
```

## API Reference

### Core Types

| Type | Description |
|------|-------------|
| `LlmConfig(provider, model)` | LLM selection and generation parameters |
| `Lens(domains)` | Perception filter for substrate access |
| `RecencyCurve.exponential(h)` / `.uniform()` | Temporal decay function |
| `AgentKind.llm(prompt, lens, config, tools)` | LLM-powered agent |
| `AgentKind.sequential(agents)` | Run children in order |
| `AgentKind.parallel(agents)` | Run children concurrently |
| `AgentKind.loop_(agent, n)` | Repeat agent N times |
| `AgentDefinition(name, kind)` | Agent blueprint |
| `Task(description)` | Task for deployment |
| `HiveEvent` | Lifecycle event (`.event_type`, `.data`, `.agent_id`) |
| `ToolContext` | Runtime context for tools (`.agent_id`, `.collective_id`) |
| `ToolResult.text(s)` / `.json(d)` / `.error(s)` | Tool execution result |

### Provider Factories

```python
openai_provider(api_key, model="gpt-4", base_url=None)  # OpenAI, Azure, Ollama, vLLM
anthropic_provider(api_key)                               # Anthropic Claude
```

## Examples

See [`examples/`](examples/) for runnable scripts:

- **[getting_started.py](examples/getting_started.py)** — Single agent with event stream
- **[multi_agent.py](examples/multi_agent.py)** — Sequential + Parallel workflows
- **[custom_tools.py](examples/custom_tools.py)** — Python-defined tools with ToolContext

## Running Tests

```bash
pytest pulsehive-py/tests/ -v
```

## License

AGPL-3.0-only
