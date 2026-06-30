# Public Boundary

This repository (`pulseai-labs/PulseHive`) is the **public, open-source** home of the PulseHive Cargo workspace — the `pulsehive`, `pulsehive-core`, `pulsehive-runtime`, `pulsehive-anthropic`, and `pulsehive-openai` crates, plus the PyO3 binding (`pulsehive-py`) and the napi-rs binding (`pulsehive-js`). This document states what belongs in public and what must stay internal, so contributors and AI tooling never leak private material into a published artifact.

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
| Project planning, MASTER-SPEC, specs, agent scaffolding, memory bank | private AI workspace |
| Upstream persistence + embedding substrate | the **PulseDB** repo (public, upstream) |
| Downstream product code & strategy | their own (private) repos |

## If something leaked

Treat any secret that reached git history as **compromised**: rotate it immediately (do not rely on a force-push to erase it). For an accidental disclosure of private material, open a private report per [SECURITY.md](./SECURITY.md).
