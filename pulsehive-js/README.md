# PulseHive TypeScript/Node.js Bindings

TypeScript/Node.js bindings for the [PulseHive](https://github.com/pulseai-labs/PulseHive) Rust SDK — shared consciousness for multi-agent AI systems.

Get Rust-native performance with TypeScript ergonomics. Define agents, tools, and workflows in TypeScript while the runtime executes in Rust.

## Installation

```bash
npm install @pulsehive/sdk
```

For development:

```bash
cd pulsehive-js
npm install
npm run build:debug
```

## Quickstart

```typescript
const {
  HiveMind, Task,
  JsAgentKind: AgentKind,
  JsAgentDefinition: AgentDefinition,
  JsLens: Lens, JsLlmConfig: LlmConfig,
  openaiProvider,
} = require("@pulsehive/sdk");

async function main() {
  // Build HiveMind with substrate and LLM provider
  const hive = HiveMind.builder()
    .substratePath("/tmp/my_project.db")
    .llmProvider("openai", openaiProvider("sk-...", "gpt-4"))
    .build();

  // Define an agent with perception lens
  const agent = new AgentDefinition("analyzer", AgentKind.llm(
    "You are a code analysis expert.",
    new Lens(["code", "architecture"]),
    new LlmConfig("openai", "gpt-4"),
  ));

  // Deploy and consume events with for-await
  const stream = await hive.deploy([agent], [new Task("Analyze the codebase")]);
  for await (const event of stream) {
    console.log(`[${event.eventType}] ${JSON.stringify(event.data)}`);
    if (event.eventType === "agent_completed") break;
  }

  hive.shutdown();
}

main().catch(console.error);
```

## Custom Tools

Use `defineTool()` for an ergonomic, typed tool definition — no manual JSON serialization needed:

```typescript
const { defineTool } = require("@pulsehive/sdk");

const calculator = defineTool({
  name: "calculator",
  description: "Performs arithmetic operations",
  parameters: {
    type: "object",
    properties: {
      operation: { type: "string", enum: ["add", "subtract", "multiply", "divide"] },
      a: { type: "number" },
      b: { type: "number" },
    },
    required: ["operation", "a", "b"],
  },
  execute: async (params, context) => {
    // params and context are already parsed — no JSON.parse needed
    console.log(`Agent ${context.agentId} calling calculator`);
    const { operation, a, b } = params;
    switch (operation) {
      case "add": return String(a + b);
      case "multiply": return String(a * b);
      default: return `Unsupported: ${operation}`;
    }
  },
});

// Attach tools to agents
const agent = new AgentDefinition("calc-agent", AgentKind.llm(
  "Use tools for math.", new Lens([]), new LlmConfig("openai", "gpt-4"),
  null, // refreshEveryNToolCalls
  [calculator],
));
```

Tools can also return objects (auto-serialized to JSON) and set `requiresApproval: true` for sensitive operations.

## Workflow Agents

```typescript
// Sequential pipeline — each agent perceives previous results
const pipeline = new AgentDefinition("pipeline", AgentKind.sequential([
  new AgentDefinition("step1", AgentKind.llm("Analyze", lens, config)),
  new AgentDefinition("step2", AgentKind.llm("Summarize", lens, config)),
]));

// Parallel execution — agents work concurrently, sharing substrate
const team = new AgentDefinition("team", AgentKind.parallel([agent1, agent2]));

// Loop — repeat until [LOOP_DONE] or max iterations
const loop = new AgentDefinition("iterator", AgentKind.loop(agent, 5));
```

## API Reference

### Core Types

| Type | Description |
|------|-------------|
| `LlmConfig(provider, model)` | LLM selection and generation parameters |
| `Lens(domains)` | Perception filter for substrate access |
| `RecencyCurve.exponential(h)` / `.uniform()` | Temporal decay function |
| `AgentKind.llm(prompt, lens, config, refresh?, tools?)` | LLM-powered agent |
| `AgentKind.sequential(agents)` | Run children in order |
| `AgentKind.parallel(agents)` | Run children concurrently |
| `AgentKind.loop(agent, n)` | Repeat agent N times |
| `AgentDefinition(name, kind)` | Agent blueprint |
| `Task(description)` | Task for deployment |
| `HiveEvent` | Lifecycle event (`.eventType`, `.data`, `.agentId`) |
| `EventStream` | Async event stream (supports `for await`) |
| `ToolContext` | Runtime context for tools (`.agentId`, `.collectiveId`) |
| `ToolResult.text(s)` / `.json(s)` / `.error(s)` | Tool execution result |
| `defineTool(config)` | Ergonomic tool definition with typed callback |

### Provider Factories

```typescript
openaiProvider(apiKey, model?, baseUrl?)  // OpenAI, Azure, Ollama, vLLM
anthropicProvider(apiKey)                  // Anthropic Claude
```

## Examples

See [`examples/`](examples/) for runnable scripts:

- **[getting-started.ts](examples/getting-started.ts)** — Single agent with event stream
- **[multi-agent.ts](examples/multi-agent.ts)** — Sequential + Parallel + Loop workflows
- **[custom-tools.ts](examples/custom-tools.ts)** — defineTool() with typed callbacks

## Running Tests

```bash
npm test
```

## License

AGPL-3.0-only
