# ADR 002: System Shape & Deployment Topology

**Status:** Accepted
**Category:** 1 - System shape & deployment topology
**Touch Surface:** `pulsehive*/src/`
**Revisit Trigger:** When spawning separate runtime processes

## Context

PulseHive is a library/SDK, not a deployed service. It runs in the caller's process.

## Decision

**System Shape:** In-process library SDK
- PulseHive runs in the calling application's process
- No separate deployment topology
- No client-server split within PulseHive itself
- Deployment unit is the crate (published to crates.io)

**Rationale:** As an SDK, PulseHive's runtime is embedded in consumer applications. Splitting into separate deployables would require measured pressure that the product imposes, not anticipatory scaling.

## Consequences

**Positive:**
- Simple deployment model - consumers depend on crates.io published version
- No inter-process communication complexity
- Direct access to PulseHive APIs in consumer code

**Neutral:**
- PulseHive lifecycle tied to consumer application lifecycle
- Resource consumption (threads, memory) happens in consumer process

**Negative:**
- Consumer responsible for their own deployment topology
- PulseHive cannot impose separate runtime processes without consumer awareness

## Implementation

Baseline 60503db ships as in-process library. All primitives (HiveMind, Agent, Tool, Lens, Experience) operate within the consuming application's process space.
