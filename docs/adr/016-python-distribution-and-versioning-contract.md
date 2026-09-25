# ADR 016: Python Distribution and Versioning Contract

**Status:** Accepted
**Category:** 7 - Rollback & evolution strategy
**Touch Surface:** `pulsehive-py/**,.github/workflows/python-release.yml`
**Revisit Trigger:** When a target is added or dropped, when the interpreter floor moves, when an sdist is reinstated, or when the Python entry decouples from the workspace version line

## Context

`pulsehive` has never been properly published to PyPI: the registry holds
exactly one version, `0.3.0b2`, and nothing else. That artifact is stale —
`pulsehive-py/pyproject.toml` still carries `version = "0.3.0b2"` while the
workspace moved on to `3.0.0` — and `python-release.yml` builds whatever
version pyproject declares, so any `v*` tag would republish `0.3.0b2` (#92).
PyPI had to be declined at the Release 1 close for exactly that reason.

The first real publication is not merely mechanical, though — it fixes a
*consumer distribution contract*: which version line the distribution follows,
how the version reaches the wheel, which interpreters and platforms are
promised, what a consumer on an unadvertised host sees, and how a republish
attempt behaves. Under ADR-005 (public contracts and compatibility policy)
and ADR-008 (rollback and evolution strategy) those are one-way doors: once
consumers pin against them, dropping a platform, raising the interpreter
floor, or reinstating the sdist is a breaking change that has to be batched
into a major release. The version line is the hardest of these to reverse,
which is why this spine is `bone` by the class ladder.

**Repo facts at `304563a`.** `pulsehive-py/pyproject.toml` carries
`version = "0.3.0b2"` while `pulsehive-py/Cargo.toml` carries `3.0.0`;
`python-release.yml` builds from pyproject, so any `v*` tag republishes
`0.3.0b2` (#92), and PyPI holds that one stale version and nothing else. The
`abi3-py39` feature is declared in the workspace root `Cargo.toml` (line 59) —
below the interpreter floor this ADR fixes. ADR-008's touch surface gained
`pulsehive-py/Cargo.toml` at this spine's planning (it already carried
`Cargo.toml`, `CHANGELOG.md`, `pulsehive-js/package.json`,
`pulsehive-py/pyproject.toml` and `.github/workflows/*-release.yml`); the
highest ADR before this one is 015.

The version-and-tag question was decided with the operator at the 2026-09-25
grill, against RELEASE.md's version and tag contract: one orchestration tag
(`v<workspace-version>`) remains the release act, and a tracked version
manifest may map that tag to distinct per-package versions. This ADR records
the Python half of that mapping. The bone-touch registry's blind spot for
`pulsehive-py/` was repaired at planning by re-pointing ADR-008's touch
surface, and this ADR is registered as a bone at spine close — recording the
contract before any packaging path lands is what makes this spine's remaining
work items implement one agreed contract rather than three divergent ones.

## Decision

**The distribution follows the workspace version line (L1).** `pulsehive`
always carries the **workspace version** the `v*` tag names — npm's rule
(ADR-015). The version manifest's Python entry is "the workspace version";
the manifest's home is `r2.s3`'s. Python stays on the one orchestration tag;
no separate Python tag prefix is introduced. `0.3.0b2` is never republished:
the first published version is Release 2's.

**The version is declared once, not carried in two places (A1).**
`pyproject.toml` declares `dynamic = ["version"]` and maturin takes the value
from `pulsehive-py/Cargo.toml`, so a wheel cannot be built at a version the
tag does not name.

**The interpreter range is `>=3.11` on the stable ABI (L2).** `requires-python`
is `>=3.11` and the wheel is `cp311-abi3` — one forward-compatible wheel per
platform, CPython 3.11 and later through the stable ABI. 3.11, 3.12 and 3.13
are the advertised and tested lines. 3.9 (EOL 2025-10-31) and 3.10 (EOL
2026-10-31) are excluded, decided before first publication when it costs
nobody.

**The advertised matrix is exactly three targets (L3).**
`aarch64-apple-darwin` (macOS arm64), `x86_64-unknown-linux-gnu` (Linux
x86_64), and `x86_64-pc-windows-msvc` (Windows x86_64). Those three are the
platform promise this distribution makes; each is installed and imported
before publish, not merely counted (w4), because a successful build cannot
stand for an installable, importable wheel on every advertised host.

**No sdist is published (L3).** PyPI carries wheels only — no sdist. A
consumer on an unadvertised platform or interpreter (Linux aarch64, musl,
3.9/3.10) gets pip's own definite *no matching distribution* error rather
than an attempted Rust source build: there is no install-time source path and
no postinstall download.

**Publication fails closed (A2).** The publish path checks, before
publishing, that the candidate set's version matches the tagged version and
that this version is not already on PyPI, and exits non-zero with a named
error otherwise (ADR-007 failure visibility). Re-publishing an existing
version must **fail closed** (RELEASE.md artifact identity), and a partially
published set is never reported as complete.

**History stays put (A5).** `0.3.0b2` remains on PyPI untouched — published
artifacts are not deleted or moved. The joined version line supersedes it in
resolution: a consumer asking for `pulsehive` gets the workspace version, and
the stale artifact is reached only by an explicit pin.

**Evolution is additive-minor or breaking-major.** Adding a target is
additive (minor). Dropping or renaming a target, raising the interpreter
floor, or reinstating the sdist is breaking (major) under ADR-005 and
ADR-008, batched into a major release with a migration guide.

**Rationale:** Taking the version from `pulsehive-py/Cargo.toml` alone makes a
tag/version mismatch unbuildable rather than merely forbidden, and a single
declared version removes the drift that produced `0.3.0b2`. The stable ABI
gives one forward-compatible wheel per platform, so the matrix stays exactly
as wide as what is actually tested. Publishing no sdist means pip's own
no-matching-distribution error is the contract for unadvertised hosts — a
source build would silently widen the platform promise to wherever a Rust
toolchain exists. Fail-closed publication is what makes RELEASE.md's artifact
identity ("the published artifacts are the ones that passed the gating tests")
checkable instead of aspirational, and leaving `0.3.0b2` in place is ADR-008's
permanent-publication rule applied to PyPI.

## Consequences

**Positive:**

- A Python consumer installs `pulsehive` from PyPI at its released version and
  imports it, receiving that version and never the stale `0.3.0b2`.
- The version line is unambiguous: one `v*` tag names one workspace version,
  and the Python distribution carries it exactly as npm does.
- The supported interpreters, the advertised matrix, and the
  unadvertised-host behaviour are written down, so a consumer can hold the
  project to them.

**Neutral:**

- `pyproject.toml` stops carrying a literal version; the wheel version is
  whatever `pulsehive-py/Cargo.toml` declares at the tagged commit.
- Unadvertised hosts see pip's *no matching distribution* error rather than an
  install; widening the matrix is a deliberate versioned act, not a helpdesk
  fix.

**Negative:**

- The release workflow must publish the version the tag names and carry the
  fail-closed version checks (candidate == tagged; tagged not already on
  PyPI), or a `v*` tag republishes `0.3.0b2` exactly as #92 described. w2 owns
  that packaging path: moving `pyproject.toml` to `dynamic = ["version"]`,
  reading the version the workflow publishes from the manifest, and moving the
  `abi3-py39` feature up to the 3.11 stable ABI the contract promises.
- The contract commits the release path to a hermetic proof that a wheel built
  from this tree installs and imports at the version the workflow would
  publish (w3), and to installing and importing every advertised target under
  3.11, 3.12 and 3.13 before publish (w4).
