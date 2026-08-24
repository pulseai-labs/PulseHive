# ADR 006: Trust Boundaries & Destructive Operations

**Status:** Accepted
**Category:** 5 - Trust boundaries & destructive operations
**Touch Surface:** `pulsehive-anthropic/src/,pulsehive-openai/src/`
**Revisit Trigger:** When adding provider with different trust model

## Context

PulseHive interfaces with external LLM providers and manages agent tool execution.

## Decision

**Trust Boundaries:**
- **LlmProvider trait abstraction:** External provider access through trait interface
- **Provider authentication:** API keys managed by consumer, not stored by PulseHive
- **Tool execution boundary:** Agents execute tools through defined Tool interface
- **No destructive operations in library:** PulseHive does not spend money, delete data, or send to third parties directly

**Destructive Operations:**
- **Spending money:** Consumer handles API key provision and billing
- **Deleting data:** PulseHive operates on consumer-provided substrate; consumer owns deletion
- **Third-party communication:** Provider communication happens through consumer-authenticated channels

**Rationale:** LlmProvider abstraction prevents hardcoding trust in specific providers. Consumer controls authentication and billing. PulseHive itself performs no irreversible destructive actions.

## Consequences

**Positive:**
- Consumer controls all API keys and authentication
- Provider portability via trait interface
- No hidden costs or data deletion risks from library itself

**Neutral:**
- Consumer responsible for provider account management
- Tool safety delegated to consumer's tool implementations

**Negative:**
- PulseHive cannot implement features requiring direct billing/deletion
- Consumer must understand provider trust models
