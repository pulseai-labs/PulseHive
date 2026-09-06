# ADR 011: Provider Transport Contract

**Status:** Proposed
**Category:** 10 - Provider transport boundary
**Touch Surface:** `pulsehive-core/src/llm.rs`, `pulsehive-core/src/error.rs`
**Revisit Trigger:** When a provider needs a transport signal that neither `LlmError` nor `LlmConfig` can carry

## Context

Every provider crate reaches the network, and until now the only way to report a
failure from that reach was `PulseHiveError::Llm(String)`. Callers that needed to
distinguish a timeout from a rate limit from a malformed tool call had to match on
message text, and downstream products did exactly that. A caller also had no way
to bound a single call: the request timeout and retry budget were provider-level
configuration, there was no way to cancel an in-flight call, and there was no way
to ask a reasoning model for more or less reasoning on one request. On the way
back, the provider's `finish_reason` and reasoning trace were discarded before the
response reached the caller.

Those gaps are all on one seam — the `LlmProvider` boundary — and closing them
means adding fields to `LlmConfig`, `LlmResponse` and the error type. On `main`
those types are plain public structs, so every added field is a literal break for
anyone constructing them. Round 2 of this spine (`pulsehive-openai`,
`pulsehive-anthropic`) needs the contract in place before it can implement
against it.

## Decision

**Structured transport errors.** `pulsehive_core::llm::LlmError` carries `kind`,
`message`, `status`, `attempts`, `body`, `finish_reason` and `retry_after`, and
`LlmErrorKind` names the classes a caller branches on: `Timeout`, `Connect`,
`RateLimited`, `ServerError`, `ClientError`, `Parse`, `MalformedToolCall`,
`Cancelled`. `PulseHiveError::LlmTransport(LlmError)` carries it, with a `#[from]`
conversion so `?` still propagates. `PulseHiveError::Llm(String)` is untouched and
keeps its job: request-build and serialization failures that never reached the
wire. `body` is stored verbatim and redaction is the consumer's call, because core
cannot know which provider's body holds a secret. No retry policy lives in core —
whether a kind is worth retrying is provider policy.

**Cancellation is a field on `LlmConfig`, not a parameter or a trait method.**
`LlmConfig::cancel` holds an `Option<tokio_util::sync::CancellationToken>` and is
`#[serde(skip)]` in both directions. A `chat()` parameter would change the
`LlmProvider` signature, and a trait method would put lifecycle control on a trait
that must stay object-safe and small. A config field reaches every provider call
through the argument that is already there, and it clones with the config, so a
runtime holding one token can hand copies to concurrent calls and cancel them
together. It is runtime state, never wire state, which is why it does not
serialize.

**Per-call transport and reasoning controls.** `timeout_secs` and `max_retries`
are `Option` on `LlmConfig`: `Some` means "override the provider's configured
value for this call", `None` means "use the provider's". `reasoning_effort`
(`ReasoningEffort`) and `tool_choice` (`ToolChoice`) are the request-side reasoning
and tool controls; `finish_reason` and `reasoning` on `LlmResponse` are the
response-side ones. Core defines the vocabulary and the field; mapping either onto
a specific provider's wire object belongs to the provider crate, which owns the
wire.

**`#[non_exhaustive]` plus constructors, once.** `LlmConfig`, `LlmResponse`,
`LlmError`, `LlmErrorKind`, `ReasoningEffort`, `ToolChoice` and `PulseHiveError`
are all `#[non_exhaustive]`. Construction moves to `LlmConfig::new`,
`LlmResponse::new` / `LlmResponse::text` and `LlmError::new` plus `with_*`
builders; fields stay `pub`, so reading and assigning them is unchanged. This is
one deliberate literal break taken now so that no later field on this contract
needs another one.

**Rationale:** The transport boundary is where PulseHive meets something it does
not control, so it is exactly where failures need to be typed and calls need to be
bounded. Taking the break once, at the moment the contract is being defined, buys
additive evolution for the rest of the major version — which is what ADR-005
promises and what downstream products depend on.

## Consequences

**Positive:**
- Callers branch on `LlmErrorKind` instead of parsing message text.
- A single call can carry its own timeout, retry budget, reasoning budget, tool
  constraint and cancellation token.
- `finish_reason` and `reasoning` survive the trip back to the caller.
- Every later field on these types is additive; this is the last literal break the
  contract takes.
- The `LlmProvider` trait, `chat_stream` and `HiveEvent` are all unchanged.

**Neutral:**
- `tokio-util` enters `pulsehive-core` for `CancellationToken` alone, with default
  features off. MASTER-SPEC §5.2 already lists it for cancellation.
- The provider crates implement against these fields in Round 2; until then they
  are carried and ignored, and `finish_reason` / `reasoning` are `None`.
- The runtime threads its own cancellation token into `LlmConfig::cancel` in a
  later spine.
- The Python and Node bindings compile against the new types but do not expose the
  new fields yet; exposing them is additive when it happens.

**Negative:**
- One coordinated break: struct literals for these types outside `pulsehive-core`
  must move to the constructors, and an exhaustive `match` on `PulseHiveError`
  needs a `_ =>` arm. Recorded under `[2.1.0] - Unreleased` in `CHANGELOG.md`; the
  published version number is a release-close decision.
- `LlmError::body` is verbatim, so a consumer that logs it without redacting can
  leak whatever the provider echoed back.
