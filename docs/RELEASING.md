# Releasing PulseHive

The canonical runbook for cutting, approving and recovering a PulseHive release.
It is the operator's half of ADR-018 (publish authorization and artifact
integrity); the mechanism it drives lives in `release-manifest.toml`,
`scripts/release-check.sh`, `scripts/crates-publish.sh`,
`pulsehive-js/scripts/npm-publish.sh`, `pulsehive-py/scripts/py-publish-check.sh`
and the four workflows under `.github/workflows/`.

One `v*` tag publishes three registries, each behind its own environment
approval: the five crates to crates.io (`crates-io`), `@pulsehive/sdk` and its
three platform packages to npm (`npm`), and the `pulsehive` wheel set to PyPI
(`pypi`). Nothing here publishes by hand: every upload runs from the tag's
workflow, and the tag is never moved.

## One-time settings

These are operator actions — no seat and no workflow changes them. Run
`bash scripts/release-settings-check.sh` after each to see the item turn from
`MISSING:` to `ok:`; `settings: ok` means every machine-readable item is in
place. Its `MISSING:` lines map here: the repository secrets and the
environment's `NPM_TOKEN` to step 2, the environment's required reviewer
(`draco28`) and `prevent_self_review: false` to the same **Settings →
Environments → `<env>`** page as step 3, the deployment policy to step 3, the
ruleset to step 4, and the required checks to step 5; a `cannot read …` or
`cannot parse …` line means the read failed — retry it.

**1. crates.io and PyPI trusted publishing.** Register the trusted publishers;
`gh` cannot read them, so they are **manual evidence rows** — record the
crates.io publisher IDs (one per crate) and the PyPI publisher in the release
notes.

- crates.io, per crate: `https://crates.io/crates/<crate>/settings` →
  **Trusted Publishing** → Add. Repository owner `pulseai-labs`, repository
  `PulseHive`, workflow filename `crates-release.yml`, environment `crates-io`.
  Do this for `pulsehive`, `pulsehive-core`, `pulsehive-runtime`,
  `pulsehive-openai` and `pulsehive-anthropic`.
- PyPI: `https://pypi.org/manage/project/pulsehive/settings/publishing/` → add a
  trusted publisher. Owner `pulseai-labs`, repository `PulseHive`, workflow
  `python-release.yml`, environment `pypi`.

**2. Move `NPM_TOKEN` into the `npm` environment and delete the repository
secrets.**

- UI: repository → **Settings → Environments → npm → Environment secrets → Add
  environment secret**, name `NPM_TOKEN`.
- `gh`: `gh secret set NPM_TOKEN --env npm --repo pulseai-labs/PulseHive`
  (prompts for the value; the `npm` environment must exist first), then
  `gh secret delete NPM_TOKEN --repo pulseai-labs/PulseHive` and
  `gh secret delete CARGO_REGISTRY_TOKEN --repo pulseai-labs/PulseHive`.
- The token's **registry-side contract** (it is a bootstrap credential; the
  settings check sees only its presence, never its scope):
  - a **granular** npm token with read-and-write on `@pulsehive/sdk`,
    `@pulsehive/sdk-darwin-arm64`, `@pulsehive/sdk-linux-x64-gnu` and
    `@pulsehive/sdk-win32-x64-msvc`;
  - able to publish without an interactive 2FA prompt;
  - expiring in **at most 30 days**;
  - stored **only** in the `npm` environment (never a repository secret);
  - revoked once npm trusted publishing lands — npm trusted publishing can only
    be configured on a package that already exists, so it follows the first
    publication (feature map).

**3. A deployment policy that admits only `v*` tags, on each environment.**

- UI: **Settings → Environments → `crates-io` / `npm` / `pypi` → Deployment
  branches and tags → Selected branches and tags** → add a **tag** rule `v*`.
- `gh`, per environment:
  `gh api --method POST repos/pulseai-labs/PulseHive/environments/<env>/deployment-branch-policies -f name='v*' -f type=tag`.

**4. Restrict tag creation on the "Protect release tags (v*)" ruleset.**

- UI: **Settings → Rules → Rulesets → Protect release tags (v\*)** → add the
  **Restrict creations** rule (keep **Restrict deletions** and **Block force
  pushes**), target **Tags**, enforcement **Active**.
- `gh`:
  `gh api --method PUT repos/pulseai-labs/PulseHive/rulesets/18326988 --input ruleset.json`,
  where `ruleset.json` carries `"target": "tag"`, `"enforcement": "active"`,
  `"conditions": {"ref_name": {"include": ["refs/tags/v*"]}}` and
  `"rules": [{"type": "creation"}, {"type": "deletion"}, {"type": "non_fast_forward"}]`.

**5. Replace the required status check "Node.js Tests (Node 20)".**

- UI: **Settings → Branches → `main` → Edit → Require status checks to pass** →
  remove "Node.js Tests (Node 20)", add "Node.js Tests (Node 22)" and
  "Node.js Tests (Node 24)".
- `gh`:
  `gh api --method PATCH repos/pulseai-labs/PulseHive/branches/main/protection/required_status_checks -F strict=true -f 'contexts[]=Node.js Tests (Node 22)' -f 'contexts[]=Node.js Tests (Node 24)'` —
  this **replaces** the whole context list, so include every check that must
  stay required.

Until step 5 is applied the spine's PR waits on a check that never reports.

## Cutting a release

1. **Bump every version file and the manifest together.** `release-manifest.toml`
   is the release engineer's input; `scripts/release-check.sh` fails on any
   disagreement between it, the workspace, the crates, `pulsehive-js` and
   `pulsehive-py`. Edit, then:
   `bash scripts/release-check.sh` until it prints `release-check: ok <v>`.
2. **Update `CHANGELOG.md`** with the release's entries.
3. **Merge to `main`** (PR, required checks green).
4. **Tag once, from `main`:** `git tag v<v> <main-commit>`, then
   `git push origin v<v>`. The tag is `v<manifest workspace.version>`; pushing it
   is the release act. Never move, delete or re-cut it.
5. **Watch the three workflows** (`Crates.io Release`, `npm Release`,
   `Python Release`). Each runs the release gate first (`Manifest and tag check`,
   then all of `ci.yml`), then builds and packages the tested artifacts, then
   waits at its environment for approval.

## Approving

Each registry's publish job waits behind its own environment. Before approving
any of them, check the gate is green and then confirm **independently** — from
`git` and the API, not from the workflow's own output — that the run is the one
you think it is:

- `gh api repos/pulseai-labs/PulseHive/compare/main...<sha>` reports `behind` or
  `identical` (the tagged commit is on `main`; anything else is `ahead` or
  `diverged` and must not be approved);
- `git show <sha>:.github/workflows/<file>` shows `needs:` on `gate` — the
  workflow at the tagged commit is the gated one;
- the **pre-approval inventory** in the last pre-environment job's summary names
  the manifest version and the expected files: `package` for crates.io (the
  `SHA256SUMS` inventory and `crates-publish.sh plan`'s decisions), `pack` for
  npm (the tarball inventory), `verify-artifacts` for PyPI (the wheel names,
  their sha256 and the `publish-mode`).

Then approve the environment when:

- the gate's **`Manifest and tag check`** and **`Tests`** are green — a run whose
  gate did not pass is never approved;
- the job summary names the version you expect (`release-check.sh --get <key>` is
  the source);
- the pre-approval inventory's file set and decisions match what you are about to
  publish.

## Recovery

**The three classifications.** *Transient* — re-run the failed jobs of the
**original** run (Actions → the run → **Re-run failed jobs**): only that path
reuses the tested artifacts, which are retained for 90 days. A
`workflow_dispatch` on the same `v*` tag is a **fresh build** — it is legitimate
only for a registry with **nothing** published for that version; every publish
step is identity-checked, so a dispatch that would republish is refused by
design, and the refusal names the remedy (`this is a fresh build; re-run the
failed jobs of the original tag run instead`). Past GitHub's 30-day re-run
window, a partially published registry takes the *corrective* path. *Corrective*
— fix on `main`, bump the patch version in the manifest (and the version files),
merge, cut a **new** tag; all three registries move to the new version together,
and versions already published stay published. *Settings* — fix the named
repository or registry setting (see `## One-time settings`), then re-run.

Every line below is quoted from the landed script that prints it. The
classification is the first response; where a line can have two causes, the
action says so. `scripts/release-settings-check.sh`'s `MISSING:` lines are not
release-run failures: each names the setting to fix in `## One-time settings`
(a `cannot read …` or `cannot parse …` line there means the read failed —
retry it).

### The gate (`release-gate.yml` → `scripts/release-check.sh`)

Every line is printed on stderr as `release-check: ERROR: <message>`.

| Line (as printed) | Class | Action |
|---|---|---|
| `ref <ref> is not a v* tag` | transient | The run was dispatched on a non-tag ref. Re-run the workflow on the `v*` tag itself; the gate refuses any other ref by design. |
| `tag <tag> does not name manifest version <v>` | corrective | The tag and the manifest disagree. Fix the manifest/version files on `main`, cut a new tag; never move this one. |
| `commit <sha> is not on origin/main` | corrective | The tag points at an unmerged commit. Merge the commit to `main` and cut a new tag from it. |
| `manifest <path> not found` / `manifest <path> does not parse: <detail>` / `manifest schema is <n>, expected 1` / `manifest <path> is missing <key>` / `unknown manifest key '<key>'` | corrective | The tagged tree's manifest is unreadable or malformed. Repair it on `main`, cut a new tag. |
| `<crate>/Cargo.toml: no readable [package] version` / `Cargo.toml: no readable [workspace.dependencies].<crate>.version` / `pulsehive-js/Cargo.toml: no readable [package] version` / `pulsehive-js/package.json: no readable version` / `pulsehive-js/package.json: no readable name` / `pulsehive-py/Cargo.toml: no readable [package] version` / `pulsehive-py/pyproject.toml: no readable [project] name` | corrective | A version or name the check reads is missing from the tagged tree. Repair on `main`, cut a new tag. |
| `<file> declares <a>, manifest <key> is <b>` / `<file>.version <a> differs from workspace.version <b> (joined line, ADR-015 L1 / ADR-016 L1)` — the shapes are `<crate>/Cargo.toml declares …, manifest rust.version is …`, `Cargo.toml declares …, manifest rust.version is …` (the workspace dependency), `pulsehive-js/Cargo.toml declares …, manifest npm.version is …`, `pulsehive-js/package.json declares …, manifest npm.version is …`, `pulsehive-js/package.json declares …, manifest npm.package is …`, `pulsehive-py/Cargo.toml declares …, manifest python.version is …`, `pulsehive-py/pyproject.toml declares …, manifest python.distribution is …` | corrective | Version drift between the manifest and a package file. Bump them together on `main`, cut a new tag. |
| The `Tests` job failed (any `ci.yml` job red) | corrective | A test or check failed on the tagged commit. Re-run once to rule out a flake; if it reproduces, fix on `main`, patch-bump, new tag. |
| `check-release-workflows: ERROR: <path>: <message>` (the `release-workflows` CI job) | corrective | A release workflow violates the gate contract (publish gating, credential scoping, pinned actions, concurrency). Fix the workflow on `main`, cut a new tag. |

### crates.io (`scripts/crates-publish.sh`)

Every line is printed on stderr as `crates-publish: ERROR: <message>`.

| Line (as printed) | Class | Action |
|---|---|---|
| `no crates.io token (did the trusted-publishing exchange fail?) — refusing` | settings | The OIDC exchange produced no token: check the crate's trusted-publisher registration (repository, workflow filename `crates-release.yml`, environment `crates-io`) in `## One-time settings`. Re-run first if the exchange failed transiently. |
| `is already on crates.io with different bytes (index <a>, tested <b>) — refusing` | corrective | A published version differs from the tested bytes. It is never overwritten: fix on `main`, patch-bump, new tag, and record the disposition in `## Partial releases`. |
| `… — refusing — this is a fresh build; re-run the failed jobs of the original tag run instead` | transient | A `workflow_dispatch` run built fresh bytes over a published version. Re-run the **original** tag run's failed jobs; the dispatch cannot publish here. |
| `is yanked on crates.io — not complete; unyanking is the operator's decision (docs/RELEASING.md)` | settings | A yanked version is not complete whatever its bytes say. The operator decides: unyank the crate (crates.io → the crate's settings) and re-run, or take the corrective path. |
| `cannot read the crates.io index for <crate> (<detail>) — refusing` | transient | The index read failed (network, registry outage, or an unparseable body). Re-run the failed jobs. |
| `<crate> <v> published, but the index never showed the version — refusing` | transient | The upload succeeded but the index did not catch up within the bounded poll. Re-run; the decision table skips a version whose cksum matches. |
| `<crate> <v> published, but the index cksum <a> is not the tested <b>` | corrective | The registry holds different bytes than the tested set. Never overwrite: patch-bump and cut a new tag; record the disposition. |
| `<crate>-<v>.crate differs from the tested package (tested <a>, rebuilt <b>)` | corrective | The pre-upload repackage did not reproduce the tested bytes. Confirm the pinned toolchain (`RELEASE_RUST_TOOLCHAIN`) and the frozen lock; if the tree is at fault, fix on `main`, new tag. |
| `no Cargo.lock in <dir> — the tested resolution is unknown, refusing` | transient | The tested artifact is incomplete (the `package` job's upload/download lost the lock). Re-run the original run; past the 30-day window, corrective. |
| `no sha256 for <crate>-<v>.crate in <dir>/SHA256SUMS` / `the repackaged <crate>-<v>.crate is missing from <file>` | transient | The tested set is incomplete. Re-run the original run; if the set is wrong on the tagged commit, corrective. |
| `cannot read rust.crates from <check>` / `cannot read rust.version from <check>` / `the manifest names no rust crates` | corrective | The manifest the script reads is wrong in the tagged tree. Repair on `main`, cut a new tag. |
| `cargo package failed for <set>` / `cargo package --no-verify failed for <set>` / `cargo publish failed for <set>` | transient | Cargo or the registry failed (network, outage, a flaky download). Re-run the failed jobs; if it reproduces, corrective. |
| `cannot copy <dir>/Cargo.lock to <ROOT>/Cargo.lock` / `cannot copy the resolved Cargo.lock into <out>` / `cannot write <out>/SHA256SUMS` / `cannot create <out>` / `cannot create a scratch dir for the repackage` / `cannot create a scratch dir for the pre-upload repackage` / `no workspace Cargo.lock after packaging — the tested resolution is unknown, refusing` | transient | A runner-local failure (disk, permissions, a scratch directory). Re-run the failed jobs. |

### npm (`pulsehive-js/scripts/npm-publish.sh`)

Every line is printed on stderr as `npm-publish: ERROR: <message>`.

| Line (as printed) | Class | Action |
|---|---|---|
| `no npm token in the npm environment — refusing` | settings | `NPM_TOKEN` is not in the `npm` environment (or the environment secret was not picked up). Apply step 2 of `## One-time settings`; re-run. |
| `is already on npm with different bytes (registry <a>, tested <b>) — refusing` | corrective | A published version differs from the tested tarball. Never overwrite: fix on `main`, patch-bump, new tag; record the disposition. |
| `… — refusing — this is a fresh build; re-run the failed jobs of the original tag run instead` | transient | A `workflow_dispatch` run would publish fresh bytes. Re-run the original run's failed jobs. |
| `cannot read npm for <name>@<version> (<detail>) — refusing` | transient | The registry read failed (network, or `npm view <spec> printed no integrity`). Re-run the failed jobs. |
| `npm publish failed for <name>@<version> (exit <rc>) — refusing` | transient | The registry rejected or failed the upload. Re-run; if the rejection is a permissions/2FA problem, fix the token (settings). |
| `<name>@<version> was published but npm reports <state>, not the tested <integrity> — refusing` | corrective | The registry holds different bytes after upload. Never overwrite: patch-bump, new tag; record the disposition. |
| `cannot read npm.package from the release manifest` / `release-check.sh --get npm.package printed nothing` / `package.json declares no napi.targets` / `napi target '<t>' has no npm platform package name` / `advertised suffix '<s>' is not in the matrix` | corrective | The packaging inputs in the tagged tree are wrong or have drifted from the advertised matrix. Fix on `main`, cut a new tag. |
| `cannot read package/package.json from <base>` / `<base> holds <name>@<version>, which npm pack would name <expected>` / `<base> holds <name>@<version>; expected version <want>` / `unexpected package name <name> in <base> (expected <a> or <b>-<suffix>)` | corrective | A packed tarball's identity or file name is wrong. Fix the pack path on `main`, cut a new tag. |
| `directory <dir> does not exist` | transient | The tarball artifact was not downloaded. Re-run the original run (artifacts live 90 days); past the window, corrective. |
| `unexpected entry <e> in <dir> (only release tarballs belong there)` / `more than one main <name> tarball (<a> and <b>)` / `more than one <name> tarball (<a> and <b>)` | corrective | The packed set carries a foreign or duplicated file. Fix the pack job on `main`, cut a new tag. |
| `no main <name> tarball in <dir>` / `missing platform tarball for suffix <s> (<name>-<s>)` | corrective | The packed set is incomplete for an advertised target. Fix the pack job on `main`, cut a new tag; re-run first only if the artifact download was partial. |
| `--dist needs a directory` / `--expect-version needs a version` / `--dist <dir> is required` / `--expect-version <v> is required` / `probe takes <name> <version>` / `unknown argument '<x>'` / `unknown argument '<x>' (use probe, plan, publish or --self-test)` | transient | The invocation is wrong. Correct the command and re-run; if a **workflow** step passes the wrong flag, that is a tagged-tree defect — fix it on `main` and cut a new tag (corrective). |

### PyPI (`pulsehive-py/scripts/py-publish-check.sh`)

Every line is printed on stderr as `py-publish-check: ERROR: <message>`.

| Line (as printed) | Class | Action |
|---|---|---|
| `published file '<file>' differs from the tested wheel — refusing` | corrective | A published file differs from the tested wheel. Never overwrite: fix on `main`, patch-bump, new tag; record the disposition. |
| `… — refusing — this is a fresh build; re-run the failed jobs of the original tag run instead` | transient | A `workflow_dispatch` run would publish fresh wheels over a published version. Re-run the original run's failed jobs. |
| `published file '<file>' is not in the tested set — refusing` | corrective | PyPI holds a file this tag never tested. Patch-bump and cut a new tag; record the disposition. |
| `published file '<file>' is yanked on PyPI — not complete; unyanking is the operator's decision (docs/RELEASING.md)` | settings | The operator decides: unyank the file on PyPI and re-run, or take the corrective path. |
| `cannot read the per-version JSON for '<name>' '<version>' — refusing to publish over a state that cannot be proved` / `cannot read the per-version JSON for '<project>' '<version>' — refusing to guess the published state` | transient | The PyPI read failed or the body did not parse. Re-run the failed jobs. |
| `cannot verify PyPI state for '<name>' '<version>' (the per-version JSON probe answered neither 200 nor 404) — refusing to publish an unproven version` / `cannot verify PyPI state for '<project>' '<version>' (the per-version JSON probe answered neither 200 nor 404)` | transient | PyPI answered an unexpected status. Re-run; if it persists, corrective. |
| `cannot create a temp dir` / `cannot create a temp dir for the probe` | transient | A runner-local scratch directory could not be made. Re-run the failed jobs. |
| `verify-published: <fault> (after <n> attempt(s))` | transient | The post-upload proof did not see every tested wheel within the bounded retries (index lag). Re-run the failed jobs. |
| `no wheels (*.whl) found in '<dir>'` / `dist directory '<dir>' not found` / `non-wheel artifact(s) in '<dir>': <list> — the release publishes wheels (*.whl) only` | transient → corrective | The wheel artifact is missing or contaminated. Re-run the original run to re-fetch it; if the build itself produced this set, fix on `main`, new tag. |
| `mislabelled wheel '<base>': filename is not <name>-<version>-<python>-<abi>-<platform>` | corrective | A wheel's file name is malformed. Fix the build on `main`, cut a new tag. |
| `wheel '<base>' belongs to distribution '<x>', not '<y>'` | corrective | A wheel is from the wrong distribution. Fix the build on `main`, cut a new tag. |
| `wheel '<base>' is built for an excluded host ('<platform>'); advertised targets: <targets>` | corrective | The build produced a wheel outside the advertised matrix (musllinux and every other host are excluded). Fix the matrix on `main`, cut a new tag. |
| `version disagreement: wheel '<base>' carries version '<v>' but --expect-version is '<w>' (compared as PEP 440 versions)` | corrective | The wheel and the manifest disagree. Fix on `main`, cut a new tag. |
| `wheel '<base>' has no readable *.dist-info/METADATA — nothing proves which distribution and version it carries` / `wheel '<base>' METADATA says Name '<a>', expected '<b>'` / `wheel '<base>' METADATA says Version '<a>', expected '<b>'` | corrective | The wheel's own metadata is missing or wrong. Fix the build on `main`, cut a new tag. |
| `missing wheel for advertised target '<target>' (looked in '<dir>')` | corrective | An advertised target produced no wheel. Fix the build matrix on `main`, cut a new tag. |
| `cannot hash wheel '<file>'` | transient | A runner-local read failure. Re-run the failed jobs. |
| `internal: no platform pattern for target '<target>'` | corrective | The script's own target table has drifted from the advertised matrix. Fix the tree on `main`, cut a new tag. |
| `missing the version rule <path> (it ships next to this script)` | corrective | The tree is incomplete (`lib/pep440.sh` absent). Fix on `main`, cut a new tag. |

## Partial releases

When one registry publishes and another does not, the release is **not** complete
until the missing registry has a recorded disposition. The operator decides the
disposition and fills this table into the release's GitHub Release notes; no seat
and no workflow creates the Release.

| Registry | Version | State | Decided by | Date |
|---|---|---|---|---|
| crates.io | | published / resumed / abandoned / corrective | | |
| npm | | published / resumed / abandoned / corrective | | |
| PyPI | | published / resumed / abandoned / corrective | | |

- **published** — the registry holds the tested artifacts for this version.
- **resumed** — the failed jobs of the original run were re-run and completed.
- **abandoned** — the registry stays at its previous version; the release notes
  say so and the next release carries the change.
- **corrective** — a new patch version and tag supersede this one for that
  registry (or for all three).

**Who decides:** the operator (`draco28`), on the evidence of the run's job
summaries.

**What consumers are told meanwhile:** the GitHub Release notes carry the table
above plus one line per registry naming the version a consumer gets — the
published one where it published, the previous version where it did not — and
that the tag was not moved. Pinning consumers are unaffected: versions already
published stay published, and a corrective release moves all three registries to
the new version together.

## Never

- **Move, delete or re-cut a `v*` tag.** A corrective release cuts a *new* tag.
- **Republish a version with different bytes.** The identity checks refuse it;
  do not work around them.
- **Publish from a workstation.** Every upload runs from the tag's workflow
  behind its environment approval.
- **Approve a run whose gate did not pass** — or whose tagged commit is not on
  `main`, or whose workflow at that commit is not the gated one.
