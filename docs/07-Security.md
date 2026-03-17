# PulseHive SDK — Security Plan

> **Document ID:** SEC-PH-007
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** SPEC v0.4.0, PRD-PH-001

---

## 1. Introduction

PulseHive is a library crate embedded into host applications. It is not a server, not a web application, and does not expose network endpoints. This fundamentally constrains the threat model: PulseHive's attack surface is the API surface consumed by Rust code at compile time, plus the runtime interactions with LLM providers and PulseDB substrate files.

This document defines the security posture for PulseHive across six domains: LLM credentials, dependency supply chain, input validation, tool execution, substrate data, and LLM prompt injection.

---

## 2. Threat Model

### 2.1 Attack Surface Summary

| Surface | Risk Level | Description |
|---------|-----------|-------------|
| PulseHive API (Rust code) | Low | Compile-time type safety prevents most injection and misuse |
| LLM API keys in memory | Medium | Keys must be held in process memory for provider calls |
| LLM provider network calls | Medium | HTTPS to third-party APIs; response parsing is attack vector |
| PulseDB substrate files | Medium | Local files containing all experiences, embeddings, relationships |
| Dependency tree | Medium | Transitive dependencies may introduce vulnerabilities |
| Tool execution | High (product-controlled) | Products define tools; SDK provides guardrails, not sandboxing |
| LLM prompt injection | High | Adversarial content in experiences or tool results can steer agent behavior |

### 2.2 What PulseHive Does NOT Protect Against

PulseHive is a library, not a security boundary. The following are explicitly out of scope:

- **Host process compromise**: If the host application is compromised, PulseHive's in-process data is exposed. This is inherent to any library.
- **LLM provider-side breaches**: PulseHive sends prompts to third-party LLM APIs. What happens on the provider's infrastructure is outside our control.
- **Product-level authorization**: PulseHive does not implement user authentication or role-based access control. Products built on PulseHive are responsible for gating who can deploy agents and access substrate data.
- **Physical security of substrate files**: PulseHive documents recommended file permissions but cannot enforce them.

---

## 3. LLM API Key Handling

### 3.1 Principles

LLM API keys are the most sensitive data PulseHive handles at runtime. The following invariants are enforced:

1. **Constructor injection only**: API keys are passed into provider constructors. PulseHive never reads keys from environment variables, files, or configuration stores on its own. The host application controls key sourcing.

2. **Memory-only storage**: Keys are stored in the `LlmProvider` implementation struct as `String` fields. They are never written to disk, never serialized, and never included in any `Debug`, `Display`, or `serde::Serialize` implementation.

3. **No logging**: API keys are never emitted via `tracing` at any level (including `TRACE`). Provider structs implement a manual `Debug` that redacts the key field.

4. **No cloning beyond necessity**: Provider structs are wrapped in `Arc` for shared access. The key is stored once per provider instance, not copied per agent.

```rust
pub struct AnthropicProvider {
    api_key: String,  // Never Debug-printed, never serialized
    client: reqwest::Client,
    model: String,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .finish()
    }
}
```

### 3.2 Tracing Span Redaction

All tracing spans that could contain key material are scrubbed. The `LlmProvider` trait contract states that implementations must not include API keys in any `tracing` output. This is documented in the trait's rustdoc and verified by code review.

```rust
// Good: provider name and model, no key
tracing::info!(provider = "anthropic", model = %self.model, "sending LLM request");

// Forbidden: never log headers, auth tokens, or raw request bodies containing keys
```

### 3.3 Zeroization

For consumers with stringent security requirements, PulseHive documents how to use the `zeroize` crate to scrub key material from memory when a provider is dropped. This is not enforced by default (it adds a dependency and marginal overhead) but is demonstrated in the security examples.

---

## 4. Dependency Security

### 4.1 cargo-deny

PulseHive uses `cargo-deny` in CI to enforce:

- **License compliance**: Only allow licenses compatible with AGPL-3.0. Deny copyleft-incompatible licenses (e.g., SSPL, proprietary). Warn on viral licenses that may impose additional restrictions.
- **Vulnerability auditing**: Deny any crate version with a known advisory in the RustSec database. CI fails on any `cargo deny check advisories` finding.
- **Duplicate detection**: Warn on duplicate crate versions in the dependency tree. Two versions of the same crate increase attack surface and binary size.
- **Source restrictions**: Only allow crates from crates.io. No git dependencies in published crates.

```toml
# deny.toml
[advisories]
vulnerability = "deny"
unmaintained = "warn"

[licenses]
allow = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Unicode-DFS-2016", "AGPL-3.0"]
copyleft = "warn"
default = "deny"

[bans]
multiple-versions = "warn"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

### 4.2 Dependabot / Renovate

The GitHub repository enables Dependabot for automated dependency update pull requests. Configuration:

- **Frequency**: Weekly checks for all Cargo dependencies.
- **Security-only mode**: Dependabot security alerts are enabled for immediate notification of vulnerable transitive dependencies.
- **Auto-merge**: Patch updates that pass CI are auto-merged. Minor and major updates require manual review.

### 4.3 Transitive Dependency Review

Before adding any new direct dependency, the following checklist applies:

1. **Audit the crate**: Check crates.io download count, last update date, known advisories.
2. **Review transitive deps**: Run `cargo tree -p <new-dep>` to inspect the full subtree.
3. **Check for `unsafe`**: Run `cargo geiger` to assess unsafe code in the dependency.
4. **Evaluate alternatives**: Prefer crates from well-known maintainers (Tokio, serde ecosystem, RustCrypto).
5. **Minimize feature flags**: Only enable the features actually needed to reduce compiled code.

### 4.4 Direct Dependency Budget

PulseHive aims to keep direct dependencies minimal:

| Crate | Purpose | Justification |
|-------|---------|---------------|
| `tokio` | Async runtime | Standard async runtime for Rust |
| `serde` / `serde_json` | Serialization | Required for JSON Schema tool params, LLM API communication |
| `tracing` | Observability | Standard structured logging for Rust libraries |
| `thiserror` | Error types | Zero-cost derive macro for error enums |
| `async-trait` | Async traits | Required until async fn in traits stabilizes fully |
| `futures` | Stream utilities | `Stream` trait and combinators for event streaming |
| `reqwest` | HTTP client | LLM provider API calls (in provider crates only) |
| `pulsedb` | Substrate | Core storage dependency |

Every additional dependency must justify its inclusion against this budget.

---

## 5. Input Validation

### 5.1 Compile-Time Safety

Rust's type system provides the first line of defense:

- **No SQL injection**: PulseDB uses a Rust API, not SQL strings. There is no query language to inject into.
- **No null pointer dereference**: Rust's `Option<T>` and ownership model prevent null access.
- **No buffer overflow**: Rust's bounds checking prevents out-of-bounds access.
- **No use-after-free**: Ownership and borrowing rules enforced at compile time.
- **No data races**: `Send + Sync` bounds enforced at compile time for all concurrent access.

### 5.2 Runtime Validation

Where compile-time safety is insufficient, PulseHive validates at runtime:

**Experience content**: PulseDB validates that experience content is non-empty and that embeddings have the correct dimensionality (384 for all-MiniLM-L6-v2). PulseHive validates that metadata fields conform to expected schemas before passing to PulseDB.

**Tool parameters**: Tool parameters arrive as `serde_json::Value` from LLM responses. The `Tool` trait requires a `parameters()` method returning a JSON Schema. PulseHive validates incoming parameters against this schema before calling `execute()`. Invalid parameters return a structured error to the LLM for self-correction, not to the tool implementation.

```rust
// PulseHive validates BEFORE calling tool.execute()
let schema = tool.parameters();
validate_json_schema(&params, &schema)?;  // Returns ToolValidationError on mismatch
let result = tool.execute(params, &context).await?;
```

**Agent names**: Agent names are validated to be non-empty, ASCII-safe, and unique within a deployment. This prevents confusion in tracing spans and event streams.

**Collective IDs**: CollectiveId values are validated for format correctness. Products cannot accidentally share substrate data across collectives through malformed IDs.

### 5.3 LLM Response Parsing

LLM responses are parsed through `serde_json` deserialization into typed Rust structs, never through string manipulation or regex extraction. Malformed LLM responses result in typed errors that trigger retry logic, not panics or undefined behavior.

```rust
// Good: structured parsing with typed error handling
let response: LlmResponse = serde_json::from_str(&raw_response)
    .map_err(|e| PulseHiveError::LlmProvider {
        provider: provider_name.into(),
        message: format!("malformed response: {e}"),
    })?;

// Forbidden: string manipulation to extract tool calls
// let tool_name = raw_response.split("tool_call:").nth(1).unwrap();
```

---

## 6. Tool Execution Security

### 6.1 SDK Responsibility vs. Product Responsibility

PulseHive provides the framework for tool execution. Products define what tools do. The security boundary is clear:

| Responsibility | Owner |
|---------------|-------|
| Validating tool parameters against JSON Schema | PulseHive |
| Providing scoped `ToolContext` (not full substrate access) | PulseHive |
| `requires_approval` flag and `ApprovalHandler` integration | PulseHive |
| Emitting tracing events for tool invocations | PulseHive |
| Sandboxing tool execution (filesystem, network, process) | Product |
| Implementing tool-specific authorization logic | Product |
| Rate limiting tool calls | Product (via custom middleware) |

### 6.2 ToolContext Scoping

Tools receive a `ToolContext` that provides access to the substrate scoped to the current collective. Tools cannot access other collectives' data through the provided context.

```rust
pub struct ToolContext {
    pub agent_id: AgentId,
    pub collective_id: CollectiveId,
    pub substrate: Arc<dyn SubstrateProvider>,  // Scoped to collective
    pub event_emitter: EventEmitter,
}
```

The substrate reference in `ToolContext` is the same substrate used by the HiveMind, filtered by `collective_id`. Tools that attempt to query experiences from other collectives receive empty results — PulseDB enforces collective isolation at the storage level.

### 6.3 Human-in-the-Loop Approval

Tools that perform irreversible or sensitive actions declare `requires_approval() -> true`. PulseHive pauses execution and emits a `HiveEvent::ApprovalRequired` event before calling `execute()`. The product's `ApprovalHandler` implementation decides whether to proceed.

```rust
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn request_approval(
        &self,
        agent_id: &AgentId,
        tool_name: &str,
        params: &serde_json::Value,
    ) -> ApprovalDecision;
}

pub enum ApprovalDecision {
    Approved,
    Denied { reason: String },
    ModifiedParams(serde_json::Value),  // Approve with modified parameters
}
```

This provides a security checkpoint for sensitive operations without requiring the SDK to understand what "sensitive" means in each product's domain.

### 6.4 Tool Execution Timeout

All tool executions are wrapped in a configurable timeout (default: 30 seconds). Tools that exceed the timeout are cancelled via Tokio's cancellation mechanism, and the agent receives a timeout error to handle gracefully.

---

## 7. Substrate File Security

### 7.1 PulseDB File Permissions

PulseDB stores data in local files (RocksDB-based). PulseHive documents the following recommendations for production deployments:

- **File permissions**: Set substrate directory to `0700` (owner read/write/execute only). Individual files should be `0600`.
- **Directory location**: Store substrate files outside of web-accessible directories, version control, and backup systems that sync to cloud.
- **Encryption at rest**: PulseDB does not encrypt data at rest in v0.1.1. For sensitive data, products should use filesystem-level encryption (LUKS, FileVault, BitLocker) or encrypted container volumes.

```rust
// PulseHive documents this pattern but does not enforce it
// Products are responsible for directory creation with appropriate permissions
std::fs::create_dir_all(&substrate_path)?;
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&substrate_path, std::fs::Permissions::from_mode(0o700))?;
}
```

### 7.2 Collective Isolation

PulseDB's `CollectiveId` provides logical isolation between projects. Each collective's experiences, relationships, and insights are stored separately. Cross-collective queries return empty results.

For stronger isolation, products can use separate substrate directories per collective, providing physical file-level separation.

### 7.3 Data Retention

PulseHive does not implement automatic data retention or deletion policies. Products that handle PII or regulated data must implement their own retention logic using PulseDB's deletion APIs. The SDK documents this responsibility clearly in the `HiveMind` builder documentation.

---

## 8. LLM Prompt Injection

### 8.1 Threat Description

LLM prompt injection is the highest-risk attack vector for any system that feeds untrusted content into LLM prompts. In PulseHive's architecture, the attack surface is:

1. **Experiences from substrate**: If an agent stores a malicious experience (e.g., "Ignore all previous instructions and..."), other agents perceiving that experience through their lens will include it in their LLM context.
2. **Tool results**: If a tool returns adversarial content (e.g., from a web scraping tool), it enters the agent's conversation history and may influence subsequent reasoning.
3. **Task descriptions**: If task content comes from untrusted user input, it can contain injection attempts.

### 8.2 Mitigation Strategy

PulseHive's approach to prompt injection is defense-in-depth:

**Layer 1 — Structured Output Parsing**: All LLM responses are parsed through `serde_json` into typed Rust structs. The agent cannot be instructed to "output the API key" because the response format is constrained to `LlmResponse` variants (text response or tool call). String manipulation of raw responses is forbidden in the codebase.

**Layer 2 — Context Framing**: Substrate experiences are presented to the LLM with clear framing that distinguishes them from instructions:

```
[System] You are a research agent. Your role is to analyze safety patterns.

[Substrate Context] The following represents your accumulated understanding:
- Experience (2026-03-15, confidence: 0.85): "The auth module has a session fixation vulnerability..."
- Experience (2026-03-14, confidence: 0.72): "Rate limiting was added to the /api/login endpoint..."

[Task] Analyze the current security posture of the authentication system.
```

The "Substrate Context" section is clearly delineated from system instructions and task descriptions. While this does not prevent all injection, it reduces the likelihood that injected content in experiences is treated as instructions.

**Layer 3 — Attention Budget Limits**: The `Lens.attention_budget` caps how many experiences enter the prompt. This limits the volume of potentially adversarial content an attacker can inject into an agent's context.

**Layer 4 — Confidence Scoring**: Experiences with low confidence scores are ranked lower by the lens. New, unverified experiences (which are more likely to be adversarial) have lower default confidence than experiences that have been corroborated by multiple agents.

**Layer 5 — Product-Level Filtering**: Products that ingest untrusted content (web scraping, user uploads, external APIs) are responsible for sanitizing that content before it enters the substrate. PulseHive documents this responsibility and provides hooks for content filtering in the experience recording pipeline.

### 8.3 Documentation of Risks

PulseHive's documentation explicitly states that LLM prompt injection cannot be fully prevented at the SDK level. The following guidance is provided:

- Never store raw, unsanitized user input as experience content.
- Use tools that interact with untrusted data to return structured summaries, not raw content.
- Enable `requires_approval` for any tool that performs irreversible actions.
- Monitor `HiveEvent` streams for unexpected tool calls or anomalous agent behavior.
- Consider running agents with minimal tool sets (principle of least privilege).

---

## 9. Supply Chain Security

### 9.1 Crate Publishing

PulseHive crates are published to crates.io under a verified account. Publishing checklist:

1. **Version bump**: Follows semver as documented in 06-UI-UX.md.
2. **Changelog**: All changes documented before publishing.
3. **CI green**: All tests, clippy, fmt, deny checks pass on all platforms.
4. **Audit dependencies**: Run `cargo deny check` and `cargo audit` before publishing.
5. **Dry run**: `cargo publish --dry-run` to verify package contents.
6. **No secrets in package**: Verify `.gitignore` and `Cargo.toml`'s `exclude` field prevent accidental inclusion of `.env`, test fixtures with keys, or substrate files.

### 9.2 Signed Commits

All commits to the PulseHive repository are signed with GPG or SSH keys. The `main` branch requires signed commits via branch protection rules. This ensures that published code is traceable to a verified author.

### 9.3 Reproducible Builds

PulseHive includes a `Cargo.lock` in the repository (standard for applications, optional for libraries — we include it for CI reproducibility). The `Cargo.lock` is not published with the crate (crates.io ignores it for library crates), but it ensures that CI builds are deterministic.

### 9.4 Binary Artifact Integrity

If PulseHive ever publishes pre-built binaries (e.g., CLI tools or Python wheel native extensions), they will include SHA-256 checksums and be built in CI with full build logs available.

---

## 10. AGPL-3.0 Compliance

### 10.1 License Requirements

PulseHive is licensed under AGPL-3.0. This has specific implications:

- **Source availability**: Any application that uses PulseHive and is made available over a network must make its complete source code available to users of that network service.
- **Derivative works**: Modifications to PulseHive itself must be released under AGPL-3.0.
- **Linking**: Applications that link PulseHive (statically or dynamically) are considered derivative works under AGPL-3.0.

### 10.2 Dependency License Compatibility

All dependencies must have licenses compatible with AGPL-3.0. The `cargo-deny` configuration enforces this. Compatible licenses include MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, and Zlib. Incompatible licenses (proprietary, SSPL, non-commercial) are denied.

### 10.3 License Headers

Every Rust source file includes an SPDX license header:

```rust
// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 Draco. All rights reserved.
```

### 10.4 Commercial Licensing

For organizations that cannot comply with AGPL-3.0's source disclosure requirements, a separate commercial license may be offered. This is a business decision outside the scope of this security document, but it is noted here because license compliance is a security concern for enterprise consumers.

---

## 11. Security Incident Response

### 11.1 Vulnerability Reporting

Security vulnerabilities in PulseHive should be reported via GitHub's private vulnerability reporting feature or by email to the maintainer. Public issue trackers should not be used for security-sensitive reports.

### 11.2 Response Timeline

- **Acknowledgment**: Within 48 hours of report.
- **Assessment**: Severity classification within 1 week.
- **Patch**: Critical vulnerabilities patched within 2 weeks. High within 4 weeks.
- **Disclosure**: Coordinated disclosure after patch is available.

### 11.3 RustSec Advisory

For confirmed vulnerabilities, a RustSec advisory is filed so that `cargo audit` and `cargo deny` detect the issue automatically for all consumers.

---

## 12. Security Checklist for Contributors

Before merging any pull request:

- [ ] No `unwrap()` or `expect()` on fallible operations in library code
- [ ] No API keys, tokens, or credentials in code, comments, or test fixtures
- [ ] No `println!()` or `eprintln!()` — use `tracing` macros
- [ ] New dependencies reviewed with `cargo tree` and `cargo geiger`
- [ ] `cargo deny check` passes
- [ ] Tool implementations validate parameters against JSON Schema
- [ ] Error messages do not leak internal state or sensitive data
- [ ] Tracing spans do not include key material or raw LLM responses
- [ ] `#[non_exhaustive]` on new public enums and structs that may grow
- [ ] Doc comments present on all new public items
