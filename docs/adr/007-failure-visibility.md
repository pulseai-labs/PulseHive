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
- **No silent failures:** Library never suppresses errors without explicit consumer acknowledgement
- **Documented error conditions:** All error variants documented in API docs
- **Provider failures:** LLM provider errors propagate to consumer
- **Substrate failures:** PulseDB errors (connection, query) surface through Result types

**What Must Never Fail Silently:**
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
- No hidden error suppression

**Neutral:**
- Error handling boilerplate required in consumer code
- Some operations become fallible that could be infallible in theory

**Negative:**
- Cannot suppress transient errors without consumer awareness
- Error propagation requires careful consumer design
