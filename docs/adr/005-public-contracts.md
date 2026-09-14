# ADR 005: Public Contracts & Compatibility Policy

**Status:** Accepted
**Category:** 4 - Public contracts & compatibility policy
**Touch Surface:** `pulsehive*/src/,pulsehive-js/package.json,pulsehive-js/wrapper.d.ts,pulsehive-py/python/pulsehive/__init__.py`
**Revisit Trigger:** When breaking changes needed

## Context

PulseHive is published as a library crate with multiple downstream consumers.

## Decision

**Public Contracts:**
- **API stability:** Semantic versioning (MAJOR.MINOR.PATCH)
- **Breaking change policy:** Breaking changes batched into next major version
- **Additive evolution:** Minor releases never break consumers (v2.1.0 exception: `HiveEvent` variants were added under `#[non_exhaustive]` without a major bump — see Event contracts below)
- **License:** AGPL-3.0-only (open-source option), commercial license available
- **Documentation guarantees:** Public APIs are documented and `cargo doc` checks the documentation builds; public examples are currently marked `rust,ignore`, so doc-tests do not compile them (wiring examples into compiled doc-tests is tracked in #58)

**Compatibility:**
- **Trait contracts:** Every public trait in PulseHive's published crates is a stable interface: `LlmProvider`, `EmbeddingProvider`, `EventExporter`, `Tool`, `StreamingTool`, `ApprovalHandler`, and `ExperienceExtractor`, all defined in `pulsehive-core` (the only crate defining public traits)
- **Value-type contracts:** Stable public value types include `Lens` — a concrete `pub struct` with public mutable fields (`pulsehive-core/src/lens.rs`), configured by construction and field assignment rather than implemented like a trait
- **Event contracts:** `HiveEvent` enum is `#[non_exhaustive]`; the v2.1.0 exception added variants in a minor release, requiring consumers' `match` arms to keep a wildcard
- **Outcome contracts:** `AgentOutcome` is `#[non_exhaustive]` from Release 1, so consumer `match` expressions on outcomes must keep a wildcard arm; the Python and JavaScript bindings map outcomes to snake_case outcome strings (`complete`, `error`, `max_iterations_reached`, `cancelled`, `partial_complete`) with an `unknown` fallback for variants the binding does not know
- **Binding contracts:** PyO3 (Python) and napi-rs (TypeScript) APIs follow Rust semantics. The JS binding maps `HiveEvent` to `JsHiveEvent` with snake_case event tags, camelCase fields, and a fallback for `HiveEvent` variants the binding does not know that preserves the event rather than discarding it — `event_type` keeps the variant's serde tag and the full event JSON travels under `raw` as a JSON string; the v2.1.0 `#[non_exhaustive]` exception permits additive Rust variants while preserving that fallback

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
