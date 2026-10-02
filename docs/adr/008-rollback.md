# ADR 008: Rollback & Evolution Strategy

**Status:** Accepted
**Category:** 7 - Rollback & evolution strategy
**Touch Surface:** `Cargo.toml,pulsehive*/Cargo.toml,.github/workflows/crates-release.yml,CHANGELOG.md`
**Revisit Trigger:** When introducing breaking migration

## Context

PulseHive is published to crates.io as versioned releases with multiple downstream consumers.

## Decision

**Rollback Strategy:**
- **Crates.io versioned releases:** Each version is permanently published
- **Semantic versioning:** MAJOR.MINOR.PATCH indicates breaking/feature/fix level
- **Rollback mechanism:** Consumers downgrade to the previous published version. The rollback release unit is the five crates jointly published to crates.io (`pulsehive`, `pulsehive-core`, `pulsehive-runtime`, `pulsehive-openai`, `pulsehive-anthropic`); a rollback must use exact matching version constraints (e.g. `=x.y.z`) across the five, and consumers must rely on their own `Cargo.lock` for reproducible resolution. The PyPI Python artifact joins the workspace version line under ADR-016 L1 (the retained `0.3.0b2` is a historical publication, not that line), and the npm `@pulsehive/sdk` package is bound to the same line under ADR-015 L1; both are binding artifacts whose rollback is not specified here (tracked in #59)
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

## Amendment — r2.s3 (ADR-018)

**The release unit is per registry.** One `v*` tag publishes three independently
approved sets: the five crates jointly to crates.io, `@pulsehive/sdk` with its
three platform packages to npm, and the `pulsehive` wheel set to PyPI. Each set
is bound to the bytes that passed the gate by ADR-018's identity checks
(`SHA256SUMS` and the index `cksum` for crates.io, `dist.integrity` for npm,
`digests.sha256` for PyPI), and a registry whose bytes differ is refused rather
than overwritten.

**Recovery is ADR-018's, not a new rollback mechanism.** A transient failure is
recovered by re-running the failed jobs of the original tag run (the only path
that reuses the tested artifacts); `workflow_dispatch` on the tag is a fresh
build and is legitimate only for a registry with nothing published for that
version; past GitHub's 30-day re-run window a partially published registry goes
corrective — fix on `main`, bump the patch version, cut a new tag — and a
corrective release moves all three registries to the new version together.
Versions already published stay published; nothing is deleted or moved.

**Completion is recorded, not assumed.** A release is complete only when every
registry the tag owns has published or carries a recorded disposition (resume,
abandon or corrective) in the per-registry table of its GitHub Release notes.
The operator decides the disposition, and `docs/RELEASING.md` says what
consumers are told meanwhile.

**Binding rollback stays with #59.** This amendment records the publish-time
unit, recovery and completion policy only; how a consumer is told to pin around
a bad release is #59's.
