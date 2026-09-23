# ADR 015: npm Installation Contract

**Status:** Accepted
**Category:** 7 - Rollback & evolution strategy
**Touch Surface:** `pulsehive-js/**,.github/workflows/npm-release.yml`
**Revisit Trigger:** When a target is added or dropped, when the Node floor moves, or when the version manifest's npm entry decouples

## Context

`@pulsehive/sdk` has never been published to npm (#97): the v3.0.0 deployment
Release 1 shipped to crates.io only, because `napi prepublish` asserts
per-target `pulsehive-js/npm/<platform>/` trees that no workflow step creates.
The first publication is not merely mechanical, though — it fixes a *consumer
installation contract*: what a Node consumer installs, how the native binary
reaches them, which hosts and Node versions the promise covers, what an
unadvertised host sees, and which version line the package follows. Under
ADR-005 (public contracts and compatibility policy) and ADR-008 (rollback and
evolution strategy) those are one-way doors: once consumers pin against them,
dropping a target, raising the Node floor, or renaming a package is a breaking
change that has to be batched into a major release.

The bone-touch registry did not see `pulsehive-js/` at release planning, which
is why `r2.s1` is `bone` by critic veto rather than by the class ladder; the
blind spot was repaired at planning (L5) by re-pointing ADR-008's touch surface
at `pulsehive-js/package.json` and `.github/workflows/*-release.yml`, and this
ADR is registered as a bone at spine close. Recording the contract before any
packaging path lands is what makes this spine's remaining work items implement
one agreed contract rather than three divergent ones.

**Repo facts at `4970e66`.** `pulsehive-js/package.json` names the package
`@pulsehive/sdk` at version `3.0.0`, declares `napi.binaryName` `pulsehive-js`,
lists exactly the three triples below as `napi.targets`, declares
`engines.node` `>= 20`, and includes `*.node` in `files`. The napi-generated
loader `pulsehive-js/index.js` already requires the platform packages by the
names below. Nothing has been published.

## Decision

**One main package and one package per advertised target (L3).** The main
package is `@pulsehive/sdk`; the platform packages use napi's default names —
`@pulsehive/sdk-darwin-arm64`, `@pulsehive/sdk-linux-x64-gnu`, and
`@pulsehive/sdk-win32-x64-msvc` — under the same scope and the same owner.
There is no second naming scheme and no unscoped fallback.

**Binaries ship only in the platform packages (A2).** The main package carries
**no** `.node` file. It lists the three platform packages as
`optionalDependencies` pinned to its own exact version; npm installs only the
one matching the host's `os`/`cpu`/`libc`, and `pulsehive-js/index.js` requires
it. A local `pulsehive-js.<platform>.node` beside the loader is a
development-build convenience, never a published path. There is no install-time
source build and no postinstall download.

**The advertised matrix is exactly the `napi.targets` list (A1).**
`aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, and
`x86_64-pc-windows-msvc`. Those three are the platform promise this package
makes; each is installed and executed before publish, not merely counted (w4),
because matching target names against artifact names can bless a mislabelled or
unloadable binary.

**The Node range is `>=22`, tested on Node 22 and Node 24 (L2).**
`engines.node` is `>=22`. Node 22 and Node 24 are the advertised and tested
lines; Node 20 is excluded (end of life 2026-04-30) and is dropped before the
first publication, while it costs nobody.

**An unadvertised host fails loudly, naming the host and the matrix (A3).**
`npm install` succeeds — the optional platform packages that do not match the
host are skipped — and the first `require('@pulsehive/sdk')` throws an error
that names the host's platform/arch and lists the three advertised targets.
Never a silent fallback, and never a raw module-not-found trace alone (ADR-007
failure visibility).

**The main package and its platform packages follow the workspace version
(L1).** `@pulsehive/sdk` and its three platform packages always carry the
**workspace version** the `v*` tag names; the version manifest's npm entry is
"the workspace version" (the manifest's home is `r2.s3`'s). `3.0.0` is never
published to npm; the first npm version is Release 2's.

**Evolution is additive-minor or breaking-major.** Adding a target is additive
(minor). Dropping a target, raising the Node floor, or renaming a package is
breaking (major) under ADR-005 and ADR-008, and is batched into a major release
with a migration guide.

**Rationale:** napi's default platform package names and the generated loader
already agree, so adopting them keeps the published shape identical to what the
code requires today. Resolving the binary through exact-version
`optionalDependencies` is the only mechanism that gives npm the host's
`os`/`cpu`/`libc` facts without an install-time build or a download script, and
it keeps the main package free of a binary that could not run on most hosts.
Pinning the platform packages to the main package's exact version keeps a
half-upgraded tree from mixing a loader with an incompatible binary. Failing
loudly on an unadvertised host is ADR-007's contract applied to the install
surface: the consumer learns the real cause and the real matrix instead of a
module-not-found trace.

## Consequences

**Positive:**

- A Node consumer installs `@pulsehive/sdk` from the registry and requires it
  with no Rust toolchain and no source build; npm selects the host's platform
  package automatically.
- The supported matrix, the Node range, and the unadvertised-host behaviour are
  written down, so a consumer can hold the project to them.
- The version line is unambiguous: one `v*` tag names one workspace version,
  and both the main and platform packages carry it.

**Neutral:**

- The main package's tarball stays small and host-independent, but the install
  depends on the platform packages being published alongside it; a missing
  platform package surfaces as a loader error on that host.
- `engines.node` moves from `>= 20` to `>=22`, excluding Node 20 before the
  first publication.

**Negative:**

- The release workflow must materialize the per-target
  `pulsehive-js/npm/<platform>/` trees and check `napi.targets` against the
  artifact naming before `napi prepublish`, or the publication fails exactly as
  #97 did (w2 owns that packaging path).
- The contract commits the release workflow to install and execute every
  advertised target before publish (w4), and to a hermetic local-registry proof
  that the host platform package resolves and the main package carries no
  binary (w3).
