# PulseHive — Roadmap

> Derived from MASTER-SPEC.md by `/plan-roadmap` on 2026-05-30.
> Co-edited by user + scaffold-dev orchestrator over time.

## Roadmap overview

PulseHive's near-term roadmap is organized around one **Phase** — *v2.x SDK Expansion*, the visionary horizon for the additive v2.x line before the v3.0 breaking work. Its first **Sprint** (the value-building window) is **v2.1.0**, which ships the provider and tooling capabilities downstream products have asked for: streaming tool execution, cooperative cancellation, and subscription-billed subprocess providers (Claude Code + Codex).

Sprint 1.1 is decomposed to full **Vertical Slice** depth — four slices in strict dependency order (streaming → cancellation → stateless subprocess → stateful subprocess), each carrying 2–3 `auto:`/`user:` demo criteria. This is the build-ready surface scaffold-dev's orchestrator consumes. Slice 1 lands first because it absorbs the single `#[non_exhaustive]` contract change the others rely on; slice 2 depends on it; slices 3–4 build the new `pulsehive-subprocess` crate (stateless then stateful), gated behind a short de-risking spike on the open CLI questions. **All four slices ship as a single v2.1.0 release rather than split across two** — they are one coherent enhancement bundle downstream products requested together, and releasing them piecemeal would fragment downstream integration. Traceability arrays are empty for now (lightweight mode — no `/scaffold-docs` SRS/BACKLOG yet); they can be backfilled via `/plan-roadmap --refine-slice` once governance docs exist.

Future enhancement batches (v2.2, v2.3, …) become **Sprint 1.2, 1.3, …** authored via `/plan-roadmap --add-sprint 1` as each approaches — keeping demo criteria grounded in real implementation context rather than speculation. The eventual v3.0 breaking work (e.g. the `TokenUsage` → tagged `Usage` enum) will open as a new Phase when its time comes.

> **Note:** the title and this overview are manual additions; `/plan-roadmap` re-renders regenerate the slice hierarchy from state but reset these two fields — re-apply them after any re-render.

## Phase 1: v2.x SDK Expansion — near-term (v2.1.0 -> v2.x, before v3.0 breaking work)

The additive provider + tooling capabilities downstream products need, shipped as a MINOR release with a single documented contract change: VS-1.1.1 marks HiveEvent #[non_exhaustive], so downstream exhaustive matches must add a `_ => {}` arm (recorded in the v2.1.0 CHANGELOG). Everything else is purely additive to v2.0.x. At phase end, subscription-billed subprocess backends and streaming/cancellable tools are first-class in the SDK.

### Sprint 1.1: v2.1.0: Streaming, Cancellation & Subprocess Providers

Ship crates.io v2.1.0 (additive, bar the one HiveEvent #[non_exhaustive] change). Before committing the subprocess slices, a short de-risking spike resolves the open CLI questions (claude --output-format json schema stability; Codex session-resume support; concurrent subprocesses under one auth); if Codex cannot resume, VS-1.1.4's Codex path degrades to NotSupported rather than blocking the sprint. Demoable at close: a Claude-Code-backed agent runs a streaming, cancellable tool end-to-end with subscription-usage reporting.

#### VS-1.1.1: Streaming tool execution + event surface

StreamingTool extension trait, ToolProgress enum, HiveEvent::ToolProgress variant; bundles the one #[non_exhaustive] migration + lock-step pulsehive-py/js binding updates. Default Tool emits Started->Completed. Depends on: none — lands first, carrying the one-time #[non_exhaustive] migration the later slices rely on.

##### Traceability

- FR: None
- NFR: None
- Backlog: None

##### Demo criteria

- [ ] auto: cargo run -p pulsehive-runtime --example streaming_tool → expected: emits at least 5 ToolProgress events in order, ending with Completed
- [ ] user: subscribe to the HiveEvent stream during a long tool call → expected: live progress renders instead of a frozen wait

#### VS-1.1.2: Cancellation infrastructure

CancellableTool extension trait + tokio-util CancellationToken plumbed through the agent loop and workflow agents; HiveMind::abort_handle() for top-level cancel; AgentOutcome::Cancelled. Depends on: VS-1.1.1 (shares the HiveEvent #[non_exhaustive] shield).

##### Traceability

- FR: None
- NFR: None
- Backlog: None

##### Demo criteria

- [ ] auto: cargo run -p pulsehive-runtime --example cancellable_tool → expected: cancel at ~50% returns partial state and no ToolCallStarted fires after the cancel
- [ ] user: call HiveMind::abort_handle().cancel() mid-sweep → expected: the current item finishes, no new items start, partial results returned

#### VS-1.1.3: Stateless subprocess providers (Shape A)

New pulsehive-subprocess crate (feature-flagged): SubprocessRunner primitive, stateless ClaudeCodeProvider + CodexProvider, reusable tool adapter, additive SubscriptionUsage + LlmCallCompletedDetailed event. Depends on: none at the code level (implements LlmProvider, not Tool), but gated behind the open-question spike noted in the sprint goal.

##### Traceability

- FR: None
- NFR: None
- Backlog: None

##### Demo criteria

- [ ] auto: cargo test -p pulsehive-subprocess → expected: exit 0 (tool-adapter round-trip property test and mock-binary lifecycle tests pass)
- [ ] auto: PULSEHIVE_CLAUDE_CODE_TEST=1 cargo test --features subprocess-integration → expected: real claude round-trip returns a non-empty completion with SubscriptionUsage set (auth-gated — skipped in CI without subscription credentials; the mock-binary lifecycle test above is the always-run gate)
- [ ] user: register ClaudeCodeProvider::stateless and run one agent turn → expected: completion returned and subscription usage logged

#### VS-1.1.4: Stateful subprocess providers (Shape B)

StatefulLlmProvider extension trait, opaque SessionHandle, session-backed Claude Code + Codex providers with subprocess supervision; agent-loop opt-in via as_stateful(); HiveMind::shutdown(); 20-turn benchmark. Depends on: VS-1.1.3 (reuses the SubprocessRunner primitive and tool adapter).

##### Traceability

- FR: None
- NFR: None
- Backlog: None

##### Demo criteria

- [ ] auto: cargo test -p pulsehive-subprocess stateful → expected: session round-trip (open then 3 turns sharing history then close) passes and no orphan subprocess survives shutdown
- [ ] auto: cargo bench --bench stateful_vs_stateless → expected: Shape B is at least 2x faster than Shape A on session-internal turns (auth-gated — needs real subscription auth; the mock stateful test above is the always-run gate)
- [ ] user: run a 20-turn coaching session via ClaudeCodeProvider::session() → expected: noticeably faster than stateless with history preserved across turns

