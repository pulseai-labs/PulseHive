# ADR 009: Stack Choices

**Status:** Accepted
**Category:** 8 - Stack
**Touch Surface:** `Cargo.toml,pulsehive-core/src/lib.rs`
**Revisit Trigger:** When requiring unsafe or changing runtime

## Context

PulseHive is a Rust library with specific technology choices that enable its design goals.

## Decision

**Core Stack:**
- **Language:** Rust (edition 2021)
- **Runtime:** Tokio async runtime
- **Key frameworks:** pulsehive-db 0.5, serde, tokio
- **Storage:** PulseDB (HNSW vector search, knowledge graph, watch system)
- **Build tooling:** Cargo, workspace structure
- **Test tooling:** cargo test, cargo doc (doc-tests)

**Code Quality Constraints:**
- **`#![forbid(unsafe_code)]`:** No unsafe code in library crates
- **Object-safe traits:** Core traits are `Send + Sync`, object-safe
- **Async traits:** `async_trait` where needed for trait methods

**Reversibility:**
- **Reversible:** Dependency versions, specific crates, test frameworks
- **Irreversible:** Rust language choice, Tokio runtime, forbid-unsafe constraint

**Rationale:** Rust provides memory safety and performance. Tokio is the de facto async runtime. `forbid(unsafe_code)` ensures safety guarantees. Object-safe traits enable dynamic dispatch.

## Consequences

**Positive:**
- Memory safety without garbage collector
- Strong type system prevents entire classes of bugs
- No unsafe code in library surface

**Neutral:**
- Async complexity required for LLM integration
- Compilation times inherent to Rust

**Negative:**
- Cannot use unsafe optimizations even if beneficial
- Tokio runtime dependency is hard to replace
- FFI (PyO3, napi-rs) adds complexity
