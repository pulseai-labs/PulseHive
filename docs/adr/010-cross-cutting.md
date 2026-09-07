# ADR 010: Cross-Cutting Constraints

**Status:** Accepted
**Category:** 9 - Cross-cutting constraints
**Touch Surface:** `pulsehive-core/src/`
**Revisit Trigger:** When considering sixth primitive or license change

## Context

PulseHive has fundamental architectural constraints that apply across all modules.

## Decision

**Cross-Cutting Constraints:**

**Five Primitives Hard Cap:**
- Exactly five primitives: HiveMind, Agent, Tool, Lens, Experience
- No sixth primitive without demonstrated use case
- Enforced via architecture review and code review

**Object-Safe Traits:**
- Core traits are `Send + Sync`, object-safe (`Arc<dyn Trait>`)
- Enables dynamic dispatch and trait objects
- `as_*() -> Option<&dyn Ext>` pattern for capability extensions

**Licensing:**
- AGPL-3.0-only (open-source license in source)
- Commercial license option available
- Dual licensing model enforced

**Substrate Boundary:**
- PulseDB strict separation enforced
- PulseHive cannot implement storage primitives
- All storage goes through substrate abstraction

**Provider Abstraction:**
- LLM access through `LlmProvider` trait only
- No hardcoded provider selection or transport behavior in core
- Provider crates may implement `LlmProvider` (e.g. `pulsehive-openai`)
- Pluggable provider model

**Rationale:** These constraints keep PulseHive small, composable, and maintainable. The five-primitive boundary prevents framework bloat. Object-safe traits enable dynamic composition. AGPL ensures open-source contributions remain open.

## Consequences

**Positive:**
- Framework remains small enough to reason about
- Clear boundaries prevent scope creep
- Provider abstraction enables multiple backends
- AGPL protects open-source investments

**Neutral:**
- New primitive requires strong justification process
- Object-safe trait requirements limit some API designs
- AGPL may limit commercial adoption without commercial license

**Negative:**
- Cannot quickly add primitives for new use cases
- AGPL incompatible with some proprietary integration requirements
- Object-safe constraints prevent some generic patterns
