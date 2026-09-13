# ADR 013: Transport & Substrate Boundary Contract

**Status:** Accepted
**Category:** 2 - Module boundaries & dependency direction
**Touch Surface:** `pulsehive/Cargo.toml,pulsehive-core/Cargo.toml,pulsehive-core/src/,pulsehive-runtime/src/,pulsehive-openai/src/,pulsehive-anthropic/src/,pulsehive-py/src/,pulsehive-js/src/,docs/adr/003-module-boundaries.md,docs/adr/004-data-ownership.md`
**Revisit Trigger:** When a consumer profile beyond transport-only and full-runtime appears, when the Release 1 version is chosen at close, or when a storage-coupled API is proposed for the always-on core surface

## Context

`pulsehive` depends on `pulsehive-runtime` unconditionally, and `pulsehive-core`
depends on PulseDB through its public identifiers and storage-coupled surfaces.
Cargo features are additive: a consumer asking for `features = ["openai"]` cannot
subtract the runtime, so no transport-only dependency footprint exists while
those defaults hold. Release 1 exit criterion 4 requires a transport-only
consumer that builds with `features = ["openai"]` and resolves neither
`pulsehive-runtime` nor `pulsehive-db`, measured against the 132-package /
54-second PulseTrader baseline.

ADR-003 records `Experience` as PulseDB-defined and re-exported from core;
ADR-004 gives PulseDB all storage ownership. Both remain true for persisted
values, but neither distinguished public identifier ownership from persistence
ownership. This ADR locks that ownership and Cargo-feature contract before any
code moves; later work items implement and prove it.

## Decision

**Core owns the public identifiers.** `pulsehive-core` owns the public typed
UUID identifiers `CollectiveId`, `ExperienceId`, `InsightId`, and `RelationId`.
They preserve the established UUID serde/text representation and the
`new`/`nil`/parse/display behavior consumers already depend on.

**Conversion lives at the substrate adapter boundary.** Conversion to and from
PulseDB identifier types occurs only in `pulsehive-runtime`, at the substrate
adapter boundary. Core never imports PulseDB to implement conversion.

**The always-on core is substrate-free.** Core identifiers and `HiveEvent` stay
available and shape-stable without PulseDB. PulseDB re-exports and the
storage-coupled portions of tool context, substrate errors, lens conversion,
and experience extraction require the default-off core `substrate` feature.

**Runtime is an explicit opt-in feature.** The `pulsehive` meta-crate has no
default runtime. `runtime` enables `pulsehive-runtime` plus core substrate
support; `openai` and `anthropic` remain transport-only provider features;
`testing` implies `runtime`; the Python and JavaScript bindings enable runtime
explicitly.

**The break is recorded, not shimmed.** Existing runtime consumers add
`runtime`. This and the ID type-identity change are breaking contract changes
owned by Release 1 close. No new facade crate, dual ID compatibility layer, or
shim is introduced.

**PulseDB keeps storage ownership.** PulseDB continues to own `Experience`,
`NewExperience`, `SubstrateProvider`, storage errors, schema, migration,
vectors, graph, and watch semantics. Core identifier ownership is not storage
ownership.

**Verification is a standalone consumer.** The transport-only proof uses a
standalone Cargo consumer outside workspace feature unification, so workspace
members cannot silently re-enable runtime or substrate. The recurring gate
requires that the resolved graph contain neither `pulsehive-runtime` nor
`pulsehive-db` and a package count below the 132-package baseline; cold-build
time is measured once against 54 seconds and recorded rather than gated.

**Rejected alternatives.** Gating the PulseDB-owned identifier types behind
`substrate` would make `HiveEvent`'s shape vary by feature, breaking
shape-stability for transport-only consumers. Moving the entire substrate
value/port model into core would duplicate PulseDB ownership and create a
second storage implementation to maintain.

**Rationale:** Identifier ownership is what forces `pulsehive-core` to depend
on PulseDB today, so moving the public typed handles into core is the smallest
change that lets the always-on surface compile substrate-free. Keeping
conversion in `pulsehive-runtime` preserves ADR-004's ownership line — PulseDB
still owns every persisted value — while making the adapter boundary the single
place the two type domains meet. An explicit `runtime` feature turns the
transport-only footprint into a consumer-visible contract rather than a hidden
default, and recording the break now gives the Release 1 version decision a
documented basis per ADR-005.

## Consequences

**Positive:**

- A transport-only consumer can depend on `pulsehive` with `features =
  ["openai"]` and resolve a graph with neither `pulsehive-runtime` nor
  PulseDB — Release 1 exit criterion 4 becomes implementable.
- `HiveEvent` and the four public identifiers are shape-stable for every
  consumer profile; no feature flag can change their definition.
- One conversion site (`pulsehive-runtime`'s substrate adapter boundary) owns
  the ID type-domain crossing, so the PulseDB boundary stays auditable.
- The breaking consequences are recorded up front, giving the Release 1 close
  version decision a recorded basis instead of an implicit one.

**Neutral:**

- Existing runtime consumers must add `runtime` to their feature set; examples,
  tests, docs, and bindings are updated to name it explicitly.
- The package-count gate runs against a standalone consumer outside workspace
  feature unification, so it is an honest oracle but one more fixture to
  maintain.
- Cold-build time is recorded once against the 54-second baseline rather than
  gated, so timing regressions surface in the ledger, not as build failures.

**Negative:**

- Consumers constructing PulseDB identifier types through core re-exports lose
  that path; the ID types change identity and there is no dual-type
  compatibility layer.
- Storage-coupled APIs behind `substrate` are unavailable to direct core
  consumers unless they opt in, adding a feature decision where none existed.
