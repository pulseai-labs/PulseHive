# ADR 012: PulseDB 0.7 Migration & Embedding Identity

**Status:** Accepted
**Category:** 11 - Substrate migration & embedding identity
**Touch Surface:** root `Cargo.toml`, `pulsehive-core/src/lib.rs`, `pulsehive-core/src/embedding.rs`, `pulsehive-runtime/src/hivemind.rs`, `docs/09-Deployment.md`, `docs/05-API-Spec.md`
**Revisit Trigger:** When PulseDB ships another on-disk format or schema migration (0.8+), when Release 1 close chooses the published major version, or when sync identity exchange is designed upstream

## Context

PulseHive pins `pulsehive-db = "0.5"` (root `Cargo.toml`, `builtin-embeddings` feature) and
existing collectives were written by PulseDB 0.5.1 against PulseHive 2.0.2. PulseDB 0.6
migrates redb's on-disk format and bincode values to postcard; 0.7 migrates schema v3→v4
and stamps provider identity, giving builtin MiniLM stores the bundled fingerprint migration.
Release 1's exit criterion 5 requires opening a legacy 2.0.2/0.5 collective under PulseDB
0.7.0 without losing prior experiences, and this spine's Round 2 performs the dependency
change. Before that change, the contract for who owns the migration, what happens to
embedding identity, what breaks publicly, and how a consumer rolls back must be recorded.

ADR-004 already delegates schema and file-format migrations to PulseDB and forbids
PulseHive from reimplementing storage logic or metadata. ADR-005 and ADR-008 require a
major release for a changed public contract and a tested rollback path. This ADR applies
those bones to the 0.5.1 → 0.7.0 move.

## Decision

**PulseDB owns the storage migration; PulseHive propagates typed failures.** The redb
format conversion, the postcard value encoding, the schema v3→v4 migration, and their
backup sidecars are PulseDB's. `HiveMindBuilder::build()` keeps calling `PulseDB::open`
on the consumer-owned path and returns typed `PulseHiveError::Substrate` failures
unchanged (`#[from] pulsedb::PulseDBError`). PulseHive adds no migration API, no
migration orchestration, and no storage metadata of its own. A read-only open cannot
perform the migration; the first open after upgrading must be writable.

**Builtin identity is adopted once by PulseDB.** A legacy builtin-MiniLM collective with
no provider stamp is adopted on first 0.7 open and normalized from the unstamped
`builtin-onnx/main_graph` identity to the bundled `builtin-onnx/onnx-<sha256>` identity.
Reopening with the same bundled model succeeds; opening with a different managed identity
fails through PulseDB's typed mismatch error, which reaches the caller as
`PulseHiveError::Substrate`. PulseHive does not intercept or rewrite that outcome.

**Custom embeddings stay caller-controlled in External mode.** The async
`EmbeddingProvider` trait is unchanged. When `HiveMindBuilder::embedding_provider` is
set, PulseHive precomputes vectors through the provider and stores them in PulseDB
External mode; PulseDB cannot verify that caller-provided vectors share a model.
Changing the provider, model, tokenizer, pipeline, or dimensions for an existing path
therefore requires an explicit re-embed into a new substrate path. Silently mixing
vectors from different embedding semantics in one collective is unsupported.

**The re-exported type break is recorded, not shimmed.** `pulsehive-core` re-exports
PulseDB value types (`pub use pulsedb::{…}` in `pulsehive-core/src/lib.rs`). Moving
0.5 → 0.7 changes those public types and their construction shape; this is a breaking
change with no dual-version shim and no PulseHive-owned replacement type layer. The
published major version is a Release 1 close decision and is not chosen here.

**Rollback restores bytes before crates.** To roll a collective fully back to 0.5.1:
stop every writer, restore the pristine `.pre-substrate.bak` produced before the 0.6
storage-format migration, then downgrade the coordinated PulseHive crates together.
`.pre-v4.bak` is the schema-v3 backup made later in the chain and is not the full 0.5
rollback image. Preserve both sidecars until the upgraded collective is validated, and
never attempt a crate-only downgrade against migrated bytes.

**Scope limits.** PulseDB sync peers must be upgraded together. Sync identity exchange,
full embedding-pipeline fingerprinting, and custom External-mode identity enforcement
are upstream or later-scope work and are out of scope for this spine. Nothing here adds
a migration service, a compatibility wrapper, or a new dependency.

**Deploy resolves a task's collective before creating one.** `HiveMind::deploy` and
`redeploy` previously created the `collective-{id}` synthetic namespace unconditionally
and replaced the task's collective ID with the result, so a task naming an existing
collective was redirected into a fresh namespace. They now keep a task's collective ID
when that collective already exists and fall back to the synthetic namespace only for
unknown IDs. This is the behavior the migration proof depends on — deploying onto a
migrated 0.5.1 collective must write into that collective — and it is recorded in the
2.1.0 changelog as the observable deploy behavior change of this upgrade.

**Rationale:** ADR-004's ownership line means the only honest PulseHive posture is to
open the substrate and propagate typed errors; duplicating upstream migration logic
would create a second storage implementation to maintain. Recording the re-export break
without a shim follows ADR-005's major-release rule while leaving the version choice
to release close, and the restore-before-downgrade order follows ADR-008: bytes written
by 0.7 cannot be read by 0.5 crates, so rollback must start from the pristine image.

## Consequences

**Positive:**

- One migration implementation to trust (PulseDB's), with PulseHive failures remaining
  typed, deterministic, and already part of the public error contract.
- Builtin collectives get identity protection without any PulseHive code change, and
  mixed-model corruption in External mode is prevented by contract (new path + re-embed)
  rather than by best-effort detection.
- The public break is stated up front in the ADR series, so the Release 1 major-version
  decision has a recorded basis instead of an implicit one.

**Neutral:**

- Consumers with custom providers must plan a re-embed into a new substrate path when
  embedding semantics change; that has always been the safe behavior, and it is now
  written down.
- The upgrade requires a writable first open and preserved sidecars; deployment docs
  carry the runbook.

**Negative:**

- The re-exported PulseDB value types break source compatibility for consumers that
  construct them; there is no shim, so those consumers must migrate at the next major
  version.
- Rollback fidelity depends on `.pre-substrate.bak` being pristine; a sidecar deleted
  or overwritten before validation forfeits the full 0.5.1 rollback path.
