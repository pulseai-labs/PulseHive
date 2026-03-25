//! PulseHive Custom Tool Example
//!
//! Demonstrates implementing the `Tool` trait to give agents capabilities:
//! 1. **Calculator** — arithmetic operations, returns text results
//! 2. **WordCounter** — text analysis, returns JSON results
//! 3. **DatabaseWrite** — requires human approval before execution
//!
//! Uses MockLlm that calls tools in sequence. No API key required. Run with:
//! ```bash
//! cargo run -p pulsehive-runtime --example custom_tool
//! ```

use std::pin::Pin;
use std::sync::Arc;

use futures::StreamExt;
use pulsehive_core::agent::{AgentDefinition, AgentKind, LlmAgentConfig};
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::*;
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};
use pulsehive_runtime::hivemind::{HiveMind, Task};

// ── Tool 1: Calculator ──────────────────────────────────────────────
// Returns a text result. Demonstrates basic Tool trait implementation.

struct Calculator;

#[async_trait::async_trait]
impl Tool for Calculator {
    fn name(&self) -> &str {
        "calculator"
    }

    fn description(&self) -> &str {
        "Performs basic arithmetic: add, subtract, multiply, divide"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["add", "subtract", "multiply", "divide"]
                },
                "a": { "type": "number" },
                "b": { "type": "number" }
            },
            "required": ["operation", "a", "b"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        context: &ToolContext,
    ) -> pulsehive_core::error::Result<ToolResult> {
        let op = params["operation"].as_str().unwrap_or("add");
        let a = params["a"].as_f64().unwrap_or(0.0);
        let b = params["b"].as_f64().unwrap_or(0.0);

        println!(
            "    [Calculator] {} {} {} (agent: {})",
            a,
            op,
            b,
            &context.agent_id[..8]
        );

        let result = match op {
            "add" => a + b,
            "subtract" => a - b,
            "multiply" => a * b,
            "divide" if b != 0.0 => a / b,
            "divide" => return Ok(ToolResult::error("Division by zero")),
            _ => return Ok(ToolResult::error(format!("Unknown operation: {op}"))),
        };

        Ok(ToolResult::text(format!("{result}")))
    }
}

// ── Tool 2: WordCounter ─────────────────────────────────────────────
// Returns a JSON result. Demonstrates structured tool output.

struct WordCounter;

#[async_trait::async_trait]
impl Tool for WordCounter {
    fn name(&self) -> &str {
        "word_counter"
    }

    fn description(&self) -> &str {
        "Analyzes text: word count, character count, sentence count"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to analyze" }
            },
            "required": ["text"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _context: &ToolContext,
    ) -> pulsehive_core::error::Result<ToolResult> {
        let text = params["text"].as_str().unwrap_or("");
        let words = text.split_whitespace().count();
        let characters = text.len();
        let sentences =
            text.matches('.').count() + text.matches('!').count() + text.matches('?').count();

        println!("    [WordCounter] {words} words, {characters} chars, {sentences} sentences");

        // Return structured JSON — the LLM receives this as a JSON string
        Ok(ToolResult::json(serde_json::json!({
            "words": words,
            "characters": characters,
            "sentences": sentences
        })))
    }
}

// ── Tool 3: DatabaseWrite ───────────────────────────────────────────
// Requires human approval. Demonstrates the approval workflow.

struct DatabaseWrite;

#[async_trait::async_trait]
impl Tool for DatabaseWrite {
    fn name(&self) -> &str {
        "database_write"
    }

    fn description(&self) -> &str {
        "Writes data to the database (requires approval)"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "table": { "type": "string" },
                "data": { "type": "object" }
            },
            "required": ["table", "data"]
        })
    }

    /// This tool requires human approval before execution.
    ///
    /// The framework calls `ApprovalHandler::request_approval()` before
    /// executing this tool. The default `AutoApprove` handler approves everything.
    fn requires_approval(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _context: &ToolContext,
    ) -> pulsehive_core::error::Result<ToolResult> {
        let table = params["table"].as_str().unwrap_or("unknown");
        println!("    [DatabaseWrite] Writing to table '{table}' (approved)");
        Ok(ToolResult::text(format!("Wrote to {table} successfully")))
    }
}

// ── Mock LLM ────────────────────────────────────────────────────────
// Simulates an LLM that decides to call the calculator tool.

struct ToolCallingLlm {
    called: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl LlmProvider for ToolCallingLlm {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        _config: &LlmConfig,
    ) -> pulsehive_core::error::Result<LlmResponse> {
        // First call: use the calculator tool (if available)
        if !self.called.swap(true, std::sync::atomic::Ordering::Relaxed) && !tools.is_empty() {
            Ok(LlmResponse {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "calculator".into(),
                    arguments: serde_json::json!({"operation": "multiply", "a": 6, "b": 7}),
                }],
                usage: TokenUsage::default(),
            })
        } else {
            // Second call: respond with the final answer
            Ok(LlmResponse {
                content: Some(
                    "The answer is 42! I used the calculator tool to compute 6 × 7.".into(),
                ),
                tool_calls: vec![],
                usage: TokenUsage::default(),
            })
        }
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("tools.db"))
        .llm_provider(
            "mock",
            ToolCallingLlm {
                called: std::sync::atomic::AtomicBool::new(false),
            },
        )
        .build()?;

    // Create an agent with all three tools
    let agent = AgentDefinition {
        name: "tool-user".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "You are a helpful assistant with access to calculator, word_counter, and database_write tools.".into(),
            tools: vec![
                Arc::new(Calculator),
                Arc::new(WordCounter),
                Arc::new(DatabaseWrite),
            ],
            lens: Lens::new(["tools"]),
            llm_config: LlmConfig::new("mock", "demo"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    };

    println!("=== Custom Tool Example ===");
    println!(
        "Agent '{}' with 3 tools: calculator, word_counter, database_write\n",
        agent.name
    );

    let mut stream = hive
        .deploy(vec![agent], vec![Task::new("Calculate 6 × 7")])
        .await?;

    while let Some(event) = stream.next().await {
        let data = format!("{event:?}");
        if data.contains("AgentStarted") {
            println!("  Agent started");
        } else if data.contains("ToolCallStarted") {
            println!("  → Tool call started");
        } else if data.contains("ToolCallCompleted") {
            println!("  ← Tool call completed");
        } else if data.contains("AgentCompleted") {
            println!("  Agent completed");
            if data.contains("Complete") {
                // Extract the response
                if let Some(start) = data.find("response: \"") {
                    let rest = &data[start + 11..];
                    if let Some(end) = rest.find('"') {
                        println!("\n  Response: {}", &rest[..end]);
                    }
                }
            }
        }
    }

    hive.shutdown();
    println!("\nDone!");
    Ok(())
}
