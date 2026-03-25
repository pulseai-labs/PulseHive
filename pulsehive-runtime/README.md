# pulsehive-runtime

**Runtime execution engine for the PulseHive multi-agent SDK.**

This crate provides the HiveMind orchestrator, agentic loop (Perceive→Think→Act→Record), workflow agents (Sequential, Parallel, Loop), and the intelligence layer (relationship detection, insight synthesis, context optimization).

> **Most users should use the [`pulsehive`](https://crates.io/crates/pulsehive) meta-crate** which re-exports this crate alongside `pulsehive-core`.

## Key Components

### HiveMind Orchestrator

```rust
let hive = HiveMind::builder()
    .substrate_path("my_project.db")
    .llm_provider("openai", provider)
    .embedding_provider(my_embeddings)  // optional
    .build()?;

let stream = hive.deploy(agents, tasks).await?;
```

### Workflow Agents

| Kind | Behavior |
|------|----------|
| `AgentKind::Llm` | Single LLM agent with tools and lens-based perception |
| `AgentKind::Sequential` | Children execute in order, each perceiving previous results |
| `AgentKind::Parallel` | Children execute concurrently, sharing substrate in real-time |
| `AgentKind::Loop` | Repeats agent until `[LOOP_DONE]` or max iterations |

### Intelligence Layer

| Component | Purpose |
|-----------|---------|
| `RelationshipDetector` | Infers semantic relations between experiences |
| `InsightSynthesizer` | LLM-powered cluster synthesis into derived insights |
| `ContextOptimizer` | 72-hour exponential decay + reinforcement boost |
| `AttractorDynamics` | Field-level perception warping by high-importance experiences |

### Perception Pipeline

Each agent perceives the substrate through its Lens:

1. **Query** — semantic search or recent experiences
2. **Re-rank** — domain weight, type weight, temporal decay, attractor influence
3. **Budget** — pack within token limit
4. **Format** — render as intrinsic knowledge for the LLM

## Performance

| Operation | 1K experiences | 10K experiences |
|-----------|---------------|-----------------|
| `search_similar(k=20)` | 200 µs | 279 µs |
| `get_recent(k=20)` | 95 µs | 588 µs |
| `store_experience` | 7.0 ms | — |
| Perception rerank (100 exp) | 26 µs | — |

## Links

- [pulsehive (meta-crate)](https://crates.io/crates/pulsehive)
- [API Docs](https://docs.rs/pulsehive-runtime)
- [GitHub](https://github.com/pulsehive/pulsehive)
- [Benchmarks](https://github.com/pulsehive/pulsehive/blob/main/docs/benchmarks.md)

## License

AGPL-3.0-only
