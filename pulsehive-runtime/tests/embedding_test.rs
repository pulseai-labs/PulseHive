//! Integration tests for EmbeddingProvider end-to-end flow.

use async_trait::async_trait;
use pulsedb::NewExperience;
use pulsehive_core::embedding::EmbeddingProvider;
use pulsehive_core::error::Result;
use pulsehive_runtime::hivemind::HiveMind;

/// Mock embedding provider that returns deterministic vectors.
struct MockEmbeddingProvider {
    dims: usize,
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        // Deterministic embedding based on text hash
        let hash = text.bytes().fold(0u32, |acc, b| acc.wrapping_add(b as u32));
        Ok((0..self.dims)
            .map(|i| ((hash as f32 + i as f32) * 0.001).sin())
            .collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

/// Mock provider that always fails.
struct FailingEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for FailingEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Err(pulsehive_core::error::PulseHiveError::embedding(
            "Mock embedding failure",
        ))
    }

    fn dimensions(&self) -> usize {
        384
    }
}

#[tokio::test]
async fn test_build_with_embedding_provider() {
    let dir = tempfile::tempdir().unwrap();
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("embed.db"))
        .embedding_provider(MockEmbeddingProvider { dims: 384 })
        .build()
        .unwrap();

    assert!(!hive.is_shutdown());
    hive.shutdown();
}

#[tokio::test]
async fn test_build_without_embedding_provider() {
    let dir = tempfile::tempdir().unwrap();
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("no-embed.db"))
        .build()
        .unwrap();

    assert!(!hive.is_shutdown());
    hive.shutdown();
}

#[tokio::test]
async fn test_record_experience_with_embedding_provider() {
    let dir = tempfile::tempdir().unwrap();
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("record-embed.db"))
        .embedding_provider(MockEmbeddingProvider { dims: 384 })
        .no_relationship_detector()
        .no_insight_synthesizer()
        .build()
        .unwrap();

    let cid = hive
        .substrate()
        .get_or_create_collective("test-embed")
        .await
        .unwrap();

    let exp = NewExperience {
        collective_id: cid,
        content: "Test experience for embedding".into(),
        experience_type: pulsedb::ExperienceType::Generic { category: None },
        embedding: None, // Should be computed by provider
        importance: 0.5,
        confidence: 0.5,
        domain: vec![],
        source_agent: pulsedb::AgentId("test".into()),
        source_task: None,
        related_files: vec![],
    };

    let id = hive.record_experience(exp).await.unwrap();

    // Verify the experience was stored (embedding computed by provider)
    let stored = hive.substrate().get_experience(id).await.unwrap();
    assert!(stored.is_some(), "Experience should be stored");
    let stored = stored.unwrap();
    assert!(
        !stored.embedding.is_empty(),
        "Embedding should be non-empty (computed by provider or PulseDB)"
    );
}

#[tokio::test]
async fn test_record_experience_with_failing_provider() {
    let dir = tempfile::tempdir().unwrap();
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("fail-embed.db"))
        .embedding_provider(FailingEmbeddingProvider)
        .no_relationship_detector()
        .no_insight_synthesizer()
        .build()
        .unwrap();

    let cid = hive
        .substrate()
        .get_or_create_collective("test-fail")
        .await
        .unwrap();

    let exp = NewExperience {
        collective_id: cid,
        content: "Test experience with failing embedding".into(),
        experience_type: pulsedb::ExperienceType::Generic { category: None },
        embedding: None,
        importance: 0.5,
        confidence: 0.5,
        domain: vec![],
        source_agent: pulsedb::AgentId("test".into()),
        source_task: None,
        related_files: vec![],
    };

    // In external mode, PulseDB requires embeddings. When the provider fails,
    // the embedding remains None and PulseDB rejects the store.
    // This is correct behavior — external mode demands valid embeddings.
    let result = hive.record_experience(exp).await;
    assert!(
        result.is_err(),
        "Should fail to store without embedding in external mode"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("embedding"),
        "Error should mention embedding: {err_msg}"
    );
}

#[tokio::test]
async fn test_record_experience_without_provider_uses_builtin() {
    let dir = tempfile::tempdir().unwrap();
    let hive = HiveMind::builder()
        .substrate_path(dir.path().join("builtin-embed.db"))
        .no_relationship_detector()
        .no_insight_synthesizer()
        .build()
        .unwrap();

    let cid = hive
        .substrate()
        .get_or_create_collective("test-builtin")
        .await
        .unwrap();

    let exp = NewExperience {
        collective_id: cid,
        content: "Test experience for builtin embedding".into(),
        experience_type: pulsedb::ExperienceType::Generic { category: None },
        embedding: None, // PulseDB builtin computes this
        importance: 0.5,
        confidence: 0.5,
        domain: vec![],
        source_agent: pulsedb::AgentId("test".into()),
        source_task: None,
        related_files: vec![],
    };

    let id = hive.record_experience(exp).await.unwrap();
    let stored = hive.substrate().get_experience(id).await.unwrap();
    assert!(stored.is_some());
    let stored = stored.unwrap();
    assert!(
        !stored.embedding.is_empty(),
        "Builtin embeddings should produce non-empty embedding"
    );
}
