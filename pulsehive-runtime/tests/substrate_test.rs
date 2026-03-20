//! Integration tests for PulseDB substrate operations through PulseHive.

use pulsedb::{AgentId, ExperienceType, NewExperience};
use pulsehive_runtime::hivemind::HiveMind;

async fn build_hive(path: &std::path::Path) -> HiveMind {
    HiveMind::builder().substrate_path(path).build().unwrap()
}

#[tokio::test]
async fn test_store_and_retrieve_experience() {
    let dir = tempfile::tempdir().unwrap();
    let hive = build_hive(&dir.path().join("test.db")).await;
    let cid = hive
        .substrate()
        .get_or_create_collective("test")
        .await
        .unwrap();

    let exp = NewExperience {
        collective_id: cid,
        content: "Rust's ownership model prevents data races.".into(),
        experience_type: ExperienceType::Generic {
            category: Some("rust".into()),
        },
        embedding: None,
        importance: 0.8,
        confidence: 0.9,
        domain: vec!["rust".into()],
        source_agent: AgentId("agent-1".into()),
        source_task: None,
        related_files: vec![],
    };

    let id = hive.record_experience(exp).await.unwrap();
    let retrieved = hive.substrate().get_experience(id).await.unwrap().unwrap();
    assert_eq!(
        retrieved.content,
        "Rust's ownership model prevents data races."
    );
    assert_eq!(retrieved.source_agent.0, "agent-1");
}

#[tokio::test]
async fn test_multiple_experiences_in_collective() {
    let dir = tempfile::tempdir().unwrap();
    let hive = build_hive(&dir.path().join("test.db")).await;
    let cid = hive
        .substrate()
        .get_or_create_collective("multi")
        .await
        .unwrap();

    for i in 0..3 {
        let exp = NewExperience {
            collective_id: cid,
            content: format!("Experience number {i}"),
            experience_type: ExperienceType::Generic { category: None },
            embedding: None,
            importance: 0.5,
            confidence: 0.5,
            domain: vec![],
            source_agent: AgentId("agent".into()),
            source_task: None,
            related_files: vec![],
        };
        hive.record_experience(exp).await.unwrap();
    }

    let recent = hive.substrate().get_recent(cid, 10).await.unwrap();
    assert_eq!(recent.len(), 3);
}

#[tokio::test]
async fn test_persistence_across_instances() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persist.db");

    // First instance: store experience
    let cid = {
        let hive = build_hive(&path).await;
        let cid = hive
            .substrate()
            .get_or_create_collective("persist")
            .await
            .unwrap();
        let exp = NewExperience {
            collective_id: cid,
            content: "Persistent knowledge".into(),
            experience_type: ExperienceType::Generic { category: None },
            embedding: None,
            importance: 0.7,
            confidence: 0.8,
            domain: vec![],
            source_agent: AgentId("agent".into()),
            source_task: None,
            related_files: vec![],
        };
        hive.record_experience(exp).await.unwrap();
        cid
        // hive dropped here
    };

    // Second instance: verify experience persisted
    let hive2 = build_hive(&path).await;
    let cid2 = hive2
        .substrate()
        .get_or_create_collective("persist")
        .await
        .unwrap();
    assert_eq!(cid, cid2); // Same collective

    let recent = hive2.substrate().get_recent(cid2, 10).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].content, "Persistent knowledge");
}
