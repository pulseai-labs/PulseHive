//! PulseHive Multi-Agent Workflow Example
//!
//! Demonstrates composing agents into workflows:
//! 1. **Sequential pipeline** — researcher → summarizer, each perceiving previous results
//! 2. **Parallel team** — two reviewers working concurrently on the same substrate
//! 3. **Nested workflow** — parallel analysis followed by sequential summary
//!
//! Uses MockLlm so no API key is required. Run with:
//! ```bash
//! cargo run -p pulsehive-runtime --example multi_agent_workflow
//! ```

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use pulsehive_core::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::*;
use pulsehive_runtime::hivemind::{HiveMind, Task};

/// Mock LLM that returns different responses based on the system prompt.
///
/// In a real application, you'd use `pulsehive_openai::OpenAICompatibleProvider`
/// or `pulsehive_anthropic::AnthropicProvider` instead.
struct MockLlm {
    call_count: AtomicUsize,
}

impl MockLlm {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for MockLlm {
    async fn chat(
        &self,
        messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        _config: &LlmConfig,
    ) -> pulsehive_core::error::Result<LlmResponse> {
        let n = self.call_count.fetch_add(1, Ordering::Relaxed);

        // Extract system prompt to tailor the response
        let system = messages
            .iter()
            .find_map(|m| match m {
                Message::System { content } => Some(content.as_str()),
                _ => None,
            })
            .unwrap_or("");

        let response = if system.contains("research") {
            format!("[Research findings #{n}] PulseHive uses a shared consciousness model where agents perceive each other's experiences through a persistent substrate.")
        } else if system.contains("summariz") {
            format!("[Summary #{n}] Key finding: shared consciousness enables implicit agent coordination without message passing.")
        } else if system.contains("frontend") {
            format!("[Frontend Review #{n}] Component architecture follows best practices.")
        } else if system.contains("backend") {
            format!("[Backend Review #{n}] API endpoints are well-structured with proper error handling.")
        } else {
            format!("[Response #{n}] Analysis complete.")
        };

        Ok(LlmResponse {
            content: Some(response),
            tool_calls: vec![],
            usage: TokenUsage::default(),
        })
    }

    async fn chat_stream(
        &self,
        _m: Vec<Message>,
        _t: Vec<ToolDefinition>,
        _c: &LlmConfig,
    ) -> pulsehive_core::error::Result<
        Pin<Box<dyn futures_core::Stream<Item = pulsehive_core::error::Result<LlmChunk>> + Send>>,
    > {
        Err(pulsehive_core::error::PulseHiveError::llm(
            "Streaming not supported in mock",
        ))
    }
}

/// Helper to create an LLM agent definition.
fn llm_agent(name: &str, prompt: &str, domains: &[&str]) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: prompt.into(),
            tools: vec![],
            lens: Lens::new(domains.iter().copied()),
            llm_config: LlmConfig::new("mock", "demo"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("multi_agent.db"))
        .llm_provider("mock", MockLlm::new())
        .build()?;

    // ── 1. Sequential Pipeline ──────────────────────────────────────
    // The summarizer perceives the researcher's experiences through
    // the shared substrate — no explicit message passing needed.
    println!("=== Sequential Pipeline: Research → Summarize ===\n");

    let pipeline = AgentDefinition {
        name: "research-pipeline".into(),
        kind: AgentKind::Sequential(vec![
            llm_agent(
                "researcher",
                "You research topics thoroughly. Provide detailed findings.",
                &["research"],
            ),
            llm_agent(
                "summarizer",
                "You summarize research findings into bullet points.",
                &["research", "summary"],
            ),
        ]),
    };

    let mut stream = hive
        .deploy(
            vec![pipeline],
            vec![Task::new("Research PulseHive architecture")],
        )
        .await?;
    while let Some(event) = stream.next().await {
        let data = format!("{event:?}");
        if data.contains("AgentStarted") || data.contains("AgentCompleted") {
            println!("  {data}");
        }
    }

    // ── 2. Parallel Team ────────────────────────────────────────────
    // Both reviewers work concurrently, sharing the substrate in real-time.
    println!("\n=== Parallel Team: Frontend + Backend Review ===\n");

    let team = AgentDefinition {
        name: "review-team".into(),
        kind: AgentKind::Parallel(vec![
            llm_agent(
                "frontend-reviewer",
                "You review frontend code for best practices.",
                &["frontend", "ui"],
            ),
            llm_agent(
                "backend-reviewer",
                "You review backend code for performance and security.",
                &["backend", "security"],
            ),
        ]),
    };

    let mut stream = hive
        .deploy(vec![team], vec![Task::new("Review the web application")])
        .await?;
    while let Some(event) = stream.next().await {
        let data = format!("{event:?}");
        if data.contains("AgentStarted") || data.contains("AgentCompleted") {
            println!("  {data}");
        }
    }

    // ── 3. Nested Workflow ──────────────────────────────────────────
    // Parallel analysis → Sequential summary
    println!("\n=== Nested: Parallel Analysis → Summary ===\n");

    let nested = AgentDefinition {
        name: "full-review".into(),
        kind: AgentKind::Sequential(vec![
            AgentDefinition {
                name: "parallel-analysis".into(),
                kind: AgentKind::Parallel(vec![
                    llm_agent("analyst-a", "You research performance.", &["performance"]),
                    llm_agent("analyst-b", "You research security.", &["security"]),
                ]),
            },
            llm_agent(
                "final-summary",
                "You summarize all findings into an executive report.",
                &["performance", "security", "summary"],
            ),
        ]),
    };

    let mut stream = hive
        .deploy(vec![nested], vec![Task::new("Full system review")])
        .await?;
    while let Some(event) = stream.next().await {
        let data = format!("{event:?}");
        if data.contains("AgentStarted") || data.contains("AgentCompleted") {
            println!("  {data}");
        }
    }

    hive.shutdown();
    println!("\nDone! All workflows completed.");
    Ok(())
}
