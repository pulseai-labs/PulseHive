# ADR 006: Trust Boundaries & Destructive Operations

**Status:** Accepted
**Category:** 5 - Trust boundaries & destructive operations
**Touch Surface:** `pulsehive-core/src/,pulsehive-runtime/src/,pulsehive-anthropic/src/,pulsehive-openai/src/`
**Revisit Trigger:** When adding provider with different trust model

## Context

PulseHive interfaces with external LLM providers and manages agent tool execution.

## Decision

**Trust Boundaries:**
- **LlmProvider trait abstraction:** External provider access through trait interface
- **Provider authentication:** Consumer supplies the API key; built-in providers retain it as an owned `String` in their config (`OpenAIConfig`, `AnthropicConfig`) for the provider's lifetime, and `AnthropicConfig`'s derived `Debug` can print it (hardening tracked in #56)
- **Tool execution boundary:** Agents execute tools through the consumer-defined Tool interface; consumer `Tool::execute` implementations can cause arbitrary side effects
- **Third-party egress and charges:** Built-in providers send authenticated HTTP POSTs to configured third-party endpoints from library code — data disclosure and charges on the consumer's API account are possible

**Destructive Operations:**
- **Spending money:** Library-initiated provider calls can incur charges on the consumer's API account; the consumer controls the key, the model choice, and the endpoints it configures
- **Deleting data:** PulseHive operates on consumer-provided substrate; consumer owns deletion
- **Third-party communication:** Provider requests are authenticated with the consumer's key but issued by PulseHive library code, not by the consumer's own runtime

**Rationale:** LlmProvider abstraction prevents hardcoding trust in specific providers. The consumer supplies credentials and chooses endpoints and tool implementations, while PulseHive code performs the outbound provider calls — so threat reviews must treat provider traffic (with its disclosure and billing exposure) as SDK behavior, and tool side effects as consumer-controlled.

## Consequences

**Positive:**
- Consumer supplies all API keys and chooses provider endpoints and tools
- Provider portability via trait interface
- Egress, charge, and key-retention exposure documented rather than assumed away

**Neutral:**
- Consumer responsible for provider account management
- Tool safety delegated to consumer's tool implementations

**Negative:**
- Library-initiated provider calls mean data-disclosure and charge risks exist even when the consumer never makes an HTTP call itself
- Consumer must understand provider trust models
