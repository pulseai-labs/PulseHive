# ADR 008: Rollback & Evolution Strategy

**Status:** Accepted
**Category:** 7 - Rollback & evolution strategy
**Touch Surface:** `Cargo.toml,CHANGELOG.md`
**Revisit Trigger:** When introducing breaking migration

## Context

PulseHive is published to crates.io as versioned releases with multiple downstream consumers.

## Decision

**Rollback Strategy:**
- **Crates.io versioned releases:** Each version is permanently published
- **Semantic versioning:** MAJOR.MINOR.PATCH indicates breaking/feature/fix level
- **Rollback mechanism:** Consumers downgrade to the previous published version. The rollback release unit is the coordinated set of seven crates published together (`pulsehive`, `pulsehive-core`, `pulsehive-runtime`, `pulsehive-openai`, `pulsehive-anthropic`, `pulsehive-py`, `pulsehive-js`); a rollback must use exact matching version constraints (e.g. `=x.y.z`) across the set, and consumers must rely on their own `Cargo.lock` for reproducible resolution. Rollback for the PyPI and npm bindings is not specified here (tracked in #59)
- **Deprecation policy:** Deprecated features persist for one major version cycle

**Evolution Strategy:**
- **Additive changes preferred:** New features added without breaking existing code
- **Breaking changes:** Batched into major version releases with migration guide
- **Compatibility shims:** Temporary adapters for major transitions (e.g., v2→v3)
- **CHANGELOG.md:** Document all changes per release

**Load-bearing vs Replaceable:**
- **Load-bearing:** Five primitives, PulseDB boundary, AGPL-3.0-only licensing (open-source option, commercial license available; deliberately fixed)
- **Replaceable:** Provider implementations, tool implementations, specific algorithms

**Rationale:** Semantic versioning provides clear rollback signals. Permanent publication enables downgrades. Additive evolution respects consumer upgrade cycles.

## Consequences

**Positive:**
- Consumers can rollback by downgrading crates.io version
- Clear upgrade paths via semantic versioning
- Deprecation enables graceful migration

**Neutral:**
- Breaking changes require major version bump
- Consumers must actively upgrade to receive fixes

**Negative:**
- Cannot remove deprecated features quickly
- Breaking changes require coordinated migration across ecosystem
