# pulsehive-core

**Core traits and types for the PulseHive multi-agent SDK.**

This crate defines the public API surface — all traits, type definitions, and error types used across the PulseHive ecosystem. It has zero runtime dependencies on LLM providers or HTTP clients.

> **Most users should use the [`pulsehive`](https://crates.io/crates/pulsehive) meta-crate** instead of depending on this crate directly. Use `pulsehive-core` only if you're implementing a custom LLM provider or embedding provider.

## Core Traits

| Trait | Purpose |
|-------|---------|
| [`LlmProvider`](https://docs.rs/pulsehive-core/latest/pulsehive_core/llm/trait.LlmProvider.html) | LLM chat + streaming interface |
| [`Tool`](https://docs.rs/pulsehive-core/latest/pulsehive_core/tool/trait.Tool.html) | Pluggable agent capability (name, description, parameters, execute) |
| [`EmbeddingProvider`](https://docs.rs/pulsehive-core/latest/pulsehive_core/embedding/trait.EmbeddingProvider.html) | Domain-specific embedding models |
| [`ApprovalHandler`](https://docs.rs/pulsehive-core/latest/pulsehive_core/approval/trait.ApprovalHandler.html) | Human-in-the-loop approval for tool execution |
| [`ExperienceExtractor`](https://docs.rs/pulsehive-core/latest/pulsehive_core/agent/trait.ExperienceExtractor.html) | Custom experience extraction from conversations |

## Key Types

| Type | Description |
|------|-------------|
| `AgentDefinition` | Agent blueprint (name + kind) |
| `AgentKind` | Llm, Sequential, Parallel, or Loop |
| `Lens` | Perception filter (domains, type weights, recency curve, attention budget) |
| `LlmConfig` | Provider + model + temperature + max_tokens |
| `HiveEvent` | 14 lifecycle event variants for observability |
| `ToolResult` | Text, Json, or Error result from tool execution |
| `PulseHiveError` | Top-level error enum (Substrate, Llm, Tool, Agent, Config, Validation, Embedding) |

## Implementing a Custom Provider

```rust
use async_trait::async_trait;
use pulsehive_core::llm::*;
use pulsehive_core::error::Result;

struct MyProvider;

#[async_trait]
impl LlmProvider for MyProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<LlmResponse> {
        // Your LLM API call here
        todo!()
    }

    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: &LlmConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk>> + Send>>> {
        todo!()
    }
}
```

## Links

- [pulsehive (meta-crate)](https://crates.io/crates/pulsehive)
- [API Docs](https://docs.rs/pulsehive-core)
- [GitHub](https://github.com/pulseai-labs/PulseHive)

## License

AGPL-3.0-only
