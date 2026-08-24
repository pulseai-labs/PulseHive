# ADR 005: Public Contracts & Compatibility Policy

**Status:** Accepted
**Category:** 4 - Public contracts & compatibility policy
**Touch Surface:** `pulsehive-*/src/`
**Revisit Trigger:** When breaking changes needed

## Context

PulseHive is published as a library crate with multiple downstream consumers.

## Decision

**Public Contracts:**
- **API stability:** Semantic versioning (MAJOR.MINOR.PATCH)
- **Breaking change policy:** Breaking changes batched into next major version
- **Additive evolution:** Minor releases never break consumers
- **License:** AGPL-3.0-only (open-source option), commercial license available
- **Documentation guarantees:** Public APIs documented with compiling doc-tests

**Compatibility:**
- **Trait contracts:** LlmProvider, StreamingTool, Lens are stable interfaces
- **Event contracts:** HiveEvent enum is `#[non_exhaustive]` (v2.1.0 exception)
- **Binding contracts:** PyO3 (Python) and napi-rs (TypeScript) APIs follow Rust semantics

**Rationale:** Downstream consumers (DevStudio, PulseTrader) require stability. Semantic versioning provides clear compatibility signals. Additive evolution within majors respects consumer upgrade cadence.

## Consequences

**Positive:**
- Predictable upgrade paths for consumers
- Clear backward compatibility guarantees
- Commercial licensing option for proprietary use

**Neutral:**
- Breaking changes require major version bump
- Consumer testing required for minor upgrades

**Negative:**
- Cannot ship breaking changes in minor releases
- Contract changes require careful coordination across bindings
