//! Persistence test: substrate continuity across HiveMind instances.
//!
//! Validates the core "shared consciousness" promise: experiences from
//! one session are perceivable in the next.

use pulsedb::{AgentId, ExperienceType, NewExperience};
use pulsehive_runtime::hivemind::HiveMind;

#[tokio::test]
async fn test_experiences_persist_across_hivemind_instances() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persist.db");

    // Phase 1: Store an experience
    {
        let hive = HiveMind::builder().substrate_path(&path).build().unwrap();

        let cid = hive
            .substrate()
            .get_or_create_collective("project")
            .await
            .unwrap();

        let exp = NewExperience {
            collective_id: cid,
            content: "The authentication module uses JWT tokens with 24h expiry.".into(),
            experience_type: ExperienceType::TechInsight {
                technology: "auth".into(),
                insight: "JWT with 24h expiry".into(),
            },
            embedding: None,
            importance: 0.8,
            confidence: 0.9,
            domain: vec!["auth".into(), "security".into()],
            tags: Default::default(),
            source_agent: AgentId("agent-phase1".into()),
            source_task: None,
            related_files: vec![],
        };
        hive.record_experience(exp).await.unwrap();
    } // HiveMind dropped, DB closed

    // Phase 2: Reopen and verify persistence
    {
        let hive = HiveMind::builder().substrate_path(&path).build().unwrap();

        let cid = hive
            .substrate()
            .get_or_create_collective("project")
            .await
            .unwrap();

        let recent = hive.substrate().get_recent(cid, 10).await.unwrap();
        assert!(
            !recent.is_empty(),
            "Experiences should persist across instances"
        );
        assert!(recent[0].content.contains("JWT tokens"));
    }
}
