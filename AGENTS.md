## Code Review Rules

### What can block a merge

- Mark a finding as blocking only if the change rejects valid input, or corrupts or loses
  state, on a path it touches — or it breaks the test suite, or it opens a security hole
  (injection, an authentication bypass, an exposed secret). Say which of those it is.
  Safe path: everything else is a suggestion; mark it non-blocking.

### Edge cases the change does not handle

- An input or state the change does not handle is not a finding by itself. Raise it only
  when it meets the blocking rule above; otherwise mention it once, marked non-blocking.
  Safe path: a documented refusal of an unsupported input is correct behaviour.

### Workflow outcomes never erase child failures

- Flag a change that lets a `Sequential` or `Parallel` parent end `Complete` when any child
  contributed an error, or that folds an earlier child's accumulated errors into a terminal
  child outcome (`Error`, `MaxIterationsReached`, `ToolCallCapReached`), because it hides a
  degraded run from consumers — the defect pulse-guard PH-1 found.
  Safe path: a degraded `Sequential`/`Parallel` ends `PartialComplete` carrying its
  accumulated errors; terminal child outcomes are returned unchanged; a `Loop` reflects only
  its final iteration — the documented exception (docs/adr/014-cancellation-semantics.md,
  amendment of 2026-09-26, L1–L2 and A1).
