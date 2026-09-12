//! Always-on surface proof: `pulsehive-core` compiles and behaves identically
//! with and without the `substrate` feature.
//!
//! This test names no storage-coupled item: `ToolContext` appears only in a
//! type position (its field set differs by feature, a recorded Release 1
//! break) and `PulseHiveError` is never matched exhaustively.

use pulsehive_core::prelude::*;

#[test]
fn always_on_surface_constructible_and_stable() {
    // The four core-owned identifiers are always available.
    let collective = CollectiveId::new();
    let experience = ExperienceId::new();
    let insight = InsightId::new();
    let relation = RelationId::new();
    assert_ne!(experience, ExperienceId::nil());
    let _ = (insight, relation);

    // `LlmAgentConfig` keeps one literal shape in both configurations.
    let config = LlmAgentConfig {
        system_prompt: "You are helpful.".into(),
        tools: Vec::new(),
        lens: Lens::new(["surface"]),
        llm_config: LlmConfig::new("openai", "gpt-4"),
        experience_extractor: None,
        refresh_every_n_tool_calls: None,
    };
    assert!(config.experience_extractor.is_none());
    assert_eq!(config.lens.attention_budget, 50);

    // `ExtractionContext` stays always-on even though `extract` is gated.
    let ctx = ExtractionContext {
        agent_id: "agent-1".into(),
        collective_id: collective,
        task_description: "prove the surface".into(),
    };
    assert_eq!(ctx.agent_id, "agent-1");

    // An ID-bearing `HiveEvent` round-trips through Serde with core IDs.
    let event = HiveEvent::WatchNotification {
        timestamp_ms: 0,
        experience_id: experience,
        collective_id: collective,
        event_type: "Created".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: HiveEvent = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        back,
        HiveEvent::WatchNotification {
            experience_id,
            collective_id,
            ..
        } if experience_id == experience && collective_id == collective
    ));

    // A non-substrate `PulseHiveError` constructs in both configurations.
    let err = PulseHiveError::validation("empty content");
    assert!(err.to_string().contains("empty content"));

    // `ExperienceTypeTag` behavior does not depend on the substrate.
    assert_eq!(ExperienceTypeTag::all().len(), 9);
}

/// Names the rest of the always-on surface in type positions: the items
/// exist without `substrate`, and the trait objects stay object-safe.
#[test]
fn always_on_surface_type_positions() {
    let _: Option<&ToolContext> = None;
    let _: Option<&ToolResult> = None;
    let _: Option<&EventEmitter> = None;
    let _: Option<&dyn ExperienceExtractor> = None;
    let _: Option<&Lens> = None;
    let _: Option<ExperienceTypeTag> = None;
    let _: Option<&PulseHiveError> = None;
    let _: Option<&dyn LlmProvider> = None;
}
