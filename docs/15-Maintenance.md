# PulseHive SDK — Maintenance Plan

> **Document ID:** OPS-PH-015
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** SPEC v0.4.0

---

## 1. Overview

PulseHive is a pre-1.0 Rust SDK maintained by a solo developer with AI-assisted development (Claude Code). This document defines the maintenance processes that keep the SDK healthy, secure, and evolving. It covers dependency management, breaking change strategy, documentation upkeep, community practices, and long-term sustainability.

---

## 2. Dependency Management

### 2.1 Update Schedule

| Cadence | Action | Tool |
|---------|--------|------|
| Weekly | Review Dependabot security alerts | GitHub Dependabot |
| Monthly | Run `cargo update` across workspace | Manual |
| Per-release | Audit full dependency tree | `cargo audit` |
| Quarterly | Evaluate major dependency upgrades (Tokio, Serde, tracing) | Manual |

### 2.2 Routine Updates

```bash
# Monthly: update all compatible dependencies within semver bounds
cargo update

# Check for security advisories
cargo audit

# Check for outdated dependencies (available newer major versions)
cargo outdated --workspace
```

After `cargo update`, run the full test suite and benchmarks before committing. Dependency updates that change behavior (even within semver bounds) occasionally cause regressions.

### 2.3 Security Patches

Dependabot is configured to open PRs for known vulnerabilities. Response SLA:

| Severity | Response Time | Action |
|----------|--------------|--------|
| Critical (RCE, data exposure) | < 24 hours | Patch and publish new version immediately |
| High (denial of service, privilege escalation) | < 72 hours | Patch in next release, accelerate if needed |
| Medium (information leak, minor DoS) | Next scheduled release | Include in regular update cycle |
| Low (theoretical, unexploitable) | Next scheduled release | Track in issue, address when convenient |

### 2.4 Key Dependencies and Their Roles

| Dependency | Role | Update Sensitivity |
|------------|------|-------------------|
| `pulsehive-db` (PulseDB) | Storage substrate | High — API changes require PulseHive changes |
| `tokio` | Async runtime | Medium — major versions rare, minor updates safe |
| `serde` | Serialization | Low — extremely stable API |
| `tracing` | Observability | Low — stable API, subscriber ecosystem |
| `async-trait` | Trait async support | Low — will be replaced by native async traits |
| `reqwest` | HTTP client (providers) | Medium — TLS and HTTP changes |
| ONNX Runtime (via PulseDB) | Embedding inference | Low — managed by PulseDB |

---

## 3. PulseDB Version Coordination

PulseDB and PulseHive are maintained by the same developer in separate repositories. Coordination is manual but disciplined.

### 3.1 Version Compatibility Matrix

| PulseHive Version | PulseDB Version | Notes |
|-------------------|-----------------|-------|
| 0.1.x | 0.1.x | Initial release, SubstrateProvider v1 |
| 0.2.x | 0.1.x or 0.2.x | Intelligence layer, may need new substrate methods |
| 0.3.x | 0.2.x+ | Python bindings, possible new PulseDB methods |

### 3.2 Coordination Process

1. **PulseDB breaking change planned**: First update PulseHive to work with the new API on a feature branch. Test both together locally. Publish PulseDB first, then PulseHive.
2. **PulseHive needs a new substrate method**: Implement in PulseDB first. Add to `SubstrateProvider` trait. Publish PulseDB. Then update PulseHive to use it.
3. **Both need changes**: Coordinate in a single work session. Use path dependencies during development, switch to crates.io versions before publishing.

```toml
# During development (local path dependency)
[dependencies]
pulsehive-db = { path = "../PulseDB" }

# Before publishing (crates.io dependency)
[dependencies]
pulsehive-db = "0.2"
```

---

## 4. Breaking Change Management

### 4.1 Pre-1.0 Policy (Current)

Breaking changes are permitted at minor version boundaries (0.1.x to 0.2.0). Every breaking change:

1. Gets a deprecation warning in the current version (when feasible).
2. Is documented in CHANGELOG.md with a migration guide.
3. Is announced in the GitHub release notes.

**Deprecation pattern:**

```rust
// In 0.1.x: deprecate the old API
#[deprecated(since = "0.1.5", note = "Use LlmConfig::new() instead")]
pub fn llm_config_from_model(model: &str) -> LlmConfig { ... }

// In 0.2.0: remove the deprecated API
// (users had at least one minor version to migrate)
```

### 4.2 Post-1.0 Policy (Future)

Once PulseHive reaches 1.0:

- Breaking changes only at major versions (1.x to 2.0).
- Minimum 6-month deprecation period before removal.
- Migration tools (cargo fix, codemods) provided for significant API changes.
- LTS support for the previous major version (security patches for 12 months).

### 4.3 What Counts as Breaking

| Change | Breaking? | Notes |
|--------|-----------|-------|
| Removing a public type or method | Yes | Always a major version change post-1.0 |
| Changing a trait method signature | Yes | All implementors must update |
| Adding a required method to a trait | Yes | All implementors must update |
| Adding an optional method with default impl | No | Existing implementors unaffected |
| Adding a new variant to a non-exhaustive enum | No | `#[non_exhaustive]` enums allow this |
| Adding a new public type | No | Additive change |
| Changing default configuration values | Depends | Document in CHANGELOG, consider it breaking if behavior changes significantly |
| Adding a new feature flag | No | Features are opt-in |

---

## 5. Documentation Maintenance

### 5.1 Documentation Layers

| Layer | Location | Update Trigger | Owner |
|-------|----------|---------------|-------|
| API reference | Generated by docs.rs from doc comments | Every `cargo publish` | Automated |
| EXECUTIVE-SUMMARY.md | Repository root | Architecture or design changes | Manual |
| CHANGELOG.md | Repository root | Every release | Manual |
| Operations docs (`docs/`) | `docs/` directory | Process or architecture changes | Manual |
| Examples | `examples/` directory | API changes that break examples | Manual |

### 5.2 docs.rs

API documentation is auto-generated from doc comments on every crates.io publish. Maintenance tasks:

- Ensure all public items have `///` doc comments.
- Include `# Examples` sections in doc comments for key types and methods.
- Run `cargo doc --no-deps` locally before publishing to catch broken links.
- Use `#[doc(hidden)]` for internal items that must be public for technical reasons but should not appear in documentation.

### 5.3 EXECUTIVE-SUMMARY.md & docs/

The specification is the source of truth. It is a living document updated when:

- A new primitive or concept is introduced.
- An architectural decision is made or revised.
- An open question is resolved.
- Phase milestones are reached.

EXECUTIVE-SUMMARY.md summarizes the spec; the detailed specification lives across `docs/01-PRD.md`, `docs/02-SRS.md`, and `docs/03-Architecture.md`. Keep them in sync with significant changes.

### 5.4 CHANGELOG.md

Follows [Keep a Changelog](https://keepachangelog.com/) format. Every published version has an entry. Categories: Added, Changed, Deprecated, Removed, Fixed, Security. Breaking changes are called out explicitly with migration instructions.

---

## 6. Community Management

### 6.1 GitHub Issues

**Triage process:**

| Label | Meaning | Response SLA |
|-------|---------|-------------|
| `bug` | Confirmed defect | Acknowledged within 48 hours |
| `security` | Security vulnerability | Acknowledged within 24 hours, fixed ASAP |
| `feature` | Feature request | Acknowledged within 1 week |
| `question` | Usage question | Answered within 1 week |
| `good-first-issue` | Suitable for new contributors | Always keep 3-5 open for onboarding |
| `wontfix` | Out of scope or against design | Explain rationale when closing |

### 6.2 Pull Request Review

**SLA:** Acknowledge PRs within 72 hours. Provide substantive review within 1 week.

**Review criteria:**

- Does it follow the code style (fmt, clippy, doc comments)?
- Does it have tests?
- Does it change public API? If so, is it backward-compatible?
- Does it align with the architecture in docs/03-Architecture.md?
- Are there performance implications? (Check benchmarks if relevant.)

### 6.3 Contributor Recognition

- Contributors listed in CHANGELOG.md for their first contribution.
- Significant contributors added to a CONTRIBUTORS.md file.
- Community contributions credited in release notes.

---

## 7. Long-Term Roadmap

### 7.1 Planned Phases

| Phase | Timeline | Key Deliverables |
|-------|----------|-----------------|
| Phase 1: Foundation | Weeks 1-4 | Core traits, single LlmAgent, PulseDB integration, basic events |
| Phase 2: Multi-Agent | Weeks 5-8 | Workflow agents, intelligence layer, Lens perception, Watch integration |
| Phase 3: Polish + Python | Weeks 9-12 | PyO3 bindings, human-in-the-loop, observability polish, benchmarks |
| Phase 4: Ecosystem | Weeks 13-16 | napi-rs TypeScript bindings, WisdomAbstractor, advanced field dynamics |

### 7.2 Post-Phase 4 Horizon

| Item | Description | Trigger |
|------|-------------|---------|
| PostgresSubstrate | Cloud-native substrate for server deployments | When a product needs server-side PulseHive |
| WisdomAbstractor | Cross-collective pattern sharing | After sufficient production data validates patterns |
| REFRAG optimization | Direct embedding injection into LLM decoders | When LLM providers add decoder-level APIs |
| PulseHive Cloud | Managed substrate hosting service | When community demand justifies |
| Custom embedding providers | Domain-specific models (medical, code, multilingual) | When default MiniLM is insufficient |
| Agent marketplace | Community-contributed agent templates | When the SDK reaches 1.0 |

---

## 8. Bus Factor Mitigation

PulseHive is currently maintained by a solo developer. The following practices reduce the risk of the project stalling if the maintainer becomes unavailable.

### 8.1 Documentation as Insurance

- **docs/03-Architecture.md** captures the architecture and design decisions with rationale.
- **CLAUDE.md** provides complete context for AI-assisted development continuity.
- **Doc comments** explain not just what code does, but why.
- **CHANGELOG.md** provides a complete history of decisions and changes.
- **discussion.md** preserves the design thinking behind non-obvious choices.

### 8.2 Test Coverage as Safety Net

- Unit tests for every public API method.
- Integration tests for the agentic loop, intelligence layer, and substrate operations.
- Criterion benchmarks as performance regression tests.
- Example programs that serve as end-to-end smoke tests.
- CI runs tests on three platforms (Linux, macOS, Windows) to catch platform-specific issues.

### 8.3 AI-Assisted Development Continuity

PulseHive is developed with Claude Code. The CLAUDE.md file contains sufficient context for any Claude instance to continue development:

- Project overview and architecture.
- Crate structure and dependency relationships.
- Key documents and their purposes.
- PulseDB relationship and API surface.
- Development commands and workflows.

If a new maintainer (human or AI-assisted) takes over, reading CLAUDE.md + EXECUTIVE-SUMMARY.md provides a complete onboarding path.

### 8.4 Code Organization

- Small, focused crates with clear boundaries.
- No god objects. HiveMind delegates to specialized components (RelationshipDetector, InsightSynthesizer, ContextOptimizer).
- Traits define extension points. Adding a new LLM provider requires implementing one trait, not understanding the entire codebase.
- Standard Rust patterns (builder, type state, async-trait) that any experienced Rust developer recognizes.

---

## 9. Operational Health Checks

### Monthly

```
[ ] Run cargo update and verify tests pass
[ ] Review and address Dependabot alerts
[ ] Check docs.rs build status for all crates
[ ] Review open issues — close stale ones, update priorities
```

### Per-Release

```
[ ] Full test suite green on all platforms
[ ] Benchmarks show no regressions > 10%
[ ] CHANGELOG.md updated with all changes
[ ] Examples compile and run correctly
[ ] cargo doc builds without warnings
[ ] cargo clippy produces zero warnings
```

### Quarterly

```
[ ] Review EXECUTIVE-SUMMARY.md + docs/03-Architecture.md — do they still match reality?
[ ] Evaluate major dependency upgrades
[ ] Review roadmap priorities
[ ] Check if any deprecated APIs are ready for removal
[ ] Assess community health (issue response times, PR review times)
```

---

*This document is maintained alongside the SDK. Updated as maintenance processes evolve.*
