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
- **Test tooling:** cargo test (including doc-tests), cargo doc (documentation checks)

**Code Quality Constraints:**
- **Unsafe-free library crates (intent):** Library crates are to stay free of `unsafe` code; `pulsehive-py`'s `unsafe impl Send`/`Sync` for its Python tool bridge is the known binding exception. No crate currently carries `#![forbid(unsafe_code)]`, so this is recorded as intent — lint enforcement is tracked in #55
- **Object-safe traits:** Core traits are `Send + Sync`, object-safe
- **Async traits:** `async_trait` where needed for trait methods

**Reversibility:**
- **Reversible:** Dependency versions, specific crates, test frameworks
- **Irreversible:** Rust language choice, Tokio runtime, unsafe-free constraint

**Rationale:** Rust provides memory safety and performance. Tokio is the de facto async runtime. The unsafe-free intent keeps safety guarantees in library code, with `pulsehive-py`'s Send/Sync impls as the accepted binding exception (enforcement tracked in #55). Object-safe traits enable dynamic dispatch.

## Consequences

**Positive:**
- Memory safety without garbage collector
- Strong type system prevents entire classes of bugs
- Library crates stay unsafe-free by intent (binding exception: `pulsehive-py` Send/Sync; enforcement tracked in #55)

**Neutral:**
- Async complexity required for LLM integration
- Compilation times inherent to Rust

**Negative:**
- Cannot use unsafe optimizations even if beneficial
- Tokio runtime dependency is hard to replace
- FFI (PyO3, napi-rs) adds complexity
