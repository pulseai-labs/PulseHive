//! One-shot legacy collective fixture generator — DO NOT COMPILE IN THIS WORKSPACE.
//!
//! Provenance: this exact source was compiled and run inside a temporary
//! checkout of canonical PulseHive commit `a9f7839` (full SHA
//! `a9f783989ccb4d12eda9f4e5bd3318ef4e3df7ae`), where the workspace crates
//! `pulsehive-core`/`pulsehive-runtime` are version 2.0.2 and the root
//! `Cargo.toml` requires `pulsehive-db = "0.5"`, resolved by Cargo to
//! `pulsehive-db` 0.5.1. The published tag `v2.0.2` pins PulseDB 0.4 and is
//! therefore NOT the source of the fixture bytes.
//!
//! Exact invocation, run from the root of that temporary checkout:
//!
//! ```text
//! cargo run --example generate_legacy_fixture
//! ```
//!
//! It writes `legacy-collective.db` into the current directory and prints a
//! JSON provenance document (collective id, per-experience ids, generated
//! timestamps, embedding dimensions) on stdout. The database bytes plus that
//! JSON output were used to build the checked-in fixture; this file is
//! retained as provenance and is not a cargo target of the current workspace.

use pulsedb::{AgentId, ExperienceType, NewExperience};
use pulsehive_runtime::hivemind::HiveMind;
use serde_json::json;

const COLLECTIVE_NAME: &str = "legacy-collective-oracle";
const OUTPUT_PATH: &str = "legacy-collective.db";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Substrate path only, no embedding provider: HiveMindBuilder::build at
    // 2.0.2 opens PulseDB with Config::with_builtin_embeddings(), so PulseDB
    // 0.5.1 computes all-MiniLM-L6-v2 embeddings internally. Defaults for the
    // relationship detector and insight synthesizer are left untouched so the
    // bytes come from the unmodified real PulseHive 2.0.2 path.
    let hive = HiveMind::builder()
        .substrate_path(OUTPUT_PATH)
        .build()
        .map_err(|e| format!("build failed: {e}"))?;

    let substrate = hive.substrate();
    let collective_id = substrate
        .get_or_create_collective(COLLECTIVE_NAME)
        .await
        .map_err(|e| format!("get_or_create_collective failed: {e}"))?;

    let seeded = vec![
        (
            NewExperience {
                collective_id,
                content: "PulseHive opens its substrate on PulseDB with builtin \
                          all-MiniLM-L6-v2 embeddings whenever no external embedding \
                          provider is configured."
                    .to_string(),
                experience_type: ExperienceType::TechInsight {
                    technology: "pulsedb".to_string(),
                    insight: "builtin embedding mode requires no external provider".to_string(),
                },
                embedding: None,
                importance: 0.9,
                confidence: 0.85,
                domain: vec!["storage".to_string(), "embeddings".to_string()],
                source_agent: AgentId("legacy-oracle-generator".to_string()),
                source_task: None,
                related_files: vec![],
            },
            "exp-embeddings",
        ),
        (
            NewExperience {
                collective_id,
                content: "Experiences submitted with embedding None are embedded \
                          internally by PulseDB 0.5.1 before they are persisted to \
                          the collective."
                    .to_string(),
                experience_type: ExperienceType::Generic {
                    category: Some("persistence".to_string()),
                },
                embedding: None,
                importance: 0.7,
                confidence: 0.9,
                domain: vec!["pulsedb".to_string(), "storage".to_string()],
                source_agent: AgentId("legacy-oracle-generator".to_string()),
                source_task: None,
                related_files: vec!["pulsehive-runtime/src/hivemind.rs".to_string()],
            },
            "exp-persistence",
        ),
        (
            NewExperience {
                collective_id,
                content: "A collective is a name-scoped namespace; \
                          get_or_create_collective returns the same id across \
                          HiveMind instances for one name."
                    .to_string(),
                experience_type: ExperienceType::Generic {
                    category: Some("collectives".to_string()),
                },
                embedding: None,
                importance: 0.55,
                confidence: 0.75,
                domain: vec!["collectives".to_string(), "identity".to_string()],
                source_agent: AgentId("legacy-oracle-generator".to_string()),
                source_task: None,
                related_files: vec![],
            },
            "exp-collective-identity",
        ),
    ];

    let mut recorded = Vec::new();
    for (experience, label) in seeded {
        let id = hive
            .record_experience(experience)
            .await
            .map_err(|e| format!("record_experience failed for {label}: {e}"))?;
        // Read back through the same substrate so the manifest carries the
        // id/timestamp values PulseDB 0.5.1 actually generated.
        let stored = substrate
            .get_experience(id)
            .await
            .map_err(|e| format!("get_experience failed for {label}: {e}"))?
            .ok_or_else(|| format!("experience {label} vanished after record"))?;
        recorded.push(json!({
            "label": label,
            "id": stored.id.0.to_string(),
            "content": stored.content,
            "importance": stored.importance,
            "confidence": stored.confidence,
            "domain": stored.domain,
            "related_files": stored.related_files,
            "source_agent": stored.source_agent.0,
            "timestamp_ms": stored.timestamp.as_millis(),
            "last_reinforced_ms": stored.last_reinforced.as_millis(),
            "embedding_dim": stored.embedding.len(),
        }));
    }

    let collectives = substrate
        .list_collectives()
        .await
        .map_err(|e| format!("list_collectives failed: {e}"))?;
    let collective = collectives
        .iter()
        .find(|c| c.name == COLLECTIVE_NAME)
        .ok_or_else(|| format!("collective {COLLECTIVE_NAME} not listed after creation"))?;

    let provenance = json!({
        "output_path": OUTPUT_PATH,
        "collective": {
            "name": collective.name,
            "id": collective.id.0.to_string(),
            "embedding_dimension": collective.embedding_dimension,
        },
        "experiences": recorded,
    });
    println!("{}", serde_json::to_string_pretty(&provenance)?);

    hive.shutdown();
    Ok(())
}
