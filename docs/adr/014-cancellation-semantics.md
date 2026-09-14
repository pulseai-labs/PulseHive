# ADR 014: Cancellation Semantics

**Status:** Accepted
**Category:** 9 - Cross-cutting constraints
**Touch Surface:** `pulsehive-core/src/agent.rs,pulsehive-core/src/tool.rs,pulsehive-runtime/src/agentic_loop.rs,pulsehive-runtime/src/workflow.rs,pulsehive-runtime/src/hivemind.rs,pulsehive-py/src/,pulsehive-js/src/,docs/adr/005-public-contracts.md,docs/adr/010-cross-cutting.md`
**Revisit Trigger:** When cancellation must preempt a running tool body or child agent, when a binding-level abort handle is proposed, or when the Release 1 version is chosen at close

## Context

Release 1 exit criterion 3 requires a consumer to cancel a running agent
turn: the in-flight provider request aborts, no further tool call starts,
and the turn returns `AgentOutcome::Cancelled` with partial results — end to
end across HiveMind, the agent loop, workflows, and the bindings. r1.s1
(ADR-011) already gave providers a cooperative cancellation channel:
`LlmConfig.cancel: Option<CancellationToken>` and
`LlmErrorKind::Cancelled`, honored by both provider crates. What did not
exist is the *ownership and observation contract* above the provider: who
creates the token, how it scopes to a task, which components observe it, and
what outcome a cancelled turn produces. This ADR locks that contract before
any behavior lands, so the loop, workflow, and HiveMind work items that
follow implement one agreed semantics rather than three divergent ones.

## Decision

**Cancellation is cooperative.** Cancellation is a signal observed at safe
checkpoints. PulseHive never force-aborts a tool body or a child agent
task — a tool in mid-write and a mid-flight substrate operation always run
to their own conclusion (ADR-004's data-ownership boundary is preserved).

**The caller owns the token, scoped to a task.** `Task::with_cancel` takes a
`tokio_util::sync::CancellationToken` and `Task::cancel_token()` reads it
back. Each agent run spawned for that task receives a child token linked to
an internal HiveMind root, so `HiveMind::shutdown()` and `Drop` cancel every
run. `deploy()`'s signature and return type do not change. Rejected:
`HiveMind::abort_handle()` returning a root token — tokio-util tokens are
one-shot, so after one stop every later deploy would start already
cancelled; and a cancellation handle returned from `deploy()` — it would
break every runtime consumer's call site for a signal that belongs on the
task.

**Observation points are fixed.** The agent loop checks the run token before
each LLM call and before each tool call. Each `chat()` call's
`LlmConfig.cancel` is a child of the run token (ADR-011); a token a caller
already set on an agent definition's `LlmConfig.cancel` is still honored,
and either token cancels the call. A provider `LlmTransport` error with kind
`Cancelled` ends the turn as `AgentOutcome::Cancelled`. Tools observe
`ToolContext.cancel`, a child token handed to every invocation. Rejected: a
`CancellableTool` extension trait — every tool already receives
`&ToolContext` and the loop never preempts, so a second path to the same
token adds surface without behavior.

**Outcomes are additive and non-exhaustive.** `AgentOutcome` is
`#[non_exhaustive]` and gains `Cancelled { partial_response }` — the latest
assistant text before the cancel point, empty if none, with partial tool
output staying on the emitted `ToolCallCompleted` events — and
`PartialComplete { responses, errors }` for partial multi-agent results
(#45). A composite's `partial_response` mirrors what its `Complete` would
carry over the children completed so far. A nested `PartialComplete` merges
into its parent's `responses` and `errors`. A child that hit its iteration
cap contributes no response and is listed in `errors` as
`<agent>: max iterations reached`.

**Workflows observe the same token.** Sequential checks the token before
dispatching its next child; Loop checks at the top of each iteration;
Parallel cancels by cooperative drain and returns once every child has
returned. Sequential and Loop treat a child's `PartialComplete` as progress
and continue. Rejected: `JoinSet::abort_all` for Parallel — it cuts tool
bodies and substrate writes mid-operation, violating both the cooperative
guarantee above and ADR-004.

**No new event.** `AgentCompleted { outcome }` already carries the result;
cancellation introduces no new `HiveEvent` variant.

**Bindings map outcomes, not control.** The Python and JavaScript bindings
map the new outcomes to snake_case outcome strings with their fields
(`cancelled` with `partial_response`; `partial_complete` with `responses`
and `errors`), plus an `unknown` outcome for variants the binding does not
know. No binding abort handle ships in Release 1.

**Rationale:** A task-scoped, caller-owned token is the smallest surface
that lets a consumer cancel exactly the work it started while keeping
`deploy()`'s signature stable. Cooperative observation at fixed checkpoints
keeps ADR-004's storage-ownership guarantee intact — nothing is ever cut
mid-write — and making each `chat()` token a child of the run token reuses
ADR-011's transport contract rather than inventing a parallel mechanism.
`#[non_exhaustive]` turns every later outcome refinement into an additive
change under ADR-005.

## Consequences

**Positive:**

- A consumer cancels a running turn with one token it already owns; no new
  handle type reaches the public API.
- Every component observes the same token family — loop, provider call, tool
  invocation, and workflow child — so cancellation composes predictably
  through nested workflows.
- Partial work is never silently lost: `Cancelled.partial_response` carries
  the last assistant text and `PartialComplete` aggregates per-child
  responses and errors.
- The contract is recorded before behavior lands, so w2–w4 implement one
  agreed semantics.

**Neutral:**

- `AgentOutcome` becomes `#[non_exhaustive]`; downstream exhaustive matches
  in and out of the workspace must add a wildcard arm (recorded in ADR-005
  and the changelog).
- Cooperative cancellation means a misbehaving tool that never observes
  `ToolContext.cancel` finishes its body anyway; cancellation bounds the
  agent, not the tool's internals.

**Negative:**

- Token plumbing adds a field to `Task`, `WorkflowContext`, `LoopContext`,
  and `ToolContext` — `ToolContext`'s shape now differs by nothing but gains
  a mandatory field, so out-of-crate struct literals stop compiling
  (recorded as breaking).
- A caller that wants the *task* cancelled must keep the token; there is no
  query for "the token of a running deployment" — by design, since ownership
  stays with the caller.
