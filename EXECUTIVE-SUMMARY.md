# PulseHive — Executive Summary

**PulseHive is a Rust SDK for building multi-agent AI systems whose agents share consciousness through a persistent substrate (PulseDB) instead of passing messages.** It is a framework — comparable to LangChain/LangGraph — **not** a product. Products are built *on* PulseHive (dev-automation tools, trading coaches, research engines, personal assistants); the first vertical product and a downstream consumer live in separate repos.

## What it is

Agents perceive and contribute to a common substrate of **experiences, insights, and relationships** rather than exchanging point-to-point messages. The division of labor is strict: **PulseDB stores and retrieves** (HNSW vector search, knowledge graph, real-time watch, context assembly); **PulseHive thinks** (attractor dynamics, lens-warped perception, conflict reasoning, insight synthesis). Knowledge is cumulative and queryable across agents and runs.

## Five primitives (hard cap)

| Primitive | Role |
|-----------|------|
| **HiveMind** | Orchestrator — owns the substrate, named providers, event stream; deploys/runs agents |
| **Agent** | LlmAgent (reasoning + tools + lens) or WorkflowAgent (Sequential/Parallel/Loop) |
| **Tool** | Pluggable domain capability; products implement |
| **Lens** | Perception filter — how an agent sees the substrate |
| **Experience** | Knowledge unit stored in PulseDB, shared across agents |

No sixth primitive without a demonstrated, repo-backed use case. Enums over strings; builder patterns; never panic in library code; every public API has a compiling doc-test.

## Status & direction

- **Shipped (v2.0.1, AGPL-3.0, crates.io):** five primitives, agentic loop, workflow agents, intelligence layer, Anthropic + OpenAI-compatible providers, `HiveEvent` stream + `EventExporter`, PulseDB integration, Python (PyO3) + TypeScript (napi-rs) bindings.
- **Next (v2.1.0 — Sprint 1, additive):** streaming tool execution + `ToolProgress` events; cooperative cancellation; subscription-billed subprocess providers (Claude Code + Codex), stateless then stateful. One coordinated `#[non_exhaustive]` migration absorbs the only contract change.
- **Deferred (v3.0):** breaking work (e.g. `TokenUsage` → tagged `Usage` enum).

## Success proof

A product author can stand up a multi-agent collective — deploy two agents against a shared substrate, have one perceive an experience the other recorded, and run a full perceive→think→act→record loop — in under ~50 lines of Rust, with sub-millisecond similarity search at 1K experiences.
