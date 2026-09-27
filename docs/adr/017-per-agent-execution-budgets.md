# ADR 017: Per-Agent Execution Budgets

**Status:** Accepted
**Category:** 9 - Cross-cutting constraints
**Touch Surface:** `pulsehive-core/src/llm.rs,pulsehive-core/src/agent.rs,pulsehive-runtime/src/agentic_loop.rs,pulsehive-runtime/src/workflow.rs`
**Revisit Trigger:** When a workflow-wide budget (PH-6) or typed failures (PH-7) land

## Context

pulse-guard's smoke pass on the published 3.0.0 found that a consumer cannot
bound one agent's work without bounding every agent's: `DEFAULT_MAX_ITERATIONS`
is a module constant applied to every LLM child by the workflow dispatch
(`pulsehive-runtime/src/workflow.rs` builds
`LoopContext { max_iterations: DEFAULT_MAX_ITERATIONS, … }`), and no tool-call
budget exists at all. The two probes are
`a5_max_iterations_is_not_per_agent_configurable` and
`a5_no_max_tool_calls_per_agent`. The failure mode is a real one for a
consumer running a cheap lens beside an expensive critic: the only lever the
runtime offered was the same iteration count for both, and nothing at all
bounded a model that loops on tool calls — a single response requesting calls
runs them all, and the turn only ends when a response requests none or the
iteration budget is spent.

Three existing contracts narrow where a budget can live.

**The exhaustively-matchable public structs cannot take a field.**
`LlmAgentConfig` and `AgentDefinition` (`pulsehive-core/src/agent.rs`) and
`LoopContext` (`pulsehive-runtime/src/agentic_loop.rs`) are public structs with
public fields, exhaustive, and have no constructors — every consumer builds
them by struct literal. Adding a field to any of them is a breaking change
(ADR-005), which is why the cancellation work put its carrier on `LlmConfig`
instead (ADR-011: `timeout_secs`, `max_retries`).

**`AgentOutcome` can take a variant.** It is `#[non_exhaustive]` since 3.0.0, so
exhaustive matches in consumer code already need a wildcard arm and a new
variant is an additive change — the same reasoning ADR-014 recorded for
`Cancelled`.

**The wire shape is frozen.** `LlmConfig` serializes; the 2.0.2 shape (d4,
`pulsehive-core/tests/llm_contract.rs`) must stay byte-identical, so any new
field is `Option` with `skip_serializing_if`.

At `409965a` (round 1's barrier) the loop is
`for iteration in 1..=ctx.max_iterations` with a per-response
`for tool_call in &response.tool_calls` whose first statement is ADR-014's A7
cancellation checkpoint, and `execute_tool_call` returns `Ok(ToolResult)` for a
missing tool and for every approval outcome alike — it does not say whether a
tool body ran. This ADR records the budget contract before the enforcement
lands, so the loop, the workflows and the changelog describe one semantics.

## Decision

**L3 — the budgets live on `LlmConfig`.** `max_iterations: Option<usize>` and
`max_tool_calls: Option<usize>`, each with
`#[serde(default, skip_serializing_if = "Option::is_none")]` (the
`timeout_secs`/`max_retries` pattern) and the builders `with_max_iterations` and
`with_max_tool_calls`. `LlmConfig::new` sets both to `None`, which keeps today's
behaviour exactly: `ctx.max_iterations` (25 from a workflow dispatch, or whatever
a direct `run_agentic_loop` caller passed) and no tool-call cap.

Rejected: a field on `LlmAgentConfig` or `AgentDefinition` — breaking, per the
struct-literal contract above, and the cap is loop policy rather than agent
identity; a field on `LoopContext` — equally breaking, and it would put the same
knob on two types with no rule for which wins; a new `AgentKind` or wrapper type
— a second way to express what an agent already is, for one integer pair.

**The cost is recorded here.** `LlmConfig` stops being purely provider-call
configuration: it now carries two values that every provider ignores. Providers
must not read them and never send them — every provider builds its own request
body from the fields it knows, and no provider crate is touched. Serde skips
each when unset, so an uncapped `LlmConfig` keeps the 2.0.2 shape; a set cap
does appear in `LlmConfig`'s own serialization. A consumer reading `LlmConfig` can no longer
assume every field describes the request on the wire; the two budget fields
describe the loop that drives the requests. Making the caps `Option` and
`None`-defaulted is what keeps that cost additive rather than breaking.

**L4 — a tripped tool-call cap returns `AgentOutcome::ToolCallCapReached { limit: usize }`.**
`limit` is the configured cap, carried on the outcome because the consumer
that set it is the one that has to explain the stop. Serde renders it
`{"status":"tool_call_cap_reached","limit":N}`, the `#[non_exhaustive]`
snake-case shape every other variant uses.

The iteration cap keeps the **unit** variant `MaxIterationsReached`: it is an
existing public variant, and adding a field to it would break every pattern that
matches it — the additive argument that justifies a new variant does not
justify changing an old one. The consequence is a deliberate asymmetry: an
iteration-capped turn does not tell the consumer *which* iteration bound it hit.

**Deferred, recorded: the iteration cap's limit is not carried on its outcome.**
A consumer that wants `MaxIterationsReached { limit }` for symmetry with
`ToolCallCapReached { limit }` has to wait for typed failures (PH-7), which
revisits `MaxIterationsReached`'s shape together with the rest of the outcome
taxonomy. Recording the gap here is the whole of the commitment this ADR makes
about it.

**L5 — enforcement, verbatim in substance.**

1. **The cap is checked before each tool invocation**, including the second and
   later calls in one response. When `executed == limit` and another call is
   requested, the loop returns `ToolCallCapReached` at once, and the unstarted
   calls get no `ToolCallStarted`.
2. **A call counts once it reaches the tool body.** A denied call or an unknown
   tool does not count.
3. **`Some(0)` is literal.** `max_tool_calls: Some(0)` means the first requested
   call trips the cap; `max_iterations: Some(0)` means `MaxIterationsReached`
   with no LLM call.
4. **Counters are per dispatch.** Every `Loop` re-dispatch and every workflow
   child starts at zero; there is no workflow-wide total (that is PH-6).
5. **Cancellation wins over a cap** (ADR-014 A7's checkpoint order — the
   cancellation checkpoint stays the first statement of the per-call loop, the
   cap check the second), and **a final answer with no tool calls ends
   `Complete` even at `executed == limit`** — the cap bounds work, not the
   ability to answer with what the work produced.

**Effective iteration cap.** `config.llm_config.max_iterations.unwrap_or(ctx.max_iterations)`
is the loop bound, so the override holds at every entry point — the workflow
dispatch keeps `max_iterations: DEFAULT_MAX_ITERATIONS` in its `LoopContext`
literal and needs no change. A `Some(0)` runs no iteration and falls through to
the existing tail: the ADR-014 cancellation checkpoint, then
`MaxIterationsReached`.

**The counter is local to one `run_agentic_loop` call.** `executed: usize` is
declared inside the loop, so a `Loop` iteration, a workflow child and a retry
each begin at zero. Making it a workflow-wide total is PH-6's budget, not this
one.

**Counting needs the dispatch to report it.** The private `execute_tool_call`
returns a private `ToolDispatch` — `Executed(ToolResult)` for the paths that
reach the body, `NotExecuted(ToolResult)` for a missing tool, a denied approval
and a failed approval handler — so every return path names whether it spent
budget. L5.2's rule is what makes `unknown_tool_does_not_count_against_the_cap`
expressible at all: a missing tool and a real one both produce a `ToolResult` to
push into the conversation, and only the second is work.

**Composites name the trip.** `Parallel`'s aggregation gains an explicit arm
naming the child, `"<agent>: tool call cap reached (limit N)"` — the same
position and shape as `"<agent>: max iterations reached"`, so a partly failed
Parallel keeps one error per failed child and stays attributable (ADR-014's
Parallel-cancel precedent). `Sequential` needs no arm: the variant is terminal
and w1's catch-all returns it unchanged (ADR-014's r2.s5 amendment, L2). In
`run_loop` the variant lands in the same arm as `MaxIterationsReached` — the
iteration's outcome is recorded and the loop continues to its own cap, exactly
as a child that exhausts its iterations does today, and no special case is
added. A cap is per dispatch (L5.4), so a `Loop` is precisely the thing that
legitimately re-dispatches a capped child with a fresh budget.

**The bindings are unchanged.** `pulsehive-py` and `pulsehive-js` map
`AgentOutcome` with a wildcard to `unknown` (ADR-014, "Bindings map outcomes"),
so `ToolCallCapReached` renders there as any other unknown status. The bindings
expose no `LlmConfig` cap knob, so a binding user cannot trip it — a deliberate
consequence of L3: the budget is available to the Rust consumer that composes the
workflow, not to the script that drives one.

## Consequences

- **Additive for every consumer.** No field is added to an exhaustive struct, no
  existing variant changes shape, and an `LlmConfig` with neither budget set
  serializes byte-identically to 2.0.2 (d4). An agent that sets neither cap
  behaves exactly as in 3.0.0.
- **Consumer code that matches `AgentOutcome` exhaustively already compiles**
  (`#[non_exhaustive]` since 3.0.0) but must handle the new status if it wants to
  explain the stop; a wildcard arm absorbs it as `unknown`.
- **`LlmConfig` is now two things at once** — the request's parameters and the
  loop's policy. The duplicate-ish name (`max_tokens` for the model,
  `max_iterations` for the loop) is the visible cost; the alternative was a
  breaking struct field, which costs more.
- **No workflow-wide budget.** Two agents in one workflow each get their own
  counters; a consumer that wants a total for the workflow does not have it
  (PH-6).
- **The iteration cap is silent about its limit** (PH-7, above).
- **A cap-ended turn records no experience.** The default extractor records a
  `Difficulty` for `MaxIterationsReached`, but `ToolCallCapReached` falls to its
  wildcard arm and extracts nothing, like `Cancelled` and `PartialComplete`. A
  custom `ExperienceExtractor` still receives the outcome and may record one.
