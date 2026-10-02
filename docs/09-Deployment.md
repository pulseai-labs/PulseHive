# PulseHive SDK — Release & Distribution Guide

> **Document ID:** OPS-PH-009
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Superseded — pointer only (r2.s3.w5)
> **Reference:** SPEC v0.4.0

---

## This document has moved

The release and distribution guide is now **`docs/RELEASING.md`** — the
canonical runbook for cutting, approving and recovering a PulseHive release
across crates.io, npm and PyPI.

The decisions behind it are recorded in
**`docs/adr/018-publish-authorization-and-artifact-integrity.md`** (who may
publish, with which credentials, from which tags, and how each published
artifact is bound to the one that was tested), with the amendments in
`docs/adr/008-rollback.md` and
`docs/adr/016-python-distribution-and-versioning-contract.md`.

Everything this document used to describe — the crate topology and publish
order, the feature flags, and the Rust-only release procedure — is either
covered by `docs/RELEASING.md` or enforced by the release scripts it names.
