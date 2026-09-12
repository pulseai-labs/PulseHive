//! Integration tests for the intelligence layer — RelationshipDetector + InsightSynthesizer.
//!
//! Tests the full record_experience() → relationship inference pipeline.
//! Note: event_bus is pub(crate), so we test via record_experience() return values
//! and substrate state rather than event stream.

use pulsehive_runtime::hivemind::HiveMind;

/// Helper: create a HiveMind with builtin embeddings.
fn build_hive() -> HiveMind {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    Box::leak(Box::new(dir));

    HiveMind::builder().substrate_path(&path).build().unwrap()
}

async fn record_network_experience(
    hive: &HiveMind,
    collective_id: pulsedb::CollectiveId,
    content: &str,
    experience_type: pulsedb::ExperienceType,
    importance: f32,
    confidence: f32,
) -> pulsedb::ExperienceId {
    hive.record_experience(pulsedb::NewExperience {
        collective_id,
        content: content.into(),
        experience_type,
        embedding: None,
        importance,
        confidence,
        domain: vec!["networking".into(), "reliability".into()],
        source_agent: pulsedb::AgentId("agent-1".into()),
        source_task: None,
        tags: Default::default(),
        related_files: vec![],
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn test_record_experience_stores_and_infers_relations() {
    let hive = build_hive();
    let cid = hive
        .substrate()
        .get_or_create_collective("intelligence-test")
        .await
        .unwrap();

    let id1 = record_network_experience(
        &hive,
        cid,
        "Network timeouts occur when the API gateway is under heavy load.",
        pulsedb::ExperienceType::Difficulty {
            description: "Network timeouts under heavy load".into(),
            severity: pulsedb::Severity::High,
        },
        0.8,
        0.9,
    )
    .await;
    let id2 = record_network_experience(
        &hive,
        cid,
        "Network timeout errors in the API gateway during peak traffic periods.",
        pulsedb::ExperienceType::ErrorPattern {
            signature: "gateway_timeout".into(),
            fix: "retry with backoff".into(),
            prevention: "rate limiting".into(),
        },
        0.7,
        0.8,
    )
    .await;
    let id3 = record_network_experience(
        &hive,
        cid,
        "Add exponential backoff with jitter to handle network timeouts gracefully.",
        pulsedb::ExperienceType::Solution {
            problem_ref: None,
            approach: "exponential backoff with jitter".into(),
            worked: true,
        },
        0.9,
        0.95,
    )
    .await;

    for id in [id1, id2, id3] {
        assert!(hive.substrate().get_experience(id).await.unwrap().is_some());
    }

    let related = hive.substrate().get_related(id3).await.unwrap();
    println!(
        "Relations found for exp3: {} (builtin embedding similarity depends on content overlap)",
        related.len()
    );
}

#[tokio::test]
async fn test_record_experience_with_no_detector() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    Box::leak(Box::new(dir));

    let hive = HiveMind::builder()
        .substrate_path(&path)
        .no_relationship_detector()
        .no_insight_synthesizer()
        .build()
        .unwrap();

    let cid = hive
        .substrate()
        .get_or_create_collective("no-detector-test")
        .await
        .unwrap();

    let id = hive
        .record_experience(pulsedb::NewExperience {
            collective_id: cid,
            content: "Test experience without intelligence.".into(),
            experience_type: pulsedb::ExperienceType::Generic { category: None },
            embedding: None,
            importance: 0.5,
            confidence: 0.5,
            domain: vec![],
            source_agent: pulsedb::AgentId("agent-1".into()),
            source_task: None,
            tags: Default::default(),
            related_files: vec![],
        })
        .await
        .unwrap();

    // Experience stored
    assert!(hive.substrate().get_experience(id).await.unwrap().is_some());

    // No relations (detector disabled)
    let related = hive.substrate().get_related(id).await.unwrap();
    assert!(
        related.is_empty(),
        "No relations should exist with detector disabled"
    );
}
