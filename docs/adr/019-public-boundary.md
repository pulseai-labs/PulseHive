# ADR 019: Public Boundary Made Machine-Checkable

**Status:** Accepted
**Category:** Trust boundaries
**Touch Surface:** `PUBLIC_BOUNDARY.md`, `scripts/check-public-boundary.sh`, `.gitignore`, `.github/workflows/boundary.yml`
**Revisit Trigger:** a private module, private repo or commercial overlay is introduced; the posture changes; or a second maintainer joins (exception approval)

## Context

`PUBLIC_BOUNDARY.md` stated the public/private boundary as prose, and prose is
read by whoever remembers to read it. Release 1's boundary audit found what that
costs: a boundary that could not return a verdict on its own, files present but
untracked in a public tree, and no history pass at all. Nothing executed the
document, so "the tree obeys the boundary" was an opinion, and a prohibited file
was caught — if at all — at a release close, long after it merged.

This repository is public: every commit, and every path ever tracked, is
disclosed permanently. The boundary therefore has two halves that fail
differently. A **rules** half is mechanical — a path either matches a
`never-tracked:` pattern or it does not — and it can be executed on every pull
request. A **semantic** half is judgment — is this fixture synthetic, does this
paragraph disclose a plan — and no pattern can decide it. Release 2 makes the
first half machine-checkable and names the second half as a judgment pass with
a named owner, so the boundary audit gives a definitive verdict instead of an
inconclusive one.

This ADR rides ADR-005 (the public contract surface and its stability promise),
ADR-006 (trust boundaries: what may leave this repository), ADR-007 (failure
visibility: every refusal is a named line, never a silent narrowing) and
ADR-018 (the publish gate, which this boundary sits behind).

## Decision

The boundary is written as data in `PUBLIC_BOUNDARY.md` and executed by
`scripts/check-public-boundary.sh`: a `never-tracked:` rules block matched
against `git ls-files`, a working-tree hygiene allowlist classified against
`git ls-files --others`, and the prose rules a judgment pass reads. The rules
block is read from the index, not the working tree, so an unstaged edit cannot
weaken the policy the checker enforces (C10). A block that is absent, empty,
unparseable, carries a directive the checker cannot execute, or is missing a
rule the template ships is INCONCLUSIVE — never clean, because a rule that
vanished checks nothing and a check that read nothing looks exactly like a check
that passed. `fixtures-must-be: synthetic` is a judgment rule: the checker
reports it as a note and never claims it as checked.

### Posture

The project's posture is **`fully-open`** with an **empty moat**. This
supersedes the `open-core` posture recorded at onboarding, and the reason is
that the moat it assumed was never built: the intelligence layer, the provider
adapters and the bindings all ship in this public repository, there is no
private functionality component, and the revenue intent is `none`. An empty moat
inventory means the moat channel is `none` — there is no item whose carrier
would justify a `data-overlay`, `private-package` or `repo-private` channel.
Tightening *documentation routing* (below) is an artifact-routing rule, not a
moat channel, and it does not change the posture. The revisit trigger is a
private module, repo or overlay appearing: that is a posture change and a
supersede ceremony, not an edit to this file.

### Documentation routing

User-facing and contributor-facing documentation is public: the README, the
`docs/` guides, the API and PulseDB references, the ADRs, the security and
licensing files, and everything under `.github/`. Project planning is private:
the roadmap, the PRD, the SRS, the backlog and the project plan moved to the
private AI workspace, and `never-tracked:` patterns stop them from returning.

**There is no history rewrite, and that is deliberate.** The five documents were
already published; a rewrite neither un-publishes them nor un-clones them, and
it would break every clone, link and release record that points at this history.
The honest disposition for material that was public and should not have been is
a pinned Accepted-disclosures row, not a rewrite that pretends it never was.

### Downstream references

No downstream product name and no downstream metric appears in this repository
outside `CHANGELOG.md` release history. The names of the products built on
PulseHive are themselves part of the private side, so the list that names them
cannot be published: it lives in the private AI workspace, and the checker reads
it through an optional `--deny-terms <file>`. Public CI runs the rules block and
the secrets scan without it; the release-close audit and local runs from the AI
workspace pass it. A failed term check names the file and the line and never
prints the term. The exemption is narrow: only `CHANGELOG.md` content **below**
its first released `## [x.y.z]` heading, because that section is the published
release record. `## [Unreleased]` is checked like any other file (C5).

### History passes

The history pass is in scope and has two halves. The **machine half** is
gitleaks over the full history, every public ref, with `--redact` (an unredacted
hit would print the secret onto the exact path that handles leaks) and with
`--log-opts="--all -m"` so a secret introduced while resolving a merge is
scanned rather than skipped. It runs with gitleaks' built-in default config —
`.gitleaks.toml` and `.gitleaksignore` are themselves `never-tracked:`, so no
repository config can weaken or extend an allowlist — and the CI job fetches the
pull-request refs before scanning so an unmerged head is covered too (C2, C3).
The **judgment half** is a path-first review of history against the rules block
and the prose rules, finding by finding, because no pattern distinguishes a
synthetic fixture from a real one.

The pass is recorded as a **History passes** row in the private boundary
inventory — repo, reviewed-through commit, date — and the row is written by the
release-close boundary audit when the operator confirms the review, never by an
agent that reviewed its own work. The commit column is what makes a row expire:
commits after the recorded commit are unreviewed again.

### What blocks a release close, and exceptions

The release close is blocked by: a tracked-rule match; a gitleaks hit; an S1
semantic finding; a missing or INCONCLUSIVE rules block or boundary inventory;
and commits after the last History passes row that no review has covered.

A **standing warning** is listed in the release-close report and does not block:
an S2 note, an allowlisted hygiene hit, or an Accepted-disclosures row whose
pinned blob is unchanged. Nothing is auto-dispositioned: every row is put in
front of the operator in the close report, and a warning that nobody lists is a
warning that nobody reads.

An **exception** is an `Accepted disclosures` row that only the operator
approves, carrying the release, the finding, the pinned surface (path, blob hash
and commit), the reason and the date. Agents propose rows and never approve
them. **A row covers only its pinned blob**, so a file that changes after the
row re-raises the finding, and nothing carries forward by copying. An allowlist
entry added because of a finding is itself such a row: editing the allowlist to
quiet a hit is exactly the failure the pinning exists to prevent.

### CI enforcement

`.github/workflows/boundary.yml` runs on every pull request and on every push to
`main`, with `contents: read`, and runs no checkout of the tree it may not
trust: two jobs, **"Public boundary"** — the checker's hermetic self-test, then
the checker — and **"Secrets scan (gitleaks)"** — a checksum-verified gitleaks
binary, then a full-history scan. The gitleaks release is downloaded and its
tarball checked against a sha256 literal in the workflow, `actions/checkout` is
pinned to a commit SHA, and the scanner is never invoked through
`gitleaks/gitleaks-action` (which needs a paid licence on organization repos).
Both jobs avoid `continue-on-error`, `|| true` and `set +e`: a check that cannot
fail is not a check.

Both checks become **required status checks on `main`**, so a prohibited file is
caught before it merges rather than at the next release close.

One consequence of keeping the template's `**/.env.*` rule verbatim — a block
missing a template rule is INCONCLUSIVE — is that a tracked file matching it
cannot be exempted without dropping the rule. The tracked `.env.example`
template was therefore renamed to `env.example`, which no `.env*` rule matches,
and `CONTRIBUTING.md` and `.gitignore` follow the new name.

## Consequences

**Positive:**

- The tracked tree's verdict is mechanical and reproducible: the same rules run
  locally, in CI, and at the release close, and every finding names its file and
  the rule it broke.
- A prohibited file is refused at pull-request time. The cheapest place to stop
  a leak is the PR that introduces it, before it is history.
- A rule that disappears is INCONCLUSIVE rather than silently absent, so the
  boundary cannot decay into a document nobody executes.
- The history pass is anchored to a commit and to retrieved refs, so "when was
  this reviewed" has an answer that expires by itself.
- Nothing is auto-dispositioned, so a standing warning reaches the operator
  instead of a log.

**Negative:**

- The scanner is a downloaded binary with a pinned checksum; bumping gitleaks
  is a deliberate two-line change and the checksum must be re-resolved, not
  guessed.
- The checker is another script to keep green: a rules-block edit that drops a
  template line fails CI with an INCONCLUSIVE verdict rather than passing.
- The judgment half does not scale down to a pattern: an S1 finding is decided
  by a reviewer, and its exception needs the operator, so a release can be
  blocked by a question no script can answer.
- The rename of `.env.example` to `env.example` is a contributor-visible change
  to a public template path; the files that referenced it were updated with it.
- Public CI cannot check the deny-term list without publishing it, so PR-time
  name checking relies on the reviewer and on the release-close audit.

## Alternatives

- **Prose only, no executable block.** Rejected: that is the state this ADR
  replaces, and it is the state that produced an INCONCLUSIVE audit.
- **Hashed deny terms in public CI.** Rejected (H1): product names are
  low-entropy and guessable from their hashes, so the hash list would disclose
  what it exists to protect. The list stays private and the public run goes
  without it.
- **Narrow the template rule to exclude `.env.example`.** Rejected: a block
  missing a template rule is INCONCLUSIVE, so narrowing trades a real check for
  a convenience. The file was renamed instead.
- **Rewrite history to remove what was published.** Rejected: it cannot
  un-publish anything, breaks clones and links, and hides the disclosure that
  the record must carry.
- **Scan only the current ref.** Rejected: a secret introduced on an unmerged
  branch, a tag or a pull-request head is still one push from public. The CI job
  fetches the pull-request refs and the scan is merge-aware.
- **Use `gitleaks/gitleaks-action`.** Rejected: it needs a paid licence on
  organization repositories; the release tarball with a checksum literal keeps
  the scan free, verifiable and version-pinned.
- **Auto-fix or auto-exempt a finding.** Rejected: an exception only the
  operator approves is the whole point, and an agent that could clear its own
  finding would make the audit self-certifying.
