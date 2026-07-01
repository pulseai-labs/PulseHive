# PulseHive SDK — Release & Distribution Guide

> **Document ID:** OPS-PH-009
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** SPEC v0.4.0

---

## 1. Overview

PulseHive is a Rust library crate published to crates.io. There are no servers to deploy, no containers to orchestrate, no infrastructure to provision. "Deployment" means publishing a new version of the SDK that downstream products can `cargo add`. This document covers the complete release workflow from version bump to crate publication.

---

## 2. Crate Topology & Publish Order

PulseHive is a Cargo workspace containing five crates with strict dependency ordering. Publishing must follow this order because crates.io resolves dependencies at publish time.

```
Publish Order (sequential, cannot be parallelized):

  1. pulsehive-core         ← No internal dependencies (only pulsedb, serde, async-trait)
  2. pulsehive-runtime      ← Depends on pulsehive-core, pulsedb, tokio, tracing
  3. pulsehive-anthropic    ← Depends on pulsehive-core
  4. pulsehive-openai       ← Depends on pulsehive-core
  5. pulsehive              ← Meta-crate, depends on all of the above
```

Each crate has its own `Cargo.toml` with an independent version number, though in practice all crates share the same version to avoid confusion. The meta-crate `pulsehive` re-exports everything and provides feature flags.

---

## 3. Cargo Feature Flags

The meta-crate exposes compile-time feature flags so products only pull in the LLM providers they need:

```toml
# Product's Cargo.toml
[dependencies]
pulsehive = { version = "0.1", features = ["anthropic"] }

# Or for OpenAI-compatible providers (GLM, vLLM, Ollama, LM Studio)
pulsehive = { version = "0.1", features = ["openai"] }

# Both providers
pulsehive = { version = "0.1", features = ["anthropic", "openai"] }
```

**Feature flag design principles:**

- **No default features.** Products opt in explicitly. A bare `pulsehive = "0.1"` gives you core traits and runtime but no LLM provider.
- **Additive only.** Features never remove functionality. Enabling `anthropic` adds the `AnthropicProvider`; it does not disable anything.
- **No feature interactions.** Enabling both `anthropic` and `openai` is the same as enabling each independently. No conditional compilation gates that depend on feature combinations.

The meta-crate's `Cargo.toml` feature section:

```toml
[features]
anthropic = ["dep:pulsehive-anthropic"]
openai = ["dep:pulsehive-openai"]
```

---

## 4. Semantic Versioning Strategy

### Pre-1.0 (Current: 0.x.y)

PulseHive follows Rust ecosystem conventions for pre-1.0 crates:

| Version Component | Meaning | Example |
|---|---|---|
| `0.x.0` | May contain breaking changes to public API | 0.1.0 to 0.2.0 |
| `0.x.y` | Bug fixes, documentation, non-breaking additions | 0.1.0 to 0.1.1 |

**Key rules:**

- Breaking changes (trait signature modifications, removed public types, changed semantics) increment the minor version: `0.1.x` to `0.2.0`.
- Non-breaking additions (new optional methods with defaults, new types, new feature flags) can land in patch versions.
- Every breaking change gets a migration note in CHANGELOG.md.

### Post-1.0 (Future)

Once the trait boundaries stabilize (target: after Phase 2), the crate moves to 1.0.0 and follows standard semver:

- **Major** (2.0.0): Breaking changes to public API.
- **Minor** (1.1.0): New features, backward-compatible additions.
- **Patch** (1.0.1): Bug fixes only.

The 1.0 milestone requires: all five core primitives stabilized, at least two LLM providers shipping, and at least one production product consuming the SDK successfully.

---

## 5. Release Checklist

Every release follows this checklist. No exceptions.

### 5.1 Pre-Release Verification

```
[ ] All CI checks pass (cargo test, cargo clippy, cargo fmt --check)
[ ] Cross-platform CI green (Linux x86_64, macOS ARM64, Windows x86_64)
[ ] No new cargo clippy warnings with -D warnings
[ ] cargo doc builds without warnings
[ ] Integration tests pass against current PulseDB version
[ ] Examples in examples/ directory compile and run correctly
```

### 5.2 Version Bump

```bash
# Update version in all Cargo.toml files (maintain lockstep versions)
# pulsehive-core/Cargo.toml
# pulsehive-runtime/Cargo.toml
# pulsehive-anthropic/Cargo.toml
# pulsehive-openai/Cargo.toml
# pulsehive/Cargo.toml

# Also update inter-crate dependency versions:
# pulsehive-runtime depends on pulsehive-core = "0.x.y"
# pulsehive depends on pulsehive-core = "0.x.y", pulsehive-runtime = "0.x.y", etc.
```

### 5.3 Changelog Update

CHANGELOG.md follows the [Keep a Changelog](https://keepachangelog.com/) format:

```markdown
## [0.2.0] - 2026-04-15

### Added
- Workflow agents: Sequential, Parallel, Loop variants
- InsightSynthesizer for cross-experience synthesis

### Changed
- **BREAKING**: `LlmProvider::chat()` now takes `&LlmConfig` instead of `LlmConfig`

### Fixed
- ContextOptimizer panic on empty experience set

### Migration
- Update all `LlmProvider` implementations to take `&LlmConfig` by reference
```

### 5.4 Publish

```bash
# Dry run first (catches missing fields, invalid metadata)
cargo publish -p pulsehive-core --dry-run
cargo publish -p pulsehive-runtime --dry-run
cargo publish -p pulsehive-anthropic --dry-run
cargo publish -p pulsehive-openai --dry-run
cargo publish -p pulsehive --dry-run

# Publish in dependency order
cargo publish -p pulsehive-core
# Wait for crates.io index update (~30-60 seconds)
cargo publish -p pulsehive-runtime
cargo publish -p pulsehive-anthropic
cargo publish -p pulsehive-openai
cargo publish -p pulsehive
```

### 5.5 Post-Publish

```bash
# Tag the release
git tag -a v0.2.0 -m "Release v0.2.0"
git push origin v0.2.0

# Verify on crates.io
# https://crates.io/crates/pulsehive

# Verify docs.rs build
# https://docs.rs/pulsehive
```

---

## 6. Yanking Strategy

If a published version has a critical bug (data corruption, security vulnerability, compilation failure on a major platform):

```bash
# Yank the broken version (does NOT delete it, just prevents new downloads)
cargo yank --version 0.1.3 pulsehive

# Publish a fixed patch version
cargo publish -p pulsehive  # now at 0.1.4
```

**Yanking rules:**

- Yank immediately for security vulnerabilities or data corruption bugs.
- Yank within 24 hours for compilation failures on tier-1 platforms.
- Do NOT yank for minor bugs that have workarounds. Publish a patch instead.
- Always publish a replacement version before or immediately after yanking.
- Document yanked versions in CHANGELOG.md with the reason.

---

## 7. PulseDB Version Coordination

PulseHive depends on PulseDB (`pulsehive-db` crate). Version coordination is critical:

```toml
# pulsehive-core/Cargo.toml
[dependencies]
pulsehive-db = "0.1"  # Accept any 0.1.x patch
```

**Rules:**

- Use pessimistic version constraints (`"0.1"` not `"0.1.1"`) to accept compatible patches.
- When PulseDB makes a breaking change (new minor version), PulseHive must also release a new minor version.
- Test PulseHive against the latest PulseDB before every release.
- Since both codebases are maintained by the same developer, coordinate breaking changes across both repos before publishing either.

---

## 8. CI/CD Pipeline

### GitHub Actions Workflow

```yaml
# Triggers: push to main, pull requests, release tags
test:
  matrix:
    os: [ubuntu-latest, macos-latest, windows-latest]
    rust: [stable, beta]
  steps:
    - cargo fmt --check
    - cargo clippy -- -D warnings
    - cargo test --all-features
    - cargo doc --no-deps

publish:
  needs: test
  if: startsWith(github.ref, 'refs/tags/v')
  steps:
    - cargo publish (in dependency order)
```

### Required Checks Before Merge

- `cargo test` passes on all three platforms.
- `cargo clippy` produces zero warnings.
- `cargo fmt --check` passes (no formatting drift).
- `cargo doc` builds without warnings (broken doc links fail the build).

---

## 9. Future Distribution Channels

### Phase 3: PyPI (Python Bindings)

```bash
# pulsehive-py crate builds a Python wheel via maturin
maturin publish  # publishes to PyPI as "pulsehive"

# Users install via pip
pip install pulsehive
```

### Phase 4: npm (TypeScript Bindings)

```bash
# pulsehive-js crate builds an npm package via napi-rs
npm publish  # publishes to npm as "@pulsehive/core"

# Users install via npm
npm install @pulsehive/core
```

### Documentation Site

- **docs.rs** auto-generates API documentation from doc comments on every crates.io publish.
- A dedicated documentation site (mdBook or similar) will host guides, tutorials, and architecture explanations once the SDK reaches 0.2.0.

---

## 10. Release Cadence

| Phase | Target Cadence | Notes |
|-------|---------------|-------|
| Phase 1 (Foundation) | Weekly patches, minor on milestone | Rapid iteration, API still forming |
| Phase 2 (Multi-Agent) | Biweekly patches, minor on milestone | API stabilizing, less churn |
| Post-1.0 | Monthly patches, quarterly minors | Stability-focused, deprecation cycle |

**Hotfix process:** For critical bugs, skip the regular cadence. Fix, test, publish patch immediately.

---

## 11. Artifact Verification

Every published crate should be verifiable:

- **Cargo checksum:** crates.io generates SHA-256 checksums automatically.
- **Git tag correlation:** Every published version has a corresponding git tag. Users can verify the tag matches the published source.
- **Reproducible builds:** `cargo build --release` with the same Rust toolchain version produces identical binaries. Pin the Rust version in `rust-toolchain.toml`.

```toml
# rust-toolchain.toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

---

*This document is maintained alongside the SDK. Updated with each significant change to the release process.*
