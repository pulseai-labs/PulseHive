# ADR 001: Ossify Adoption

**Status:** Accepted
**Date:** 2026-08-23
**Context:** Adoption ceremony from scaffold-dev to ossify 1.0.3

## Context

PulseHive was developed using the scaffold-dev workflow (sprint → slice → work-item loop). As the project matured beyond v2.0.1 with multiple slices planned for v2.1.0, the need arose for:
- Formal architectural decision tracking (bones registry)
- Release-level planning with exit criteria
- Ceremony-driven development with fail-closed gates
- Retrospective recording and harvest discipline

## Decision

Adopt ossify 1.0.3 as the development workflow framework, retiring scaffold-dev.

### Adoption Details

**Baseline:** Release 0, closed retroactively at SHA 60503db
- v2.0.1 shipped: five primitives, agentic loop, intelligence layer, providers, bindings
- VS-1.1.1 shipped: streaming tool execution + ToolProgress events

**Transition:**
- Sprint 1.1 closed at 1/4 completion (VS-1.1.1 merged via PR #44)
- Remaining slices (VS-1.1.2/1.1.3/1.1.4) handed to Release 1 for planning
- scaffold-dev ceremonies retired
- ossify ceremonies activated (`/ossify:*` slash commands)

**Rationale:**
- Ossify provides ADR-backed bones (nine categories with touch surfaces)
- Release planning with feature mapping and exit criteria
- Fail-closed gates at every ceremony (adoption, planning, execution, close)
- Retrospective discipline with harvest → known-issues/decisions-log

### Consequences

**Positive:**
- Structured release planning with `/ossify:plan-release`
- Architectural decisions tracked as bones with touch glob triggers
- Demo-ledger discipline (seed candidates recorded, exercised at spine close)
- State-driven workflow (project-state.json as source of truth)

**Neutral:**
- Learning curve for ossify ceremonies
- Legacy scaffold-dev stack preserved but inactive
- AI workspace artifacts maintained (memory-bank, MASTER-SPEC.md)

**Negative:**
- No ADR directory existed at adoption (created, bones back-derivation skipped)
- Sprint 1.1 partial completion required explicit handoff to Release 1

## Implementation

**Adoption Record:** See `/Users/draco/projects/PulseHive/pulsehive-ai/ADOPTION.md`

**Next Steps:**
- `/ossify:plan-release` for Release 1 feature mapping
- Plan first spine with VS-1.1.2 (cancellation infrastructure)
- Author bones forward starting with Release 1

## References

- Ossify 1.0.3 adoption ceremony: `/ossify:adopt`
- Build session protocol: claude-agent-scaffolding-ai-52
- Baseline SHA: 60503db38514a4a4d5d6e6405a6dc2465dd9744a
