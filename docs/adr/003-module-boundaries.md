# ADR 003: Module Boundaries & Dependency Direction

**Status:** Accepted
**Category:** 2 - Module boundaries & dependency direction
**Touch Surface:** `pulsehive/src/,pulsehive-core/src/,pulsehive-runtime/src/,pulsehive-openai/src/,pulsehive-anthropic/src/,pulsehive-py/src/,pulsehive-js/src/`
**Revisit Trigger:** When adding sixth primitive or blurring PulseDB boundary

## Context

PulseHive is organized around five core primitives with strict separation from the substrate layer.

## Decision

**Module Boundaries:**
- **Five primitives hard cap:** HiveMind, Agent, Tool, Lens, Experience
- **PulseDB strict separation:** PulseHive owns intelligence layer; PulseDB owns storage
- **Dependency direction:** PulseHive → PulseDB (never reverse)
- **Primitive homes:** `pulsehive-core` defines the Agent and Tool interfaces and the concrete `Lens` value type; `HiveMind` and its builder are defined in `pulsehive-runtime/src/hivemind.rs`; `Experience` is defined by PulseDB and re-exported by `pulsehive-core`
- **Provider modules:** `pulsehive-anthropic`, `pulsehive-openai` implement LlmProvider trait
- **Runtime module:** `pulsehive-runtime` implements agentic loop and workflow execution

**Rationale:** The five-primitive boundary keeps the framework small enough to reason about. PulseDB separation ensures substrate independence. Dependency direction prevents tight coupling to storage implementation.

## Consequences

**Positive:**
- Clear, bounded surface area
- Substrate portability (PulseDB can evolve independently)
- Provider abstraction enables multiple LLM backends

**Neutral:**
- New primitive requires strong justification
- Provider changes isolated to specific crates

**Negative:**
- Cannot add sixth primitive without demonstrated need
- PulseDB cannot depend on PulseHive features
