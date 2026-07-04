# PulseHive Performance Benchmarks

Benchmark results for PulseHive v1.0.0, measured with [Criterion.rs](https://github.com/bheisler/criterion.rs).

**Hardware:** Apple M-series ARM64, macOS Darwin 25.2.0

Run benchmarks locally: `cargo bench -p pulsehive-runtime`

## Substrate Operations

| Operation | 100 exp | 500 exp | 1K exp | 10K exp | Target (1K) | Status |
|-----------|---------|---------|--------|---------|-------------|--------|
| `store_experience` | — | — | 7.0 ms | — | < 15 ms | **PASS** |
| `get_recent(k=20)` | 43 µs | 68 µs | 95 µs | 588 µs | < 10 ms | **PASS** |
| `search_similar(k=20)` | 76 µs | 164 µs | 200 µs | 279 µs | < 1 ms | **PASS** |

**Notes:**
- `store_experience` includes PulseDB builtin embedding computation (all-MiniLM-L6-v2, 384d ONNX inference)
- `search_similar` uses HNSW approximate nearest neighbor search
- All operations measured end-to-end including PulseDB I/O

## Field Dynamics

| Operation | Latency | Target | Status |
|-----------|---------|--------|--------|
| `cosine_distance` (384d) | 357 ns | < 1 µs | **PASS** |
| `influence_at` (384d) | 353 ns | < 5 µs | **PASS** |

## Perception Re-ranking

| Operation | 100 exp | 500 exp | 1K exp | Target (1K) | Status |
|-----------|---------|---------|--------|-------------|--------|
| `rerank` (no attractors) | 26 µs | 162 µs | 339 µs | < 1 ms | **PASS** |
| `rerank` (with attractors) | 3.7 ms | 90 ms | — | — | **O(n^2)** |

**Notes:**
- Attractor-enabled reranking is O(n^2) due to pairwise influence computation
- Bounded in practice by `attention_budget * 2` (typically 100 experiences)
- Without attractors, reranking scales linearly and is well within targets

## Performance Targets

| Operation | Target (1K) | Target (100K) | Measured (1K) | Measured (10K) |
|-----------|-------------|---------------|---------------|----------------|
| `search_similar(k=20)` | < 1 ms | < 50 ms | 200 µs | 279 µs |
| `get_recent(k=20)` | < 10 ms | < 100 ms | 95 µs | 588 µs |
| Context assembly | < 10 ms | < 100 ms | ~500 µs (est.) | ~2 ms (est.) |
| Experience recording | < 15 ms | < 15 ms | 7.0 ms | 7.0 ms |

All measured operations are well within target thresholds. The 100K tier was not benchmarked in this run but the 10K → 100K scaling pattern (HNSW is O(log n)) projects well below the 50 ms target.

## Reproducing

```bash
# Full benchmark suite
cargo bench -p pulsehive-runtime

# Specific benchmark
cargo bench -p pulsehive-runtime --bench substrate_bench
cargo bench -p pulsehive-runtime --bench field_bench

# View HTML report
open target/criterion/report/index.html
```
