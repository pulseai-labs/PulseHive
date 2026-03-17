# PulseHive SDK — Testing Strategy

> **Document ID:** TEST-PH-008
> **Version:** 1.0
> **Date:** 2026-03-17
> **Author:** Draco (with Claude Code)
> **Status:** Active
> **Reference:** SPEC v0.4.0, PRD-PH-001

---

## 1. Introduction

PulseHive is a Rust SDK with async internals, trait-based extensibility, and a dependency on PulseDB for storage. Testing must cover unit logic in isolation, integration across crate boundaries, and the full agentic loop with mocked LLM providers. This document defines the testing strategy, tooling, coverage targets, and CI pipeline.

---

## 2. Testing Tools

| Tool | Purpose |
|------|---------|
| `cargo test` | Standard test runner for unit and integration tests |
| `cargo nextest` | Parallel test execution with better output and retry support |
| `tokio::test` | Async test harness for all async functions |
| `mockall` | Mock generation for `SubstrateProvider`, `LlmProvider`, `Tool` traits |
| `proptest` | Property-based testing for lens math, ranking algorithms, configuration validation |
| `cargo-tarpaulin` | Code coverage measurement |
| `criterion` | Benchmark framework for performance-critical paths |
| `assert_matches` | Ergonomic enum variant assertions |

---

## 3. Unit Tests

### 3.1 Structure

Unit tests live in `#[cfg(test)] mod tests` at the bottom of each source file. They test a single function or method in isolation, with all external dependencies mocked.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use mockall::predicate::*;

    #[test]
    fn lens_domain_filtering_excludes_unrelated_experiences() {
        let lens = Lens::new()
            .domain("safety")
            .attention_budget(10);

        let experiences = vec![
            make_experience("safety", "Error in auth module"),
            make_experience("performance", "Cache hit rate is 95%"),
            make_experience("safety", "XSS vulnerability found"),
        ];

        let filtered = lens.filter_by_domain(&experiences);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|e| e.domains.contains(&"safety".to_string())));
    }
}
```

### 3.2 Mocking Strategy

PulseHive's trait-based architecture makes mocking straightforward. The three primary mock targets are:

**SubstrateProvider mock**: Used in all unit tests that interact with the substrate. Returns scripted experiences, relationships, and insights without touching PulseDB.

```rust
use mockall::automock;

#[automock]
#[async_trait]
impl SubstrateProvider for MockSubstrateProvider {
    // mockall generates this from the trait definition
}

#[tokio::test]
async fn record_experience_triggers_relationship_detection() {
    let mut mock_substrate = MockSubstrateProvider::new();
    mock_substrate
        .expect_store_experience()
        .times(1)
        .returning(|_| Ok(ExperienceId::new()));
    mock_substrate
        .expect_search_similar()
        .returning(|_, _, _| Ok(vec![]));

    let hive = HiveMind::builder()
        .substrate(Box::new(mock_substrate))
        .build()
        .unwrap();

    let result = hive.record_experience(test_experience()).await;
    assert!(result.is_ok());
}
```

**LlmProvider mock**: Returns scripted LLM responses (text or tool calls) to test the agentic loop without making real API calls.

```rust
struct ScriptedLlmProvider {
    responses: Vec<LlmResponse>,
    call_index: AtomicUsize,
}

#[async_trait]
impl LlmProvider for ScriptedLlmProvider {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse> {
        let idx = self.call_index.fetch_add(1, Ordering::SeqCst);
        Ok(self.responses[idx].clone())
    }
}
```

**Tool mock**: Verifies that tools receive correct parameters and that their results are properly integrated into the agent's conversation history.

### 3.3 Property-Based Tests

Lens math and ranking algorithms are tested with `proptest` to catch edge cases that hand-written tests miss:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn recency_score_is_always_between_zero_and_one(
        half_life in 1.0f32..10000.0,
        age_hours in 0.0f32..100000.0,
    ) {
        let curve = RecencyCurve::Exponential { half_life_hours: half_life };
        let score = curve.compute_score(age_hours);
        prop_assert!(score >= 0.0);
        prop_assert!(score <= 1.0);
    }

    #[test]
    fn attention_budget_is_respected(
        budget in 1usize..100,
        experience_count in 0usize..500,
    ) {
        let lens = Lens::new().attention_budget(budget);
        let experiences = generate_experiences(experience_count);
        let perceived = lens.apply(&experiences);
        prop_assert!(perceived.len() <= budget);
    }

    #[test]
    fn attractor_strength_is_non_negative(
        importance in 0.0f32..1.0,
        confidence in 0.0f32..1.0,
        applications in 0u32..1000,
    ) {
        let strength = compute_attractor_strength(importance, confidence, applications);
        prop_assert!(strength >= 0.0);
    }
}
```

---

## 4. Integration Tests

### 4.1 Structure

Integration tests live in the `tests/` directory at the workspace root. They test cross-crate interactions with a real PulseDB substrate (using temporary directories) and a mock LLM provider.

```
tests/
├── agentic_loop.rs         # Full perceive-think-act-record cycle
├── multi_agent.rs          # Two+ agents sharing substrate via Watch
├── workflow_agents.rs      # Sequential, Parallel, Loop execution
├── intelligence.rs         # RelationshipDetector + InsightSynthesizer pipeline
├── context_assembly.rs     # Lens perception + PulseDB search + re-ranking
├── event_stream.rs         # HiveEvent stream completeness and ordering
├── provider_openai.rs      # OpenAI-compatible provider with mock HTTP server
└── error_handling.rs       # Graceful degradation on tool/LLM/substrate failures
```

### 4.2 Test Substrate

Integration tests use PulseDB with temporary directories that are cleaned up automatically:

```rust
use tempfile::TempDir;

fn create_test_substrate() -> (Box<dyn SubstrateProvider>, TempDir) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let substrate = PulseDb::open(dir.path()).expect("failed to open test substrate");
    (Box::new(substrate), dir)  // TempDir dropped = directory deleted
}
```

### 4.3 Seed Functions

Test fixtures provide pre-populated substrates for tests that need existing data:

```rust
async fn seed_safety_experiences(substrate: &dyn SubstrateProvider, count: usize) -> Vec<ExperienceId> {
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let exp = NewExperience {
            content: format!("Safety finding #{i}: authentication bypass in module {i}"),
            experience_type: ExperienceType::ErrorPattern {
                signature: format!("AUTH_BYPASS_{i}"),
                fix: "Add session validation".into(),
                prevention: "Enable CSRF tokens".into(),
            },
            domains: vec!["safety".into()],
            importance: 0.8,
            confidence: 0.9,
            ..Default::default()
        };
        ids.push(substrate.store_experience(exp).await.unwrap());
    }
    ids
}
```

### 4.4 Mock LLM Provider for Integration Tests

A `ScriptedLlmProvider` returns pre-defined responses in sequence, allowing deterministic testing of the full agentic loop:

```rust
/// Creates a provider that simulates: think -> tool call -> think -> final response
fn create_tool_use_provider() -> ScriptedLlmProvider {
    ScriptedLlmProvider::new(vec![
        // First call: agent decides to use a tool
        LlmResponse::ToolCall {
            tool_name: "search".into(),
            parameters: json!({"query": "auth vulnerabilities"}),
        },
        // Second call: agent produces final response after seeing tool result
        LlmResponse::Text("Found 3 authentication vulnerabilities requiring immediate attention.".into()),
    ])
}
```

---

## 5. Critical Test Paths

These scenarios must have 100% test coverage. Any regression in these paths blocks a release.

### 5.1 Full Agentic Loop

Test the complete perceive-think-act-record cycle for a single LLM agent:

1. Seed substrate with experiences.
2. Deploy agent with lens, tools, and scripted LLM provider.
3. Verify agent perceives relevant experiences (lens filtering works).
4. Verify agent calls tools with correct parameters.
5. Verify agent produces a final response.
6. Verify experiences are recorded to substrate after completion.
7. Verify all expected `HiveEvent` variants are emitted in correct order.

### 5.2 Multi-Agent Substrate Sharing

Test that agents sharing a substrate perceive each other's contributions:

1. Deploy Agent A and Agent B on the same substrate.
2. Agent A records an experience during its task.
3. Verify Agent B perceives Agent A's experience in its next substrate read.
4. Test with PulseDB's Watch system: verify real-time propagation without polling.

### 5.3 Intelligence Pipeline

Test the experience-to-relationship-to-insight pipeline:

1. Record experience E1.
2. Verify RelationshipDetector runs and stores relationships.
3. Record experience E2 that is semantically related to E1.
4. Verify a relationship (Supports/Contradicts/Elaborates) is created between E1 and E2.
5. Record enough related experiences to trigger InsightSynthesizer.
6. Verify an insight is generated and stored in the substrate.

### 5.4 Workflow Agent Execution Order

Test all three workflow agent types:

**Sequential**: Verify agents execute in order and each sees previous agents' experiences.

**Parallel**: Verify all agents start concurrently and all complete before the parent workflow completes. Verify substrate sharing works across parallel agents.

**Loop**: Verify the agent re-executes up to `max_iterations`. Verify early termination when the agent signals completion. Verify `MaxIterationsExceeded` error when limit is hit without completion.

### 5.5 Error Recovery

Test graceful handling of failures at every layer:

- LLM provider returns error mid-conversation: agent emits error event and terminates cleanly.
- Tool execution fails: error is fed back to LLM for self-correction.
- Substrate write fails: experience recording failure does not crash the agent.
- Approval denied: tool call is skipped, agent continues with denial context.
- Timeout exceeded: agent and tool execution are cancelled cleanly.

---

## 6. Async Testing

### 6.1 Tokio Test Harness

All async tests use `#[tokio::test]`:

```rust
#[tokio::test]
async fn deploy_returns_stream_of_events() {
    let (substrate, _dir) = create_test_substrate();
    let hive = HiveMind::builder()
        .substrate(substrate)
        .llm_provider("mock", create_simple_provider())
        .build()
        .unwrap();

    let agent = AgentDefinition::llm("test", "You are a test agent.", "mock");
    let mut stream = hive.deploy(vec![agent], vec![Task::new("Say hello")]).await.unwrap();

    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }

    assert!(events.iter().any(|e| matches!(e, HiveEvent::AgentStarted { .. })));
    assert!(events.iter().any(|e| matches!(e, HiveEvent::AgentCompleted { .. })));
}
```

### 6.2 Timeout Handling

Tests that involve async operations use `tokio::time::timeout` to prevent hangs in CI:

```rust
#[tokio::test]
async fn agent_does_not_hang_on_empty_substrate() {
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_single_agent_test(),
    ).await;

    assert!(result.is_ok(), "test timed out — possible deadlock");
    assert!(result.unwrap().is_ok());
}
```

### 6.3 Testing Stream Consumers

Tests verify that `Stream<Item = HiveEvent>` is consumed correctly:

```rust
#[tokio::test]
async fn event_stream_completes_when_all_agents_finish() {
    let mut stream = deploy_test_agents().await.unwrap();
    let events: Vec<HiveEvent> = stream.collect().await;

    // Stream must terminate (not hang)
    assert!(!events.is_empty());

    // Last event should be deployment completed
    assert!(matches!(events.last(), Some(HiveEvent::DeploymentCompleted { .. })));
}
```

---

## 7. Performance Tests

### 7.1 Criterion Benchmarks

Performance-critical paths are benchmarked with `criterion` in the `benches/` directory:

```
benches/
├── substrate_search.rs     # search_similar() latency at various experience counts
├── context_assembly.rs     # Full lens perception pipeline latency
├── lens_ranking.rs         # Post-search re-ranking with temporal decay
└── agent_deployment.rs     # Overhead of deploying agents (excluding LLM calls)
```

### 7.2 Performance Targets

From PRD-PH-001:

| Operation | Target | Measurement |
|-----------|--------|-------------|
| Substrate search (1K experiences, k=20) | < 1ms | `search_similar()` benchmark |
| Context assembly (1K experiences) | < 10ms | Full lens perception + ranking |
| Experience recording | < 15ms | Store + relationship inference (no LLM) |
| Agent deployment overhead | < 5ms | Time from `deploy()` to first agent starting |

### 7.3 Regression Detection

Criterion benchmarks run in CI on every pull request. Performance regressions exceeding 10% on any target metric block the merge. Criterion's statistical comparison handles noise — only statistically significant regressions are flagged.

---

## 8. Coverage Targets

| Crate | Target | Rationale |
|-------|--------|-----------|
| `pulsehive-core` | 80%+ | Trait definitions, types, lens math |
| `pulsehive-runtime` | 80%+ | HiveMind, agentic loop, workflow agents, intelligence |
| `pulsehive-anthropic` | 60%+ | HTTP client code is harder to unit test; integration tests cover it |
| `pulsehive-openai` | 60%+ | Same as Anthropic — provider-specific integration testing |
| Critical paths (Section 5) | 100% | Agentic loop, multi-agent sharing, intelligence pipeline, workflow execution |

Coverage is measured with `cargo-tarpaulin` and reported in CI. Coverage decreases on critical paths block the merge.

```bash
# Local coverage measurement
cargo tarpaulin --workspace --out html --skip-clean

# CI coverage (report to codecov or similar)
cargo tarpaulin --workspace --out xml --skip-clean
```

---

## 9. CI Pipeline

### 9.1 GitHub Actions Matrix

```yaml
strategy:
  matrix:
    os: [ubuntu-latest, macos-latest, windows-latest]
    rust: [stable, beta]
```

### 9.2 Pipeline Stages

Every pull request runs the following stages in order:

**Stage 1 — Formatting and Linting**
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

**Stage 2 — Build**
```bash
cargo build --workspace --all-targets
```

**Stage 3 — Unit and Integration Tests**
```bash
cargo nextest run --workspace
cargo test --doc --workspace   # Doc example tests
```

**Stage 4 — Security Checks**
```bash
cargo deny check advisories licenses bans sources
cargo audit
```

**Stage 5 — Documentation**
```bash
cargo doc --workspace --no-deps
```
Documentation must build without warnings. Missing doc comments on public items produce warnings that fail this stage.

**Stage 6 — Coverage (main branch only)**
```bash
cargo tarpaulin --workspace --out xml --skip-clean
```

**Stage 7 — Benchmarks (main branch only)**
```bash
cargo bench --workspace
```
Benchmark results are stored as CI artifacts for historical comparison.

### 9.3 Pre-Release Checks

Before publishing a new version, the following additional checks run:

```bash
cargo publish --dry-run -p pulsehive-core
cargo publish --dry-run -p pulsehive-runtime
cargo publish --dry-run -p pulsehive-anthropic
cargo publish --dry-run -p pulsehive-openai
cargo publish --dry-run -p pulsehive
```

---

## 10. Testing with Real LLM Providers

### 10.1 When to Test Against Real APIs

Real LLM provider testing happens in two situations:

1. **Pre-release manual testing**: Before publishing a new version, run the integration test suite against live Claude and at least one OpenAI-compatible endpoint (GLM or Ollama) to verify protocol compatibility.
2. **Provider implementation changes**: When modifying `pulsehive-anthropic` or `pulsehive-openai` crate internals, run targeted tests against the real API.

Real API tests are never run in automated CI. They are gated behind environment variables and a `--ignored` test flag:

```rust
#[tokio::test]
#[ignore]  // Only run manually: cargo test -- --ignored
async fn live_anthropic_provider_completes_request() {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY must be set for live tests");

    let provider = AnthropicProvider::new(&api_key, "claude-sonnet-4-20250514");
    let response = provider.complete(LlmRequest {
        system: "You are a test assistant.".into(),
        messages: vec![UserMessage("Say hello.".into())],
        tools: vec![],
        max_tokens: 100,
    }).await;

    assert!(response.is_ok());
    match response.unwrap() {
        LlmResponse::Text(text) => assert!(!text.is_empty()),
        _ => panic!("expected text response"),
    }
}
```

### 10.2 Live Test Checklist

Before release, manually verify:

- [ ] Anthropic Claude: single-turn completion, multi-turn with tool use, streaming
- [ ] OpenAI-compatible (Ollama local): single-turn, tool use
- [ ] Error handling: invalid API key returns typed error (not panic)
- [ ] Error handling: rate limit returns typed error with retry-after info
- [ ] Timeout: provider respects configured timeout

### 10.3 Mock HTTP Server for CI

For CI, provider integration tests use a local mock HTTP server (via `wiremock` or `httpmock`) that returns scripted responses matching the Anthropic/OpenAI response format:

```rust
#[tokio::test]
async fn anthropic_provider_parses_tool_use_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{
                "type": "tool_use",
                "id": "call_123",
                "name": "search",
                "input": {"query": "test"}
            }],
            "stop_reason": "tool_use"
        })))
        .mount(&mock_server)
        .await;

    let provider = AnthropicProvider::new("test-key", "claude-sonnet-4-20250514")
        .with_base_url(&mock_server.uri());

    let response = provider.complete(test_request()).await.unwrap();
    assert!(matches!(response, LlmResponse::ToolCall { .. }));
}
```

---

## 11. Test Naming Conventions

Tests follow a consistent naming pattern: `<unit>_<condition>_<expected_outcome>`.

```rust
// Good: clear what is tested, under what condition, and what is expected
fn lens_with_empty_domain_list_returns_all_experiences()
fn recency_score_with_zero_age_returns_one()
fn agentic_loop_with_tool_failure_feeds_error_to_llm()
fn sequential_workflow_with_three_agents_executes_in_order()

// Bad: vague or missing condition/outcome
fn test_lens()
fn it_works()
fn workflow_test_1()
```

---

## 12. Test Organization Summary

```
pulsehive/
├── pulsehive-core/
│   └── src/
│       ├── lens.rs          # #[cfg(test)] mod tests — lens math, filtering
│       ├── event.rs         # #[cfg(test)] mod tests — event serialization
│       └── ...
├── pulsehive-runtime/
│   └── src/
│       ├── hivemind.rs      # #[cfg(test)] mod tests — builder validation, config
│       ├── agentic_loop.rs  # #[cfg(test)] mod tests — loop logic with mocks
│       ├── intelligence.rs  # #[cfg(test)] mod tests — relationship/insight algorithms
│       └── ...
├── pulsehive-anthropic/
│   └── src/
│       └── lib.rs           # #[cfg(test)] mod tests — response parsing, error mapping
├── pulsehive-openai/
│   └── src/
│       └── lib.rs           # #[cfg(test)] mod tests — response parsing, error mapping
├── tests/                   # Integration tests (cross-crate, real PulseDB)
│   ├── agentic_loop.rs
│   ├── multi_agent.rs
│   ├── workflow_agents.rs
│   ├── intelligence.rs
│   ├── context_assembly.rs
│   ├── event_stream.rs
│   ├── provider_openai.rs
│   └── error_handling.rs
├── benches/                 # Criterion benchmarks
│   ├── substrate_search.rs
│   ├── context_assembly.rs
│   ├── lens_ranking.rs
│   └── agent_deployment.rs
└── test_utils/              # Shared test helpers (optional internal crate)
    └── src/
        └── lib.rs           # create_test_substrate(), seed functions, ScriptedLlmProvider
```
