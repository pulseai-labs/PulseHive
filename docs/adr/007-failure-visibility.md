# ADR 007: Failure Visibility

**Status:** Accepted
**Category:** 6 - Failure visibility
**Touch Surface:** `pulsehive*/src/`
**Revisit Trigger:** When introducing panic paths or silent error swallowing

## Context

PulseHive operates in complex distributed systems (LLM providers, substrate) where failures must be visible and actionable.

## Decision

**Failure Visibility:**
- **Result types:** Public APIs return `Result<T, E>` for fallible operations
- **No silent failures, with documented best-effort exceptions:** Most failures surface through `Result`, but known paths are best-effort today — they log (or silently skip) and continue (`pulsehive-runtime/src/`); error propagation is tracked in #57:
  - `HiveMind::deploy` logs a failed Watch subscription and continues (`hivemind.rs`)
  - `record_experience` continues after an embedding failure, silently ignores `get_experience` errors, and logs-and-continues after `store_relation` and `store_insight` failures (`hivemind.rs`)
  - the event stream drops events with a warning when a subscriber lags (`hivemind.rs`)
  - the agentic loop continues with empty context when perception fails, stores experiences without embeddings after an embedding failure, and still returns the successful `AgentOutcome` when `store_experience` fails (`agentic_loop.rs`)
  - the streaming-tool progress forwarder is aborted with a warning after the drain grace period when a tool leaves its progress sender open (`agentic_loop.rs`)
  - relationship detection and insight synthesis log and continue when their substrate queries (`search_similar`, `get_related`) or insight LLM calls fail (`intelligence/`)
- **Known panic path:** the built-in provider constructors panic via `.expect` if reqwest client construction fails instead of returning `Result` (tracked in #61)
- **Documented error conditions:** All error variants documented in API docs
- **Provider failures:** LLM provider request errors propagate to consumer (constructor client-build failures panic; tracked in #61)
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
- The best-effort sites above are log-only or silent today (propagation tracked in #57), and provider constructors panic on client-build failure (Result return tracked in #61)
- Error propagation requires careful consumer design
