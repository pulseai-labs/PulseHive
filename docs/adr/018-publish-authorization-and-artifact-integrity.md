# ADR 018: Publish Authorization and Artifact Integrity

**Status:** Accepted
**Category:** Trust boundaries
**Touch Surface:** `release-manifest.toml`, `scripts/release-check.sh`, `scripts/crates-publish.sh`, `scripts/check-release-workflows.py`, `scripts/release-settings-check.sh`, `.github/workflows/release-gate.yml`, `.github/workflows/crates-release.yml`, `.github/workflows/npm-release.yml`, `.github/workflows/python-release.yml`, `pulsehive-js/scripts/npm-publish.sh`, `pulsehive-py/scripts/py-publish-check.sh`, `docs/RELEASING.md`
**Revisit Trigger:** a second maintainer gets write access (turn `prevent_self_review` on and require a second approver); npm's first publication lands (move npm to trusted publishing); or a registry changes its identity or trusted-publishing model

## Context

Release 2 makes one `v*` tag publish PulseHive to three registries — crates.io,
npm and PyPI — each behind its own GitHub deployment environment. Rounds 1–2 of
spine `r2.s3` landed the mechanism this ADR records:

- `release-manifest.toml` plus `scripts/release-check.sh` and the reusable
  `release-gate.yml` (w1);
- `scripts/crates-publish.sh` and the gated `crates-release.yml` (w2);
- `pulsehive-js/scripts/npm-publish.sh` and the gated `npm-release.yml` (w3);
- the extended `pulsehive-py/scripts/py-publish-check.sh` and the gated
  `python-release.yml` (w4).

RELEASE.md poses five questions for this spine: who approves a publication,
whether self-approval is permitted, which tag sources are allowed, where
credentials are exposed, and how each published artifact is bound to the one
that was tested. It also requires that a partially released state have a design:
a transient retry distinguished from a corrective release, and a per-registry
completion record. This ADR answers those questions against the landed scripts
and workflows, and `docs/RELEASING.md` is the operator's runbook for executing
and recovering them.

**Repo facts read at planning (2026-09-27).** `CARGO_REGISTRY_TOKEN` and
`NPM_TOKEN` were **repository** secrets; the environments `crates-io`, `npm` and
`pypi` each required reviewer `draco28`, with `prevent_self_review: false` and no
deployment policy; ruleset `18326988` "Protect release tags (v*)" blocked only
deletion and non-fast-forward; and the required status checks still included
"Node.js Tests (Node 20)". The operator actions that repair the first four facts
are in `docs/RELEASING.md`'s `## One-time settings`, and
`scripts/release-settings-check.sh` reports whether they have been applied.

**The version and tag contract** (RELEASE.md): one orchestration tag
`v<workspace-version>` is the release act; `release-manifest.toml` is the
tracked mapping the release engineer edits before tagging, and every registry
line joins the workspace line while ADR-015 L1 and ADR-016 L1 hold.
`scripts/release-check.sh` fails on any disagreement between the tag, the
manifest and a package file (A1).

This ADR rides ADR-006 (trust boundaries — the credential surface is the one
job that publishes), ADR-007 (failure visibility — every refusal is a named
line), ADR-008 (rollback — published artifacts are never deleted or moved;
binding rollback stays with #59), ADR-015 (the npm installation contract) and
ADR-016 (the Python distribution and versioning contract).

## Threat model

**The gate runs from the tagged commit, so it checks itself: it catches
mistakes, not attacks.** `release-gate.yml` is invoked with `uses:` from the
tagged tree, and a `v*` tag on an older `main` commit runs that commit's
workflows — including the old, ungated ones. Nothing in this design makes a
stale tag safe to approve.

**The trust boundary is the admin role (tag creation) plus the operator's
environment approval.** Tag creation is restricted to the admin role by the
`v*` ruleset; the environment approval is the second, independent human gate.
Everything else in the pipeline is defence in depth.

Before approving, the approver independently confirms two things, and neither
relies on workflow output:

- `gh api repos/pulseai-labs/PulseHive/compare/main...<sha>` reports `behind` or
  `identical` — the tagged commit is on `main`;
- `git show <sha>:.github/workflows/<file>` shows `needs:` on `gate` — the
  workflow at the tagged commit is the gated one.

Both are written into `docs/RELEASING.md`'s `## Approving` checklist. This
threat model is SPINE.md's plan-audit fold-in (2026-10-02, host + codex),
which withdrew the earlier framing of machine checks as something an approver
could not skip.

## Decision

### Who approves

The operator (`draco28`, the repository's only collaborator) is the required
reviewer on all three environments — `crates-io`, `npm` and `pypi`. A publish
job sits behind its environment, so it starts only after that approval, and only
for a `v*` tag (`if: startsWith(github.ref, 'refs/tags/v')` on every publish
job). `release-settings-check.sh` asserts the reviewer and that each
environment's deployment policy admits only `v*` tags.

### Whether self-approval is permitted

**Self-approval is permitted explicitly.** `prevent_self_review` is `false`, and
that is deliberate: whoever pushes the tag starts the run, so with a single
maintainer requiring a second reviewer would make publishing impossible. The
compensating controls are the gate's manifest/tag check and CI, the on-`main`
ancestry check, the identity checks in each publish script, the `v*`-only
deployment policies, and the tag-creation restriction. Revisit trigger: a second
maintainer with write access — then turn `prevent_self_review` on and require a
second approver.

### Which tag sources are allowed

The gate's `manifest-check` job requires the ref to be a tag named
`v<manifest workspace.version>` whose commit is an ancestor of `origin/main`
(`bash scripts/release-check.sh --ref "$GITHUB_REF" --on-main "$GITHUB_SHA"`).
A tag cut from a branch or an unmerged commit fails before tests run. The
`v*`-only environment deployment policies and the tag-creation restriction are
defence in depth, applied by the operator.

### Where credentials are exposed

| Registry | Mechanism | The one job and step that sees it |
|---|---|---|
| crates.io | OIDC trusted publishing; `rust-lang/crates-io-auth-action` (pinned to a commit SHA) exchanges the job's OIDC token for a short-lived token | job `publish` in `crates-release.yml`, step **Publish the tested crates** (`CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}`); `id-token: write` is granted on that job alone |
| npm | granular `NPM_TOKEN`, stored in the `npm` environment for the first publication | job `publish` in `npm-release.yml`, step **Publish to npm** (`NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}`) |
| PyPI | OIDC trusted publishing through `pypa/gh-action-pypi-publish` (pinned) | job `publish` in `python-release.yml`, step **Publish to PyPI** |

**The whole publish job is the privileged unit, not the step that names the
credential.** OIDC `id-token: write` and a step-level environment variable are
both visible to every step of that job, so the boundary drawn here is the job:
no other job in any release workflow may read a registry credential, and
`check-release-workflows.py` enforces that — it rejects a secret referenced
outside the publish job, `secrets: inherit`, bracket secret access, and
`id-token: write` on any job other than the publish job. Every action in a
publish job is pinned to a 40-hex commit SHA, which the same checker enforces.

### How the published artifact is bound to the tested one

**crates.io — `SHA256SUMS` under the frozen `Cargo.lock`, with a joint pre-upload
repackage and a per-crate index `cksum` backstop.** The gate's `package` job runs
verified `cargo package` for the five manifest crates and uploads the `.crate`
files, the workspace `Cargo.lock` the packaging resolved, and a `SHA256SUMS`
(artifact `tested-crates`, `retention-days: 90`). The publish job restores that
lock to the workspace root and every cargo call runs `--locked` against it. The
crates the decision table marks `publish` are uploaded **jointly** — one
`cargo publish --no-verify --locked` with one `-p` per crate, in manifest order
— preceded by a **joint** repackage of exactly that set, byte-compared to
`SHA256SUMS` before a single byte leaves the machine, and followed by a
per-crate index `cksum` poll that must equal the tested hash.

*Why jointly, measured.* Packaging a crate alone embeds the registry's cksum of
an intra-workspace dependency in the `.crate`'s inner `Cargo.lock` instead of
the tested overlay checksum: `cargo publish -p pulsehive-openai --locked
--no-verify --dry-run` packages `adf8dec5…`, while the tested joint artifact for
the same crate is `7ae33a15…`. The joint form packages exactly the five tested
hashes (`9367f29b…`, `7ae33a15…`, `1dc5f9e7…`, `59d1f352…`, `bfbca49e…`), and
the crate-alone form cannot even run while a dependency is unpublished
(`no matching package named … found`, exit 101). This supersedes the per-crate
`cargo publish -p <crate>` and single-crate pre-upload check of the plan
(audit 2's L5 amendment), on the orchestrator's ruling of 2026-10-02.

*Reproducibility and its cross-toolchain limit.* `cargo package` is
byte-reproducible on one toolchain: touching every source mtime left the
sha256 `9bdf7363…` unchanged. It is **not** reproducible across toolchains —
the published 3.0.0 `pulsehive-core` carries index cksum `7b102944…` from
another toolchain. That pair is the negative-control fixture the scripts'
self-tests use, and it is why both crates.io jobs install one pinned toolchain
(`RELEASE_RUST_TOOLCHAIN: "1.98.1"`, declared once in `crates-release.yml`).
The pin is bumped deliberately, never incidentally.

**"Tested" means the same commit's workspace passed the gate's CI and each
packaged crate compiled under verified `cargo package`.** The archive's own
tests are not run — a `.crate` carries no test invocation, and claiming
otherwise would overstate the evidence.

**npm — `dist.integrity`.** Nothing is re-packed in the publish job: the
tarballs are the bytes `verify-install` installed and executed. For each tarball
(platform packages first, the main package last) the local
`sha512-<base64>` integrity of the file is compared against the registry's
`dist.integrity` for `<name>@<version>`: equal → skipped as already published
with the tested bytes; different → refused, and nothing further is published;
E404 → published. After each upload the published integrity is re-read with
bounded retries and must equal the tested one. The main package is never
published unless every platform package is published or skipped.

**PyPI — `digests.sha256`.** The tested wheels' sha256 inventory is compared
against PyPI's per-version JSON: a published file that is foreign to the tested
set, yanked, or byte-different refuses the run; `complete` (every published file
is a tested wheel byte for byte, and every tested wheel is published) publishes
nothing; `resume` uploads only the missing wheels. The publish job re-checks
the state after approval (A15) and uploads with that fresh mode, and
`--verify-published` proves after the upload that every tested wheel is on PyPI
with the same sha256.

## Recovery

L4, as narrowed by the plan audit (audit 3):

- **Transient retry** — network, registry outage, a partial publish loop: re-run
  the failed jobs of the **original** run. Only that path reuses the tested
  artifacts; they are retained for 90 days (`retention-days: 90` on all three
  artifact uploads).
- **`workflow_dispatch` on the tag is a fresh build.** It is legitimate only for
  a registry with **nothing** published for that version; otherwise the identity
  check refuses it by design, and the refusal names the remedy — `this is a
  fresh build; re-run the failed jobs of the original tag run instead`.
- **Past GitHub's 30-day re-run window**, a partially published registry takes
  the corrective path.
- **Corrective release** — a defect in the tagged commit's source or workflow:
  fix on `main`, bump the patch version in the manifest, cut a new tag. All
  three registries move to the new version together, keeping ADR-015 L1 and
  ADR-016 L1's joined line; versions already published stay published.
- **Never overwrite.** A version already published with different bytes is
  refused; a version or file that is yanked counts as not complete, whatever its
  bytes say, and unyanking is the operator's decision.
- Every refusal is a named `ERROR:` line on stderr, and
  `docs/RELEASING.md`'s `## Recovery` table maps every line the four scripts and
  the gate can print to its classification (transient / corrective / settings)
  and its exact action.

## Completion

A release is complete only when every registry the tag owns has published, or
has a **recorded disposition** — resume, abandon or corrective — in the
per-registry table of its GitHub Release notes. The operator decides the
disposition; `docs/RELEASING.md`'s `## Partial releases` carries the template and
says what consumers are told meanwhile. Each publish job writes its outcome
lines to the job summary (A9/A16), so the run that produced the state is
readable next to the state itself.

## Alternatives

- **One combined release workflow** — rejected. A single job would hold all
  three registries' credentials, and one approval would admit all three
  publishes; the per-registry environment approvals and the per-registry
  identity checks would collapse into one, and a failure in any registry would
  take the others' approvals with it.
- **Repository-wide tokens** — rejected. A repository secret is readable by
  every workflow and every step; the environment-scoped `NPM_TOKEN` and the two
  OIDC exchanges exist to keep each credential to the one job that publishes.
- **`prevent_self_review` with a single maintainer** — rejected. Whoever pushes
  the tag starts the run, so it would make publishing impossible; the
  compensating controls above are what hold the boundary instead.
- **Treating "already exists" as success** — rejected. The published bytes may
  differ from the tested ones; a version whose bytes differ is not this tag's
  artifact, and a re-run must not bless it. The identity check refuses it and
  names the remedy.
- **Uploading a prebuilt `.crate` through the raw API** — rejected. It would
  bypass verified `cargo package` and the registry's own checksum and index
  bookkeeping; the tested set is repackaged jointly and published by cargo
  itself.

## Consequences

**Positive:**

- Every registry's publish is behind its own approval and its own identity
  check, so a `v*` tag cannot ship bytes the gate did not test.
- Each credential is exposed to exactly one publish job, and each such job is
  auditable: pinned actions, `id-token` scoping, and a checker that rejects the
  alternatives.
- A partially released state has a written response and a per-registry record,
  so "complete" is a decision with an owner rather than an assumption.

**Negative:**

- Three workflows each run the full CI on a tag — the gate is invoked three
  times for one release, which is deliberate redundancy and costs runner time.
- The toolchain pin needs a deliberate bump; a `.crate`'s bytes are
  toolchain-bound, so drifting the pin silently would break the tested-hash
  chain.
- npm keeps a stored token until its trusted publishing lands; the token is
  environment-scoped and time-limited, but it is a stored credential where the
  other two registries have none.
- The whole publish job is privileged — a compromised step in it can read the
  credential — which is the price of a publish job that must run a script.
- A release can be complete only through an operator's recorded disposition;
  the process does not complete itself.
