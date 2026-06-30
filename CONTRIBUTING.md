# Contributing to PulseHive

Thank you for your interest in contributing to PulseHive! This guide covers development setup, quality standards, and the PR process.

For extended onboarding context (architecture deep-dive, design decisions), see [`docs/14-Team-Onboarding.md`](docs/14-Team-Onboarding.md).

## Development Setup

```bash
# Clone the repository
git clone https://github.com/pulseai-labs/PulseHive.git
cd pulsehive

# Build all crates
cargo build --workspace

# Run tests (excludes language binding crates — they need their own runtimes)
cargo test --workspace --exclude pulsehive-py --exclude pulsehive-js

# Generate documentation
cargo doc --no-deps --workspace --open
```

### Prerequisites

- **Rust**: stable toolchain (install via [rustup](https://rustup.rs/))
- **Python 3.9+** (for pulsehive-py development): `pip install maturin pytest pytest-asyncio`
- **Node.js 18+** (for pulsehive-js development): `npm install` in `pulsehive-js/`

## Code Quality

All of these checks run in CI and must pass before merge:

```bash
# Formatting (enforced)
cargo fmt --all --check

# Linting (zero warnings)
cargo clippy --all-targets --workspace -- -D warnings

# Supply chain audit
cargo deny check

# Documentation (zero warnings)
cargo doc --no-deps --workspace
```

## Testing

### Rust Tests

```bash
# Run all Rust tests
cargo test --workspace --exclude pulsehive-py --exclude pulsehive-js

# Run a specific crate's tests
cargo test -p pulsehive-core
cargo test -p pulsehive-runtime

# Run with output visible
cargo test --workspace --exclude pulsehive-py --exclude pulsehive-js -- --nocapture

# Faster parallel execution
cargo nextest run
```

### Python Binding Tests

```bash
cd pulsehive-py
python -m venv .venv && source .venv/bin/activate
pip install maturin pytest pytest-asyncio
maturin develop
pytest tests/ -v
```

### TypeScript Binding Tests

```bash
cd pulsehive-js
npm install
npm run build:debug
npm test
```

### Benchmarks

```bash
cargo bench -p pulsehive-runtime
```

## Running Live API Tests

Live tests call a real LLM API (GLM via OpenAI-compatible endpoint) and are skipped by default.

```bash
# 1. Copy .env.example to .env and fill in your API key
cp .env.example .env

# 2. Run all live tests
cargo test -- --ignored

# 3. Run specific live test suites
cargo test -p pulsehive-openai --test live_api_test -- --ignored
cargo test -p pulsehive-runtime --test live_integration_test -- --ignored
```

Live tests validate: single-turn chat, multi-turn tool calling, streaming, error handling, full agentic loop with tools, and multi-agent sequential workflows.

## Pull Request Process

1. **Branch naming**: `feature/short-description`, `fix/issue-name`, or `docs/topic`
2. **Keep PRs focused**: one feature or fix per PR
3. **Tests required**: add or update tests for any code changes
4. **CI must pass**: fmt, clippy, test, doc — all green
5. **Documentation**: update rustdoc for any public API changes

## Code Conventions

PulseHive follows the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/):

- **Error handling**: Use `thiserror` for error derivation. Never panic in library code — always return `Result`.
- **Async traits**: Use `async_trait` for async methods in trait objects.
- **Builder pattern**: Use for complex struct construction (see `HiveMindBuilder`).
- **Type safety**: Prefer enums over strings. No stringly-typed APIs.
- **Documentation**: All public items must have `///` doc comments. Doc examples should compile.
- **Maximum 5 core primitives**: HiveMind, Agent, Tool, Lens, Experience. No abstraction without demonstrated use case.

## Crate Structure

| Crate | Purpose |
|-------|---------|
| `pulsehive-core` | Traits and types (zero provider dependencies) |
| `pulsehive-runtime` | HiveMind orchestrator, agentic loop, workflows, intelligence |
| `pulsehive-openai` | OpenAI-compatible LLM provider |
| `pulsehive-anthropic` | Anthropic Claude provider |
| `pulsehive` | Meta-crate with feature flags |
| `pulsehive-py` | Python bindings (PyO3) |
| `pulsehive-js` | TypeScript/Node.js bindings (napi-rs) |

## License

PulseHive is licensed under [AGPL-3.0-only](LICENSE). By contributing, you agree that your contributions will be licensed under the same license.
