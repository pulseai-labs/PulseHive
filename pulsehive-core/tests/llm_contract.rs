//! Consumer-view contract tests for the `pulsehive-core` LLM transport types.
//!
//! This file is an integration test, so it compiles as an external crate and sees
//! exactly the surface a downstream consumer sees.

use std::time::Duration;

use pulsehive_core::error::PulseHiveError;
use pulsehive_core::llm::{
    LlmConfig, LlmError, LlmErrorKind, LlmResponse, ReasoningEffort, TokenUsage, ToolCall,
    ToolChoice,
};
use tokio_util::sync::CancellationToken;

/// The 2.0.2 wire shape of `LlmConfig` must survive this item byte for byte.
#[test]
fn llm_config_pre_existing_shape_serializes_byte_identical_to_2_0_2() {
    let mut config = LlmConfig::new("openai", "gpt-4o");
    config.temperature = 0.2;
    config.max_tokens = 512;

    assert_eq!(
        serde_json::to_string(&config).unwrap(),
        r#"{"provider":"openai","model":"gpt-4o","temperature":0.2,"max_tokens":512}"#
    );
}

/// A 2.0.2 payload still deserializes; every field added by this item is absent.
#[test]
fn llm_config_2_0_2_json_deserializes_with_new_fields_none() {
    let json = r#"{"provider":"openai","model":"gpt-4o","temperature":0.2,"max_tokens":512}"#;
    let config: LlmConfig = serde_json::from_str(json).unwrap();

    assert_eq!(config.provider, "openai");
    assert_eq!(config.model, "gpt-4o");
    assert_eq!(config.timeout_secs, None);
    assert_eq!(config.max_retries, None);
    assert_eq!(config.reasoning_effort, None);
    assert_eq!(config.tool_choice, None);
    assert!(config.cancel.is_none());
}

/// New fields are additive on the wire: a key appears only once it is set.
#[test]
fn llm_config_new_fields_serialize_only_when_set() {
    let base = serde_json::to_string(&LlmConfig::new("openai", "gpt-4o")).unwrap();
    assert!(!base.contains("timeout_secs"));
    assert!(!base.contains("max_retries"));
    assert!(!base.contains("reasoning_effort"));
    assert!(!base.contains("tool_choice"));

    let timeout = serde_json::to_string(&LlmConfig::new("o", "m").with_timeout_secs(5)).unwrap();
    assert!(timeout.contains(r#""timeout_secs":5"#), "{timeout}");

    // Zero is a set value, not an absent one.
    let retries = serde_json::to_string(&LlmConfig::new("o", "m").with_max_retries(0)).unwrap();
    assert!(retries.contains(r#""max_retries":0"#), "{retries}");

    let effort = serde_json::to_string(
        &LlmConfig::new("o", "m").with_reasoning_effort(ReasoningEffort::Low),
    )
    .unwrap();
    assert!(effort.contains(r#""reasoning_effort":"low""#), "{effort}");

    let choice =
        serde_json::to_string(&LlmConfig::new("o", "m").with_tool_choice(ToolChoice::Required))
            .unwrap();
    assert!(choice.contains(r#""tool_choice":"required""#), "{choice}");
}

/// The cancellation carrier is runtime state, never wire state — and cloning a
/// config shares the token, which is what r1.s2's thread-through relies on.
#[test]
fn llm_config_cancel_token_never_serializes_and_clones_share_cancellation() {
    let config = LlmConfig::new("openai", "gpt-4o").with_cancel(CancellationToken::new());

    let json = serde_json::to_string(&config).unwrap();
    assert!(!json.contains("cancel"), "{json}");

    let round_tripped: LlmConfig = serde_json::from_str(&json).unwrap();
    assert!(round_tripped.cancel.is_none());

    let clone = config.clone();
    clone.cancel.as_ref().unwrap().cancel();
    assert!(config.cancel.as_ref().unwrap().is_cancelled());
}

#[test]
fn llm_response_constructors_and_builders() {
    let text = LlmResponse::text("hi");
    assert_eq!(text.content.as_deref(), Some("hi"));
    assert!(text.tool_calls.is_empty());
    assert_eq!(text.finish_reason, None);
    assert_eq!(text.reasoning, None);

    let annotated = LlmResponse::text("hi")
        .with_finish_reason("stop")
        .with_reasoning("thought about it");
    assert_eq!(annotated.finish_reason.as_deref(), Some("stop"));
    assert_eq!(annotated.reasoning.as_deref(), Some("thought about it"));

    let empty = LlmResponse::new(None, vec![], TokenUsage::default());
    assert_eq!(empty.content, None);
    assert!(empty.tool_calls.is_empty());

    let with_calls = LlmResponse::new(None, vec![], TokenUsage::default())
        .with_tool_calls(vec![ToolCall {
            id: "call_1".into(),
            name: "search".into(),
            arguments: serde_json::json!({"query": "rust"}),
        }])
        .with_usage(TokenUsage {
            input_tokens: 7,
            output_tokens: 3,
        });
    assert_eq!(with_calls.tool_calls.len(), 1);
    assert_eq!(with_calls.usage.input_tokens, 7);
    assert_eq!(with_calls.usage.output_tokens, 3);
}

#[test]
fn llm_error_display_and_conversion() {
    let err = LlmError::new(LlmErrorKind::Timeout, "deadline");
    assert_eq!(err.attempts, 1);
    assert_eq!(err.status, None);
    assert_eq!(err.body, None);
    assert_eq!(err.finish_reason, None);
    assert_eq!(err.retry_after, None);

    let rendered = err.to_string();
    assert!(rendered.contains("timeout"), "{rendered}");
    assert!(rendered.contains("1 attempt"), "{rendered}");

    let with_status = err.clone().with_status(503);
    assert!(with_status.to_string().contains("HTTP 503"));

    let full = LlmError::new(LlmErrorKind::RateLimited, "slow down")
        .with_attempts(3)
        .with_body("{\"error\":\"rate\"}")
        .with_finish_reason("length")
        .with_retry_after(Duration::from_secs(2));
    assert_eq!(full.attempts, 3);
    assert_eq!(full.body.as_deref(), Some("{\"error\":\"rate\"}"));
    assert_eq!(full.finish_reason.as_deref(), Some("length"));
    assert_eq!(full.retry_after, Some(Duration::from_secs(2)));

    assert!(matches!(
        PulseHiveError::from(err.clone()),
        PulseHiveError::LlmTransport(_)
    ));
    assert!(matches!(
        PulseHiveError::llm_transport(err),
        PulseHiveError::LlmTransport(_)
    ));

    // The stringly variant is untouched: it still exists and still routes.
    assert!(matches!(PulseHiveError::llm("x"), PulseHiveError::Llm(_)));
}

#[test]
fn llm_error_kind_serde_snake_case() {
    assert_eq!(
        serde_json::to_string(&LlmErrorKind::MalformedToolCall).unwrap(),
        r#""malformed_tool_call""#
    );
    assert_eq!(
        serde_json::from_str::<LlmErrorKind>(r#""malformed_tool_call""#).unwrap(),
        LlmErrorKind::MalformedToolCall
    );

    assert_eq!(
        serde_json::to_string(&LlmErrorKind::RateLimited).unwrap(),
        r#""rate_limited""#
    );
    assert_eq!(
        serde_json::from_str::<LlmErrorKind>(r#""rate_limited""#).unwrap(),
        LlmErrorKind::RateLimited
    );
}

#[test]
fn tool_choice_serde_shapes() {
    assert_eq!(
        serde_json::to_string(&ToolChoice::Auto).unwrap(),
        r#""auto""#
    );
    assert_eq!(
        serde_json::from_str::<ToolChoice>(r#""auto""#).unwrap(),
        ToolChoice::Auto
    );

    assert_eq!(
        serde_json::to_string(&ToolChoice::None).unwrap(),
        r#""none""#
    );
    assert_eq!(
        serde_json::from_str::<ToolChoice>(r#""none""#).unwrap(),
        ToolChoice::None
    );

    let function = ToolChoice::Function { name: "f".into() };
    assert_eq!(
        serde_json::to_string(&function).unwrap(),
        r#"{"function":{"name":"f"}}"#
    );
    assert_eq!(
        serde_json::from_str::<ToolChoice>(r#"{"function":{"name":"f"}}"#).unwrap(),
        function
    );
}

#[test]
fn reasoning_effort_serde_lowercase() {
    for (variant, wire) in [
        (ReasoningEffort::Minimal, r#""minimal""#),
        (ReasoningEffort::Low, r#""low""#),
        (ReasoningEffort::Medium, r#""medium""#),
        (ReasoningEffort::High, r#""high""#),
    ] {
        assert_eq!(serde_json::to_string(&variant).unwrap(), wire);
        assert_eq!(
            serde_json::from_str::<ReasoningEffort>(wire).unwrap(),
            variant
        );
    }
}
