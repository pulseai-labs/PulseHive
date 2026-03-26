//! Live API integration tests for pulsehive-openai.
//!
//! These tests call a real OpenAI-compatible API (GLM) and are skipped by default.
//! Run with: `cargo test -p pulsehive-openai --test live_api_test -- --ignored`
//!
//! Requires `.env` file at repo root with:
//!   PULSEHIVE_API_KEY=your-key
//!   PULSEHIVE_BASE_URL=https://api.z.ai/api/coding/paas/v4
//!   PULSEHIVE_MODEL=GLM-4.7

use futures::StreamExt;
use pulsehive_core::llm::*;
use pulsehive_openai::{OpenAICompatibleProvider, OpenAIConfig};

fn setup() -> (OpenAICompatibleProvider, LlmConfig) {
    dotenvy::from_filename("../.env")
        .or_else(|_| dotenvy::dotenv())
        .ok();
    let api_key = std::env::var("PULSEHIVE_API_KEY").expect("Set PULSEHIVE_API_KEY in .env");
    let base_url = std::env::var("PULSEHIVE_BASE_URL").expect("Set PULSEHIVE_BASE_URL in .env");
    let model = std::env::var("PULSEHIVE_MODEL").expect("Set PULSEHIVE_MODEL in .env");

    let provider =
        OpenAICompatibleProvider::new(OpenAIConfig::new(&api_key, &model).with_base_url(&base_url));
    let config = LlmConfig::new("openai", &model);
    (provider, config)
}

#[tokio::test]
#[ignore]
async fn live_single_turn_chat() {
    let (provider, config) = setup();

    let response = provider
        .chat(
            vec![
                Message::system("You are concise. Respond in one word only."),
                Message::user("Say hello."),
            ],
            vec![],
            &config,
        )
        .await
        .unwrap();

    assert!(
        response.content.is_some(),
        "Expected text content in response"
    );
    let text = response.content.unwrap();
    assert!(!text.is_empty(), "Response should not be empty");
    println!("Single-turn response: {text}");
}

#[tokio::test]
#[ignore]
async fn live_multi_turn_tool_calling() {
    let (provider, config) = setup();

    // Define a simple tool
    let tools = vec![ToolDefinition {
        name: "get_weather".into(),
        description: "Get the current weather for a city".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name" }
            },
            "required": ["city"]
        }),
    }];

    // Turn 1: Ask about weather — LLM should call the tool
    let response = provider
        .chat(
            vec![
                Message::system(
                    "You have access to a weather tool. Use it when asked about weather.",
                ),
                Message::user("What's the weather in Tokyo?"),
            ],
            tools.clone(),
            &config,
        )
        .await
        .unwrap();

    assert!(
        !response.tool_calls.is_empty(),
        "Expected tool call for weather query, got text: {:?}",
        response.content
    );
    let tool_call = &response.tool_calls[0];
    assert_eq!(tool_call.name, "get_weather");
    println!(
        "Tool call: {} with args: {}",
        tool_call.name, tool_call.arguments
    );

    // Turn 2: Send tool result back — LLM should respond with final answer
    // This is the exact flow that was broken by GH Issue #1
    let response2 = provider
        .chat(
            vec![
                Message::system(
                    "You have access to a weather tool. Use it when asked about weather.",
                ),
                Message::user("What's the weather in Tokyo?"),
                Message::assistant_with_tool_calls(response.tool_calls.clone()),
                Message::tool_result(&tool_call.id, "Tokyo: 22°C, sunny, humidity 45%"),
            ],
            tools,
            &config,
        )
        .await
        .unwrap();

    assert!(
        response2.content.is_some(),
        "Expected final text response after tool result"
    );
    let text = response2.content.unwrap();
    assert!(!text.is_empty(), "Final response should not be empty");
    println!("Final response: {text}");
}

#[tokio::test]
#[ignore]
async fn live_streaming_chat() {
    let (provider, config) = setup();

    let stream = provider
        .chat_stream(
            vec![
                Message::system("You are concise."),
                Message::user("Count from 1 to 5."),
            ],
            vec![],
            &config,
        )
        .await
        .unwrap();

    let chunks: Vec<LlmChunk> = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();

    assert!(!chunks.is_empty(), "Expected streaming chunks, got none");
    let full_text: String = chunks
        .iter()
        .filter_map(|c| match c {
            LlmChunk::Text(content) => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert!(!full_text.is_empty(), "Assembled text should not be empty");
    println!("Streamed response ({} chunks): {full_text}", chunks.len());
}

#[tokio::test]
#[ignore]
async fn live_invalid_api_key_returns_error() {
    dotenvy::from_filename("../.env")
        .or_else(|_| dotenvy::dotenv())
        .ok();
    let base_url = std::env::var("PULSEHIVE_BASE_URL").expect("Set PULSEHIVE_BASE_URL in .env");
    let model = std::env::var("PULSEHIVE_MODEL").expect("Set PULSEHIVE_MODEL in .env");

    let provider = OpenAICompatibleProvider::new(
        OpenAIConfig::new("invalid-key-12345", &model).with_base_url(&base_url),
    );
    let config = LlmConfig::new("openai", &model);

    let result = provider
        .chat(vec![Message::user("Hello")], vec![], &config)
        .await;

    assert!(result.is_err(), "Invalid API key should return error");
    println!("Error (expected): {}", result.unwrap_err());
}
