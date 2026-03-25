# pulsehive-anthropic

**Anthropic Claude LLM provider for PulseHive.**

Supports the full Claude model family via the Messages API with native tool use support.

| Model | ID |
|-------|-----|
| Claude Opus 4.6 | `claude-opus-4-6` |
| Claude Sonnet 4.6 | `claude-sonnet-4-6` |
| Claude Haiku 4.5 | `claude-haiku-4-5-20251001` |

## Usage

```toml
[dependencies]
pulsehive = { version = "1.0", features = ["anthropic"] }
```

```rust
use pulsehive_anthropic::AnthropicProvider;

let provider = AnthropicProvider::new("sk-ant-...");
```

With custom configuration:
```rust
use pulsehive_anthropic::{AnthropicConfig, AnthropicProvider};

let config = AnthropicConfig::new("sk-ant-...")
    .with_model("claude-sonnet-4-6");
let provider = AnthropicProvider::from_config(config);
```

Register with HiveMind:
```rust
let hive = HiveMind::builder()
    .substrate_path("my.db")
    .llm_provider("anthropic", provider)
    .build()?;
```

## Features

- Messages API with `tool_use` content blocks
- Automatic retry on 429 (rate limit) and 529 (overloaded)
- System prompt extraction from message history
- Multi-block response parsing (text + tool_use)

## Links

- [pulsehive (meta-crate)](https://crates.io/crates/pulsehive)
- [API Docs](https://docs.rs/pulsehive-anthropic)
- [GitHub](https://github.com/pulsehive/pulsehive)

## License

AGPL-3.0-only
