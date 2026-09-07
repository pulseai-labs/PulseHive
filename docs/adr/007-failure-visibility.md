# ADR 007: Failure Visibility

**Status:** Accepted
**Category:** 6 - Failure visibility
**Touch Surface:** `pulsehive-*/src/`
**Revisit Trigger:** When introducing panic paths or silent error swallowing

## Context

PulseHive operates in complex distributed systems (LLM providers, substrate) where failures must be visible and actionable.

## Decision

**Failure Visibility:**
- **Result types:** Public APIs return `Result<T, E>` for fallible operations
- **No silent failures, with documented best-effort exceptions:** Most failures surface through `Result`, but known paths are best-effort today: `HiveMind::deploy` only logs a failed Watch subscription, and `record_experience` logs and continues after an embedding failure and silently ignores `get_experience` errors (`pulsehive-runtime/src/hivemind.rs`); error propagation for these paths is tracked in #57
- **Documented error conditions:** All error variants documented in API docs
- **Provider failures:** LLM provider errors propagate to consumer
- **Substrate failures:** PulseDB errors (connection, query) surface through Result types, excepting the best-effort paths above

**What Must Never Fail Silently (intent; the best-effort exceptions above are the known gaps):**
- API key authentication failures
- Substrate connection failures
- LLM provider outages
- Tool execution failures
- Experience corruption

**Rationale:** In agentic systems, silent failures cause incorrect agent behavior. Explicit error handling enables consumers to implement appropriate retry/fallback logic.

## Consequences

**Positive:**
- Consumers can implement targeted error handling
- Failures are debuggable and observable
- Best-effort exception paths are documented rather than hidden (propagation tracked in #57)

**Neutral:**
- Error handling boilerplate required in consumer code
- Some operations become fallible that could be infallible in theory

**Negative:**
- Deploy-time Watch subscription and record_experience embedding/get_experience failures are currently log-only or silent (propagation tracked in #57)
- Error propagation requires careful consumer design
