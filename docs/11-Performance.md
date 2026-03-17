# PulseHive SDK — Performance Optimization Guide

> **Document ID:** OPS-PH-011
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** SPEC v0.4.0

---

## 1. Performance Philosophy

PulseHive is a Rust SDK with an embedded database. There are no network hops between the application and its storage layer. No REST APIs between components. No serialization/deserialization over the wire. The performance ceiling is defined by CPU, memory bandwidth, and the embedding model — not by infrastructure.

This document covers measured benchmarks, scaling targets, tuning parameters, and strategies for keeping PulseHive fast as experience counts grow.

---

## 2. Benchmarks (Measured)

All benchmarks measured on Apple M-series hardware, release builds, single-threaded unless noted. PulseDB v0.1.1 as the substrate.

### 2.1 Substrate Operations

| Operation | 1K Experiences | Target at 100K | Notes |
|-----------|---------------|----------------|-------|
| `search_similar(k=20)` | 95 us | < 50ms | HNSW approximate nearest neighbor, 384d vectors |
| `get_context_candidates()` | < 10ms | < 100ms | Combined search + filter + sort |
| `store_experience()` | < 5ms | < 10ms | Write + HNSW index update |
| `store_experience()` with embedding | < 50ms | < 60ms | Includes ONNX inference for all-MiniLM-L6-v2 |
| `get_experience()` by ID | < 100 us | < 200 us | Direct key lookup in redb |
| `get_related()` | < 2ms | < 20ms | Graph traversal from single experience |

### 2.2 Intelligence Layer

| Operation | Typical Duration | Notes |
|-----------|-----------------|-------|
| RelationshipDetector (no LLM) | < 5ms | Embedding similarity + pattern matching |
| RelationshipDetector (with LLM) | 500ms-2s | Depends on LLM provider latency |
| InsightSynthesizer | 1-5s | LLM-bound, runs asynchronously |
| ContextOptimizer assembly | < 10ms | Decay computation + token budget packing |
| Lens transformation | < 1ms | Embedding warping + weight multiplication |

### 2.3 End-to-End Agentic Loop

| Phase | Duration | Bottleneck |
|-------|----------|-----------|
| Perceive (substrate query + context assembly) | 10-20ms | Substrate search + decay math |
| Think (LLM call) | 500ms-30s | LLM provider latency (network-bound) |
| Act (tool execution) | Tool-dependent | Product-defined tools |
| Record (experience storage + relationship inference) | 5-60ms | With/without LLM classification |

The LLM call dominates every agentic loop iteration. Substrate operations are three orders of magnitude faster than LLM calls. Optimizing substrate performance is valuable for scaling, but the user-perceived latency is almost entirely LLM-bound.

---

## 3. PulseDB Internals

Understanding PulseDB's internals is essential for tuning PulseHive performance.

### 3.1 Storage Engine: redb

PulseDB uses [redb](https://github.com/cberner/redb), a Rust-native embedded key-value store with memory-mapped files. Key characteristics:

- **Memory-mapped I/O**: The OS kernel manages page caching. Hot data stays in RAM without explicit cache management.
- **Copy-on-write B-tree**: Writes do not block reads. Multiple concurrent readers are supported.
- **ACID transactions**: Every write is durable. No data loss on crash.
- **Zero-copy reads**: Data is read directly from the memory-mapped region. No deserialization for raw bytes.

**Tuning implications:** redb performance scales with available RAM. If the working set (frequently accessed experiences) fits in the OS page cache, reads are effectively memory-speed. If the database file exceeds available RAM, reads may trigger disk I/O.

### 3.2 HNSW Index

PulseDB's vector search uses a Hierarchical Navigable Small World (HNSW) graph for approximate nearest neighbor search on 384-dimensional embeddings.

**Key parameters:**

| Parameter | Default | Effect | Trade-off |
|-----------|---------|--------|-----------|
| `ef_construction` | 200 | Graph quality during index building | Higher = better recall, slower inserts |
| `ef_search` | 100 | Search quality at query time | Higher = better recall, slower queries |
| `m` | 16 | Max connections per node per layer | Higher = better recall, more memory |

**Scaling behavior:**

- Search complexity: O(log N) with HNSW, where N is the number of experiences.
- At 1K experiences: ~95 us per search (measured).
- At 10K experiences: ~500 us per search (projected from HNSW scaling).
- At 100K experiences: ~5-50ms per search (depends on `ef_search` setting).

**When to tune:**

- If search latency exceeds targets, reduce `ef_search` (trades recall for speed).
- If recall is poor (relevant experiences not found), increase `ef_search` and `m`.
- After bulk imports, rebuild the index to optimize graph connectivity.

### 3.3 Embedding Model

PulseDB ships with all-MiniLM-L6-v2 via ONNX Runtime:

- **Dimensions**: 384
- **Inference time**: ~40-50ms per embedding (single text, CPU)
- **Batch inference**: Available but not yet exposed via SubstrateProvider

Embedding computation happens at experience storage time. It does not affect query latency (queries use pre-computed embeddings from the lens or task description).

---

## 4. Tokio Runtime Tuning

PulseHive runs on Tokio's multi-threaded work-stealing scheduler. Correct async usage is critical for performance.

### 4.1 Avoid Blocking in Async Context

The most common performance mistake in PulseHive products:

```rust
// BAD: Blocks the Tokio worker thread
impl Tool for BadTool {
    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let result = std::fs::read_to_string("large_file.txt")?;  // BLOCKS
        Ok(ToolResult::text(result))
    }
}

// GOOD: Use async I/O or spawn_blocking
impl Tool for GoodTool {
    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let result = tokio::fs::read_to_string("large_file.txt").await?;
        Ok(ToolResult::text(result))
    }
}

// GOOD: For CPU-intensive work, use spawn_blocking
impl Tool for CpuTool {
    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let result = tokio::task::spawn_blocking(move || {
            expensive_computation()
        }).await?;
        Ok(ToolResult::text(result))
    }
}
```

Blocking a Tokio worker thread stalls all tasks on that thread, including other agents' LLM streaming and substrate watches. A single blocking call can degrade the entire system.

### 4.2 Agent Task Concurrency

Each agent runs as a Tokio task. Parallel workflow agents spawn sub-tasks concurrently. The Tokio runtime handles scheduling.

**Default configuration**: Tokio uses one worker thread per CPU core. For a 10-core machine, up to 10 agents can execute truly concurrently (CPU-bound work). Since most agent work is I/O-bound (waiting on LLM APIs), hundreds of agents can run concurrently on a single machine.

**When concurrency becomes a problem:**

- Many agents performing CPU-intensive tool work (image processing, data analysis) can saturate worker threads.
- Solution: Use `spawn_blocking` for CPU-bound tool implementations.
- Solution: Configure Tokio with `worker_threads` if the default is insufficient.

```rust
#[tokio::main(worker_threads = 16)]
async fn main() {
    // 16 worker threads instead of the default (CPU cores)
}
```

### 4.3 Substrate Watch Overhead

The PulseDB Watch system uses async channels to notify agents of substrate changes. Each watching agent holds a receiver. Overhead per watcher:

- Memory: ~256 bytes for the channel receiver.
- CPU: Zero when idle. Notification delivery is O(watchers) per substrate write.
- At 100 concurrent watchers: negligible overhead per write.
- At 10,000 concurrent watchers: measure, as channel fan-out may become significant.

---

## 5. Context Budget Optimization

The ContextOptimizer packs experiences and insights into a token budget for LLM calls. Poor budget allocation wastes tokens on low-value context.

### 5.1 Token Budget Allocation

```rust
pub struct ContextBudget {
    pub total_tokens: usize,      // Max tokens for context section
    pub insight_reserve: f32,     // Fraction reserved for insights (default: 0.3)
    pub activity_reserve: f32,    // Fraction reserved for activity awareness (default: 0.1)
    // Remaining fraction goes to experiences
}
```

**Default allocation for a 4K token budget:**

| Category | Tokens | Purpose |
|----------|--------|---------|
| Insights | 1,200 (30%) | High-value synthesized knowledge |
| Activities | 400 (10%) | Awareness of other agents' work |
| Experiences | 2,400 (60%) | Direct experiential knowledge |

### 5.2 Decay Computation Cost

The ContextOptimizer computes decayed importance for every candidate experience:

```rust
decayed = importance * 0.5^(age_hours / half_life) * (1.0 + applications_count * boost)
```

This is a simple floating-point computation: one `powf`, one multiply, one add per experience. At 1K experiences: < 0.1ms. At 100K experiences: ~5ms. Not a bottleneck.

### 5.3 Reducing Context Assembly Time

If context assembly is slow:

1. **Reduce `attention_budget`** in the agent's Lens. Fewer candidates to rank.
2. **Narrow `domain_focus`**. PulseDB filters by domain before PulseHive ranks, reducing the candidate set.
3. **Increase `insight_reserve`**. Insights are pre-synthesized and token-dense. Fewer experiences needed to convey the same knowledge.

---

## 6. Memory Usage

### 6.1 Baseline Overhead

| Component | Memory (Idle) | Notes |
|-----------|--------------|-------|
| HiveMind struct | < 1MB | Coordinator, no large buffers |
| PulseDB substrate | 10-50MB | Memory-mapped file + HNSW index in RAM |
| ONNX Runtime (embedding model) | ~30MB | Loaded once, shared across all agents |
| Per-agent overhead | < 5MB | Task state, message history, tool context |
| EventBus | < 1MB | Channel buffers for HiveEvent distribution |

**Total idle footprint for a HiveMind with PulseDB and one provider: < 50MB.**

### 6.2 Scaling with Experience Count

| Experience Count | Estimated PulseDB Memory | HNSW Index Memory |
|-----------------|-------------------------|-------------------|
| 1K | ~5MB | ~1.5MB (384d * 1K * 16 connections * 4 bytes) |
| 10K | ~50MB | ~15MB |
| 100K | ~500MB | ~150MB |
| 1M | ~5GB | ~1.5GB |

The HNSW index is the primary memory consumer at scale. Each experience stores a 384-dimensional float32 vector (1.5KB) plus graph connectivity data. At 100K experiences, the index alone is ~150MB.

### 6.3 Reducing Memory Usage

- **Lower `m` parameter**: Reduces HNSW graph connectivity. Each connection is 4 bytes * 2 (bidirectional). Reducing `m` from 16 to 8 halves graph memory at the cost of search recall.
- **Archive old experiences**: Move experiences older than a threshold to a separate archive database. Keep the active index lean.
- **Collective isolation**: Each collective maintains its own HNSW index. Large systems should split work across collectives rather than putting everything in one.

---

## 7. Compile Time & Binary Size

### 7.1 Compile Time Targets

| Build Type | Target | Notes |
|------------|--------|-------|
| Clean release build (full workspace) | < 60s | On Apple M-series, `cargo build --release` |
| Incremental debug build | < 10s | After changing a single file |
| `cargo test` (full workspace) | < 30s | Compile + run all tests |

**Strategies for fast compilation:**

- Split workspace into small crates (already done: 5 crates).
- Use `[profile.dev] opt-level = 0` for fast debug builds.
- Avoid procedural macros where possible (they serialize compilation).
- Use `cargo-nextest` for parallelized test execution.

### 7.2 Binary Size

| Configuration | Estimated Size | Notes |
|---------------|---------------|-------|
| Meta-crate + one provider (release) | < 10MB | Stripped, LTO enabled |
| Meta-crate + both providers (release) | < 12MB | Two HTTP clients |
| With PulseDB + ONNX model | +15MB | Embedding model weights |

**Reducing binary size:**

```toml
[profile.release]
lto = true          # Link-time optimization (slower build, smaller binary)
strip = true        # Strip debug symbols
codegen-units = 1   # Single codegen unit (slower build, better optimization)
opt-level = "z"     # Optimize for size instead of speed
```

---

## 8. Performance Regression Testing

### 8.1 Criterion Benchmarks

PulseHive includes criterion benchmarks in each crate's `benches/` directory:

```rust
// pulsehive-runtime/benches/context_assembly.rs
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_context_assembly(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (hive, experiences) = setup_hive_with_experiences(1000);

    c.bench_function("context_assembly_1k", |b| {
        b.iter(|| {
            runtime.block_on(async {
                hive.get_context(&lens, &task, budget).await.unwrap()
            })
        })
    });
}

criterion_group!(benches, bench_context_assembly);
criterion_main!(benches);
```

### 8.2 CI Integration

```yaml
# GitHub Actions: run benchmarks and compare against baseline
benchmark:
  steps:
    - cargo bench --workspace -- --save-baseline current
    - critcmp baseline current --threshold 10
    # Fail CI if any benchmark regresses by more than 10%
```

### 8.3 Benchmark Suite Coverage

| Benchmark | Crate | What It Measures |
|-----------|-------|------------------|
| `substrate_search_1k` | pulsehive-runtime | `search_similar(k=20)` on 1K experiences |
| `substrate_search_10k` | pulsehive-runtime | `search_similar(k=20)` on 10K experiences |
| `context_assembly_1k` | pulsehive-runtime | Full perceive pipeline on 1K experiences |
| `decay_computation_1k` | pulsehive-runtime | ContextOptimizer decay on 1K candidates |
| `lens_transformation` | pulsehive-runtime | Embedding warping through Lens |
| `relationship_detection` | pulsehive-runtime | RelationshipDetector without LLM |
| `experience_store` | pulsehive-runtime | `store_experience()` with pre-computed embedding |
| `experience_store_embed` | pulsehive-runtime | `store_experience()` with ONNX embedding |

---

## 9. Profiling Guide

### 9.1 Flamegraph Profiling

```bash
# Install cargo-flamegraph
cargo install flamegraph

# Profile a specific benchmark
cargo flamegraph --bench context_assembly -- --bench

# Profile the entire application
cargo flamegraph --bin my_product
```

### 9.2 Tokio Console

For diagnosing async runtime issues (task starvation, blocking calls):

```bash
# Install tokio-console
cargo install tokio-console

# Add to product's dependencies
# tokio = { version = "1", features = ["tracing"] }
# console-subscriber = "0.4"

# In main():
console_subscriber::init();

# Run tokio-console in another terminal
tokio-console
```

Tokio Console shows live task states, wake counts, and poll durations. It immediately reveals blocked tasks and scheduling delays.

### 9.3 Memory Profiling

```bash
# Use DHAT for heap profiling (requires nightly)
cargo +nightly run --features dhat-heap

# Or use Instruments on macOS
xcrun xctrace record --template "Allocations" --launch cargo run --release
```

---

## 10. Performance Checklist for Product Developers

When building a product on PulseHive, verify these items:

```
[ ] All Tool implementations use async I/O (no blocking file or network calls)
[ ] CPU-intensive tool work uses tokio::task::spawn_blocking
[ ] Agent Lens has appropriate attention_budget (not unlimited)
[ ] Lens domain_focus is specific enough to filter effectively
[ ] Context budget is sized for the LLM's context window
[ ] Experience archival strategy for long-running collectives
[ ] Release builds use LTO and stripping for production deployment
[ ] Criterion benchmarks exist for product-specific hot paths
```

---

*This document is maintained alongside the SDK. Updated with new benchmark data as the implementation matures.*
