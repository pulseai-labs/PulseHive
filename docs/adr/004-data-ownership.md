# ADR 004: Data Ownership & Migration Posture

**Status:** Accepted
**Category:** 3 - Data ownership & migration posture
**Touch Surface:** `pulsehive-runtime/src/`
**Revisit Trigger:** When PulseHive starts owning persistence or bypassing the substrate abstraction

## Context

PulseHive processes agent experiences but does not own the persistent storage layer.

## Decision

**Data Ownership:**
- **PulseDB owns storage:** Vectors, graph, watch system, context assembly
- **PulseHive owns intelligence:** Attractor dynamics, lens warping, conflict reasoning, insight synthesis
- **Consumer owns PulseDB instance:** Substrate path provided by consumer
- **Identifier vs storage ownership:** identifier ownership does not transfer storage ownership — PulseHive owns the typed handles (`CollectiveId`, `ExperienceId`, `InsightId`, `RelationId`) used by its public contracts; PulseDB owns persisted records and storage behavior; `pulsehive-runtime` performs lossless ID conversion only at the adapter boundary (ADR-013)
- **Execution location, not ownership:** PulseHive does not own persistence, but it does run the database engine in-process — `HiveMindBuilder::build()` opens PulseDB at the consumer-configured substrate path inside the consumer's process and wraps it in `PulseDBSubstrate` (`pulsehive-runtime/src/hivemind.rs`)

**Migration Posture:** Expand/contract via PulseDB
- PulseHive operates through substrate abstraction
- Schema changes managed at PulseDB level
- PulseHive queries through versioned substrate interface

**Rationale:** Clear ownership boundaries prevent responsibility confusion. PulseHive's intelligence layer operates on data PulseDB owns and persists. Consumers control substrate lifecycle.

## Consequences

**Positive:**
- Clear separation of concerns
- PulseDB can evolve independently
- Consumers control persistence lifecycle

**Neutral:**
- PulseHive cannot define its own persistence schema
- Migration complexity delegated to substrate

**Negative:**
- PulseHive cannot optimize storage for specific intelligence operations
- Substrate bugs affect PulseHive (dependency risk)
