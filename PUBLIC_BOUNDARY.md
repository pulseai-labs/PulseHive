# Public Boundary

This repository (`pulseai-labs/PulseHive`) is the **public, open-source** home of the PulseHive Cargo workspace — the `pulsehive`, `pulsehive-core`, `pulsehive-runtime`, `pulsehive-anthropic`, and `pulsehive-openai` crates, plus the PyO3 binding (`pulsehive-py`) and the napi-rs binding (`pulsehive-js`). This document states what belongs in public and what must stay internal, so contributors and AI tooling never leak private material into a published artifact.

The three blocks at the end of this file are machine-checkable. `scripts/check-public-boundary.sh` executes the rules block and the allowlist in CI on every pull request, and the release-close boundary audit executes them at every release close (`docs/adr/019-public-boundary.md`).

## What is public (intentionally)

- The source (`src/`), tests, benchmarks, and `examples/` of every workspace member crate listed above.
- Public API documentation, the README, CHANGELOG, and governance docs (this file, [`SECURITY.md`](./SECURITY.md), [`LICENSING.md`](./LICENSING.md), `CONTRIBUTING.md`).
- CI/release configuration under `.github/`.

## Upstream substrate: PulseDB

PulseHive is **built on PulseDB**. The `pulsehive-db` crate (the public PulseDB substrate) is an **upstream dependency** of this workspace — PulseHive uses it for the persistence layer and for builtin embeddings. PulseDB sits *below* PulseHive in the stack:

- PulseDB is an **upstream** crate this repo depends on. It is never a downstream consumer of PulseHive.
- This repo documents how PulseHive *uses* PulseDB. It does not document PulseDB's own internals, roadmap, or strategy — those live in the PulseDB repo.

## What must NEVER be committed here

- **Secrets**: API keys, tokens, `CARGO_TOKEN`, `.env` files, private keys (`*.pem`, `*.key`, `id_rsa`), credentials. Secret scanning + push protection are enabled, and `.gitignore` covers common patterns — but the first line of defense is not committing them.
- **Downstream product strategy / roadmaps**: PulseHive is an SDK. Any business or product strategy for systems built *on top of* PulseHive belongs in those products' own private repos, never here. This repo documents PulseHive's *own* SDK capabilities only.
- **Customer data**, real datasets, or `*.db` fixtures containing anything non-synthetic. Test fixtures must be synthetic.
- **AI-workspace material**: `CLAUDE.md`, `AGENTS.md`, the memory-bank, `MASTER-SPEC.md`, sprint specs, handoffs, and scaffold tooling live in the **private** AI workspace, not here (already `.gitignore`d).

## Internal vs. public repos

| Concern | Lives in |
|---------|----------|
| Workspace crate code, public docs, releases | **this repo (public)** |
| Project planning — the roadmap, PRD, SRS, backlog and project-plan docs — plus MASTER-SPEC, specs, agent scaffolding and the memory bank | private AI workspace |
| Upstream persistence + embedding substrate | the **PulseDB** repo (public, upstream) |
| Downstream product code & strategy | their own (private) repos |

## If something leaked

Treat any secret that reached git history as **compromised**: rotate it immediately (do not rely on a force-push to erase it). For an accidental disclosure of private material, open a private report per [SECURITY.md](./SECURITY.md).

## Machine-checkable rules

<!-- Executed by `scripts/check-public-boundary.sh` on every pull request and by
     the release-close boundary audit (ADR-019). A block that is absent, empty,
     unparseable, carries a directive the checker cannot execute, or is missing a
     rule the template ships is INCONCLUSIVE — never clean. -->
never-tracked: **/.env, **/.env.*, **/*.pem, **/*.key, **/id_rsa*
never-tracked: **/secrets/**, **/credentials.json
never-tracked: **/SPEC.md, docs/planning/**
never-tracked: ROADMAP.md, docs/01-PRD.md, docs/02-SRS.md, docs/12-Backlog.md, docs/13-Project-Plan.md
never-tracked: MASTER-SPEC.md, .claude/memory-bank/**, docs/specs/**, docs/handoffs/**
never-tracked: .gitleaks.toml, .gitleaksignore
fixtures-must-be: synthetic

## Working-tree hygiene allowlist

<!-- Classes of untracked sensitive files known to exist in local clones, named
     by pattern only — never by content description. Report only: an `unlisted`
     path is a note for the operator, not a failure. -->
- `.env`, `.env.*` (untracked, gitignored)
- `*.db` (local substrates, gitignored)
- `.claude/settings.local.json`, `.claude/scheduled_tasks.lock` (orthogonal local tool state, inside the gitignored `.claude/`)
- `pulsehive-js/*.node`, `pulsehive-js/index.js`, `pulsehive-js/index.d.ts`, `pulsehive-js/npm/**` (local napi build outputs; gitignored by `pulsehive-js/.gitignore`)
- `.pytest_cache/**` (local pytest cache; not gitignored)
- `**/*.so`, `**/*.dSYM/**`, `target/**`, `Cargo.lock` (local build outputs and caches, gitignored)

## Never here (prose rules)

- No secrets, tokens, or credentials of any kind.
- No downstream strategy, roadmap, or competitive material.
- No non-synthetic fixtures — no real user data, ever.
- No AI-workspace material (specs-in-progress, planning docs, agent transcripts).
- No private-side implementation of a declared port; the public repo holds the port, never its private implementation.
- No downstream product names or downstream metrics outside `CHANGELOG.md` release history (ADR-019).
