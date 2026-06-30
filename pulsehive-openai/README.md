# pulsehive-openai

**OpenAI-compatible LLM provider for PulseHive.**

Works with any OpenAI-compatible API — not just OpenAI:

| Provider | Base URL |
|----------|----------|
| **OpenAI** | `https://api.openai.com/v1` (default) |
| **Azure OpenAI** | `https://{resource}.openai.azure.com/...` |
| **Ollama** | `http://localhost:11434/v1` |
| **vLLM** | `http://localhost:8000/v1` |
| **LM Studio** | `http://localhost:1234/v1` |
| **Groq** | `https://api.groq.com/openai/v1` |
| **Together** | `https://api.together.xyz/v1` |

## Usage

```toml
[dependencies]
pulsehive = { version = "1.0", features = ["openai"] }
```

```rust
use pulsehive_openai::{OpenAIConfig, OpenAICompatibleProvider};

// OpenAI
let provider = OpenAICompatibleProvider::new(
    OpenAIConfig::new("sk-...", "gpt-4")
);

// Ollama (local)
let provider = OpenAICompatibleProvider::new(
    OpenAIConfig::new("unused", "llama3")
        .with_base_url("http://localhost:11434/v1")
);
```

Register with HiveMind:
```rust
let hive = HiveMind::builder()
    .substrate_path("my.db")
    .llm_provider("openai", provider)
    .build()?;
```

## Features

- Chat completions with tool calling support
- SSE streaming responses
- Automatic retry on 429 (rate limit) and 5xx errors
- Configurable base URL for any OpenAI-compatible endpoint

## Links

- [pulsehive (meta-crate)](https://crates.io/crates/pulsehive)
- [API Docs](https://docs.rs/pulsehive-openai)
- [GitHub](https://github.com/pulseai-labs/PulseHive)

## License

AGPL-3.0-only
