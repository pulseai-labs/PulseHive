# PulseHive SDK — Observability Architecture

> **Document ID:** OPS-PH-010
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** SPEC v0.4.0

---

## 1. Design Philosophy

Observability in PulseHive is a core architectural primitive, not a bolt-on. Every operation in the system — agent lifecycle events, LLM calls, tool executions, substrate operations, perception cycles — emits structured data through two complementary channels:

1. **HiveEvent enum** — A typed event stream returned from `HiveMind::deploy()`, consumed by products for real-time UI, dashboards, and custom logic.
2. **tracing crate integration** — Structured spans and events compatible with the Rust ecosystem's standard observability infrastructure.

There is no proprietary observability platform. No vendor lock-in. Products choose their own subscriber stack, from simple stdout logging to full OpenTelemetry export.

---

## 2. HiveEvent Enum

The `HiveEvent` enum is defined in `pulsehive-core` and covers every observable event in the system.

### 2.1 Agent Lifecycle Events

```rust
HiveEvent::AgentStarted {
    agent_id: AgentId,
    name: String,
    kind: AgentKindTag,    // Llm, Sequential, Parallel, Loop
}

HiveEvent::AgentCompleted {
    agent_id: AgentId,
    outcome: AgentOutcome,  // Success with response, or Error with details
}
```

These events bracket the entire lifetime of an agent. For workflow agents (Sequential, Parallel, Loop), the parent agent emits `AgentStarted`/`AgentCompleted`, and each child agent emits its own pair. This creates a natural hierarchy that products can use to build tree-view UIs.

### 2.2 LLM Interaction Events

```rust
HiveEvent::LlmCallStarted {
    agent_id: AgentId,
    model: String,          // "claude-sonnet-4-6", "glm-5", etc.
    token_count: usize,     // Input tokens (prompt + context)
}

HiveEvent::LlmCallCompleted {
    agent_id: AgentId,
    model: String,
    duration_ms: u64,       // Wall-clock time for the API call
}

HiveEvent::LlmTokenStreamed {
    agent_id: AgentId,
    token: String,          // Individual token for real-time streaming UI
}
```

LLM events are critical for cost tracking and latency monitoring. The `token_count` on `LlmCallStarted` reports the input token count; output tokens are derivable from the count of `LlmTokenStreamed` events between `LlmCallStarted` and `LlmCallCompleted`.

### 2.3 Tool Execution Events

```rust
HiveEvent::ToolCallStarted {
    agent_id: AgentId,
    tool_name: String,
}

HiveEvent::ToolCallCompleted {
    agent_id: AgentId,
    tool_name: String,
    duration_ms: u64,
}

HiveEvent::ToolApprovalRequested {
    agent_id: AgentId,
    tool_name: String,
    action: PendingAction,  // Contains params and description
}
```

Tool events let products track which tools are being used, how long they take, and when human approval is blocking execution. A tool that consistently takes >5 seconds is a performance bottleneck worth investigating.

### 2.4 Substrate Operation Events

```rust
HiveEvent::ExperienceRecorded {
    experience_id: ExperienceId,
    agent_id: AgentId,
}

HiveEvent::RelationshipInferred {
    relation_id: RelationId,
}

HiveEvent::InsightGenerated {
    insight_id: InsightId,
    source_count: usize,    // Number of experiences that contributed
}
```

These events track the growth of the shared consciousness. A healthy multi-agent system should show a steady stream of `ExperienceRecorded` events, periodic `RelationshipInferred` events as the knowledge graph densifies, and occasional `InsightGenerated` events when experience clusters reach the synthesis threshold.

### 2.5 Perception Events

```rust
HiveEvent::SubstratePerceived {
    agent_id: AgentId,
    experience_count: usize,  // Experiences loaded through lens
    insight_count: usize,     // Insights loaded through lens
}
```

Perception events tell you what an agent "saw" when it queried the substrate through its lens. If `experience_count` is consistently 0, the agent's lens may be misconfigured (wrong domain focus, overly narrow attention budget).

---

## 3. Tracing Integration

PulseHive uses the `tracing` crate for structured, hierarchical instrumentation. Every significant operation creates a span with typed fields.

### 3.1 Span Hierarchy

```
hivemind.deploy
  └── agent.run [agent_id, agent_name, agent_kind]
        ├── agent.perceive [experience_count, insight_count, duration_ms]
        ├── agent.think
        │     └── llm.call [model, input_tokens, output_tokens, duration_ms]
        ├── agent.act
        │     └── tool.execute [tool_name, duration_ms, success]
        ├── agent.think  (loop continues until final response)
        │     └── llm.call [...]
        └── agent.record
              ├── substrate.store_experience [experience_id]
              ├── intelligence.detect_relationships [relation_count]
              └── intelligence.synthesize_insights [insight_count]
```

This hierarchy maps directly to the agentic loop: perceive, think, act, record. Products that use distributed tracing (Jaeger, Datadog, Honeycomb) get a flamegraph view of every agent's execution cycle.

### 3.2 Span Fields

Every span includes structured fields that can be queried and aggregated:

| Span | Key Fields |
|------|-----------|
| `agent.run` | `agent_id`, `agent_name`, `agent_kind`, `collective_id` |
| `llm.call` | `model`, `provider`, `input_tokens`, `output_tokens`, `duration_ms` |
| `tool.execute` | `tool_name`, `agent_id`, `duration_ms`, `success` |
| `substrate.store_experience` | `experience_id`, `collective_id`, `experience_type` |
| `substrate.search_similar` | `collective_id`, `k`, `result_count`, `duration_ms` |
| `intelligence.detect_relationships` | `experience_id`, `candidate_count`, `relation_count` |
| `intelligence.synthesize_insights` | `collective_id`, `cluster_count`, `insight_count` |

### 3.3 Event Levels

PulseHive uses tracing levels consistently:

| Level | Usage |
|-------|-------|
| `ERROR` | Unrecoverable failures: LLM provider errors, substrate corruption, tool panics |
| `WARN` | Degraded operation: slow LLM response, empty perception result, tool timeout |
| `INFO` | Significant milestones: agent started/completed, experience recorded, insight generated |
| `DEBUG` | Operational detail: context assembly steps, lens transformation, decay computation |
| `TRACE` | Verbose internals: individual embedding comparisons, token-by-token streaming |

---

## 4. Product-Side Consumption

Products choose how to consume observability data. PulseHive provides the events; products pick the subscriber.

### 4.1 Simple Stdout Logging

```rust
use tracing_subscriber;

// Human-readable logs to stderr
tracing_subscriber::fmt()
    .with_max_level(tracing::Level::INFO)
    .init();

let hive = HiveMind::builder()
    .substrate_path("./project.db")
    .build()?;
```

Output:

```
2026-03-17T10:00:01Z INFO  agent.run{agent_id=a1 agent_name="Researcher"}: started
2026-03-17T10:00:01Z INFO  llm.call{model="claude-sonnet-4-6"}: completed, duration_ms=1200, tokens=450
2026-03-17T10:00:02Z INFO  tool.execute{tool_name="web_search"}: completed, duration_ms=800
2026-03-17T10:00:03Z INFO  substrate.store_experience{experience_id=e42}: recorded
```

### 4.2 JSON Structured Logging

```rust
use tracing_subscriber::fmt::format::json;

tracing_subscriber::fmt()
    .json()
    .with_max_level(tracing::Level::DEBUG)
    .init();
```

Produces machine-parseable JSON lines suitable for log aggregation (ELK, Loki, CloudWatch):

```json
{"timestamp":"2026-03-17T10:00:01Z","level":"INFO","span":"llm.call","fields":{"model":"claude-sonnet-4-6","input_tokens":1200,"output_tokens":450,"duration_ms":1200}}
```

### 4.3 OpenTelemetry Export

```rust
use tracing_opentelemetry::OpenTelemetryLayer;
use opentelemetry_otlp::new_exporter;

let tracer = opentelemetry_otlp::new_pipeline()
    .tracing()
    .with_exporter(new_exporter().tonic().with_endpoint("http://localhost:4317"))
    .install_batch(opentelemetry_sdk::runtime::Tokio)?;

let telemetry = OpenTelemetryLayer::new(tracer);

tracing_subscriber::registry()
    .with(telemetry)
    .with(tracing_subscriber::fmt::layer())
    .init();
```

This sends spans and events to any OpenTelemetry-compatible backend: Jaeger, Datadog, Honeycomb, Grafana Tempo, AWS X-Ray, or a self-hosted collector. No PulseHive-specific code needed.

### 4.4 HiveEvent Stream for Custom Dashboards

```rust
let mut stream = hive.deploy(agents, tasks).await?;

while let Some(event) = stream.next().await {
    match event {
        HiveEvent::LlmTokenStreamed { token, .. } => {
            // Real-time chat UI
            print!("{}", token);
        }
        HiveEvent::AgentCompleted { outcome, .. } => {
            dashboard.update_agent_status(outcome);
        }
        HiveEvent::ExperienceRecorded { experience_id, .. } => {
            dashboard.increment_experience_counter();
        }
        HiveEvent::InsightGenerated { source_count, .. } => {
            dashboard.show_notification(
                format!("New insight from {} experiences", source_count)
            );
        }
        _ => {}
    }
}
```

The `Stream<Item = HiveEvent>` is the primary interface for products that need real-time UI updates. It runs alongside tracing (not instead of it).

---

## 5. Key Metrics

These are the metrics that matter for monitoring PulseHive-powered systems. Products should track these and set alerts on the ones relevant to their SLAs.

### 5.1 Latency Metrics

| Metric | Target (1K experiences) | Target (100K experiences) | Source |
|--------|------------------------|--------------------------|--------|
| `substrate_search_duration` | < 1ms | < 50ms | `substrate.search_similar` span |
| `context_assembly_duration` | < 10ms | < 100ms | `agent.perceive` span |
| `llm_call_duration` | Provider-dependent | Provider-dependent | `llm.call` span |
| `tool_execution_duration` | Tool-dependent | Tool-dependent | `tool.execute` span |
| `experience_recording_duration` | < 15ms | < 20ms | `substrate.store_experience` span |

### 5.2 Throughput Metrics

| Metric | Description | Source |
|--------|-------------|--------|
| `experience_count` | Total experiences in collective | `ExperienceRecorded` events |
| `insight_count` | Total derived insights | `InsightGenerated` events |
| `relation_count` | Total inferred relationships | `RelationshipInferred` events |
| `llm_calls_total` | Total LLM API calls | `LlmCallCompleted` events |
| `tool_calls_total` | Total tool executions | `ToolCallCompleted` events |

### 5.3 Cost Metrics

| Metric | Description | Source |
|--------|-------------|--------|
| `token_usage_input` | Total input tokens sent to LLM providers | `LlmCallStarted.token_count` sum |
| `token_usage_output` | Total output tokens received | `LlmTokenStreamed` count between call boundaries |
| `token_usage_by_model` | Token usage broken down by model | `LlmCallStarted.model` field |
| `token_usage_by_agent` | Token usage broken down by agent | `LlmCallStarted.agent_id` field |

---

## 6. Debugging the Agentic Loop

When an agent produces unexpected results, trace through the perceive-think-act-record cycle:

### Step 1: Check Perception

Look at the `agent.perceive` span and the `SubstratePerceived` event.

- **experience_count = 0**: The agent's lens is not matching any experiences. Check `domain_focus`, `attention_budget`, and whether the collective has any stored experiences.
- **experience_count very high but irrelevant**: The lens is too broad. Narrow the `domain_focus` or reduce `attention_budget`.
- **Missing recent experiences**: Check the `recency_curve` configuration. An aggressive `Exponential` decay might be suppressing recent entries if their `importance` is low.

### Step 2: Check LLM Input

Enable `TRACE` level logging to see the full prompt sent to the LLM, including the assembled context. Verify that:

- The system prompt is correct.
- Substrate context is being presented as intrinsic knowledge ("You understand that...").
- The token budget is not being exceeded (causing context truncation).

### Step 3: Check Tool Execution

Review `tool.execute` spans for failures, slow execution, or unexpected parameters.

- **Tool taking too long**: The tool implementation may be making blocking network calls. Ensure tools use async I/O.
- **Tool returning errors**: Check the `ToolResult` in the span fields. Common issue: tools returning errors that the LLM cannot recover from.

### Step 4: Check Experience Recording

Verify that `ExperienceRecorded` events fire after agent completion.

- **No experiences recorded**: The experience extractor may not be finding extractable content. Check whether the agent's response contains actionable patterns.
- **Relationships not inferred**: The `RelationshipDetector.auto_threshold` may be too high. Lower it or check that the embedding model is producing meaningful similarity scores.

---

## 7. Why Not LangSmith?

LangSmith (LangChain's observability platform) is a proprietary SaaS product. PulseHive explicitly avoids this pattern, and the reasoning applies to any vendor-specific observability platform.

| Factor | LangSmith Approach | PulseHive Approach |
|--------|-------------------|-------------------|
| **Lock-in** | Requires LangSmith account and API key | Uses `tracing` crate, any subscriber works |
| **Cost** | Per-trace pricing at scale | Zero cost (you own the infrastructure) |
| **Data sovereignty** | Traces sent to third-party servers | Traces stay where you configure them |
| **Flexibility** | LangSmith dashboard or nothing | Jaeger, Datadog, Honeycomb, Grafana, stdout, custom |
| **SDK coupling** | Deeply integrated into LangChain internals | Cleanly separated via standard tracing interface |
| **Offline support** | Requires internet connection | Works fully offline (stdout, file-based subscribers) |

The `tracing` crate is the Rust ecosystem's standard for structured, composable observability. It has first-class support in Tokio, Tower, Hyper, Axum, and every major Rust framework. By building on `tracing`, PulseHive gets compatibility with the entire ecosystem for free.

Products that want a LangSmith-like experience can use Grafana + Tempo + Loki with the OpenTelemetry exporter. Products that want simplicity can use stdout. Products that want custom dashboards consume the `HiveEvent` stream directly. The SDK does not impose a choice.

---

## 8. Observability Configuration Reference

### Runtime Configuration

```rust
let hive = HiveMind::builder()
    .substrate_path("./project.db")
    .llm_provider("anthropic", AnthropicProvider::new(api_key))
    // Observability is always on — no opt-in needed
    // Configure what you consume, not what PulseHive emits
    .build()?;
```

### Environment Variables for Tracing

```bash
# Control tracing verbosity per module
RUST_LOG=pulsehive=info,pulsehive_runtime::loop=debug

# Enable all PulseHive tracing at debug level
RUST_LOG=pulsehive=debug

# Verbose: see embedding comparisons and token streaming
RUST_LOG=pulsehive=trace
```

### Filtering by Span

Products can create tracing subscribers that filter specific spans:

```rust
use tracing_subscriber::filter::EnvFilter;

let filter = EnvFilter::new("pulsehive=info")
    .add_directive("pulsehive_runtime::intelligence=debug".parse()?)
    .add_directive("pulsehive_runtime::loop=trace".parse()?);

tracing_subscriber::fmt()
    .with_env_filter(filter)
    .init();
```

This gives fine-grained control: INFO for general operations, DEBUG for intelligence algorithms, TRACE for the agentic loop internals.

---

*This document is maintained alongside the SDK. Updated with each change to the observability architecture or HiveEvent enum.*
