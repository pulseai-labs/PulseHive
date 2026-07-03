# Security Policy

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues, discussions, or pull requests.**

Report privately via either channel:

1. **GitHub private vulnerability reporting** (preferred) — use the **"Report a vulnerability"** button on the repository's [Security tab](https://github.com/pulseai-labs/PulseHive/security). This is enabled on this repo.
2. **Email** — **praveensingh2897@gmail.com** with subject `SECURITY: PulseHive`.

Please include, where possible:

- A description of the vulnerability and its impact.
- Steps to reproduce (a minimal proof-of-concept, affected version/commit).
- Any suggested remediation.

## What to expect

- **Acknowledgement** within 5 business days.
- An assessment and, if accepted, a remediation plan with a target timeline.
- Coordinated disclosure: we will agree on a disclosure date and credit you (if you wish) once a fix is released.

Please give us a reasonable window to release a fix before any public disclosure.

## Supported versions

PulseHive is on the `2.x` line; the latest published `2.x` release on crates.io is supported. Security fixes target the **latest published `2.x` release** — older releases are not patched, so please upgrade.

| Version | Supported |
|---------|-----------|
| latest `2.x` | ✅ |
| older | ❌ (upgrade) |

## Scope

In scope: the PulseHive crates — `pulsehive`, `pulsehive-core`, `pulsehive-runtime`, `pulsehive-anthropic`, `pulsehive-openai`, `pulsehive-py`, and `pulsehive-js` — and their build/release pipeline. Out of scope: downstream products built on PulseHive, and issues requiring a compromised host or physical access.
