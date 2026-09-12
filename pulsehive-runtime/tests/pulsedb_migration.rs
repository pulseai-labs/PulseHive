//! PulseDB 0.5.1 to 0.7 migration and agent-perception proof.
//!
//! Every test copies the checked-in PulseHive 2.0.2/PulseDB 0.5.1 database
//! before opening it. The copied bytes exercise PulseDB-owned migration while
//! the immutable fixture remains the rollback oracle.

use std::pin::Pin;
use std::time::Duration;

use futures::StreamExt;
use futures_core::Stream;
use pulsedb::{CollectiveId, Config, ExperienceId, PulseDB};
use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::lens::Lens;
use pulsehive_core::llm::{LlmConfig, Message};
use pulsehive_core::testing::ScriptedProvider;
use pulsehive_runtime::hivemind::{HiveMind, Task};
use serde_json::Value;
use sha2::{Digest, Sha256};

const FIXTURE_DIR: &str = "tests/fixtures/pulsedb-0.5.1-pulsehive-2.0.2";

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURE_DIR)
        .join(name)
}

/// Loads `manifest.json` and asserts it carries exactly the provenance the
/// oracle claims, including that the checked-in `collective.db` bytes hash
/// to the manifest's `sha256` pin.
fn load_manifest_and_check_provenance() -> Value {
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_path("manifest.json")).expect("manifest.json readable"),
    )
    .expect("manifest.json parses");

    assert_eq!(manifest["schema_version"], 1, "schema version");
    assert_eq!(
        manifest["pulsehive_commit"], "a9f783989ccb4d12eda9f4e5bd3318ef4e3df7ae",
        "generator commit"
    );
    assert_eq!(manifest["pulsehive_version"], "2.0.2", "PulseHive version");
    assert_eq!(manifest["pulsehive_db_version"], "0.5.1", "PulseDB version");
    assert_fixture_matches_sha256_pin(&manifest);
    manifest
}

/// Enforces the manifest's `sha256` pin against the actual `collective.db`
/// bytes. A regenerated or edited fixture fails here with both digests
/// instead of silently invalidating the migration oracle the tests assert
/// against; regenerate `manifest.json` alongside the fixture (see the
/// fixture directory's README) when the oracle is deliberately replaced.
fn assert_fixture_matches_sha256_pin(manifest: &Value) {
    let pinned = manifest["sha256"]
        .as_str()
        .expect("manifest.json pins a sha256")
        .to_ascii_lowercase();
    let bytes = std::fs::read(fixture_path("collective.db")).expect("collective.db readable");
    let actual: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    assert_eq!(
        pinned, actual,
        "collective.db does not match the manifest sha256 pin {pinned} (actual {actual}); \
         the legacy oracle bytes changed without updating manifest.json"
    );
}

/// Copies the checked-in database to a fresh temporary directory. The
/// returned `TempDir` must stay alive for as long as the copy is open; the
/// checked-in bytes are never a writable test target.
fn copy_fixture(db_path: &std::path::Path) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let copy = tmp.path().join("collective.db");
    std::fs::copy(db_path, &copy).expect("fixture copied to temp dir");
    (tmp, copy)
}

/// Asserts the seeded collective reads back under its recorded identity.
async fn assert_collective_identity(hive: &HiveMind, collective: &Value) {
    let collective_name = collective["name"].as_str().expect("collective name");
    let collectives = hive
        .substrate()
        .list_collectives()
        .await
        .expect("list_collectives");
    assert_eq!(
        collectives.len(),
        1,
        "migration and deploy must not create another collective"
    );
    let matching: Vec<_> = collectives
        .iter()
        .filter(|c| c.name == collective_name)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one collective named {collective_name}"
    );
    assert_eq!(
        matching[0].id.0.to_string(),
        collective["id"].as_str().expect("collective id"),
        "collective id matches the manifest"
    );
}

/// Reads one seeded experience back through the substrate and asserts every
/// recorded field. Values were captured from 0.5.1 output, so comparisons
/// are exact; the JSON f64s are widenings of the stored f32s.
async fn read_back_one_experience(hive: &HiveMind, exp: &Value) {
    let label = exp["label"].as_str().unwrap_or("unlabeled");
    let id = exp["id"].as_str().expect("experience id");
    let id = uuid::Uuid::parse_str(id).expect("experience id parses as UUID");
    let stored = hive
        .substrate()
        .get_experience(ExperienceId(id))
        .await
        .expect("get_experience")
        .unwrap_or_else(|| panic!("experience {label} reads back from the copy"));
    assert_experience_fields(&stored, exp, label);
}

/// Field-by-field semantic oracle for one seeded experience.
fn assert_experience_fields(stored: &pulsedb::Experience, exp: &Value, label: &str) {
    assert_eq!(stored.content, exp["content"].as_str().expect("content"));
    assert_eq!(
        stored.importance,
        exp["importance"].as_f64().expect("importance") as f32
    );
    assert_eq!(
        stored.confidence,
        exp["confidence"].as_f64().expect("confidence") as f32
    );
    let expected_domain: Vec<&str> = exp["domain"]
        .as_array()
        .expect("domain array")
        .iter()
        .map(|d| d.as_str().expect("domain tag"))
        .collect();
    assert_eq!(stored.domain, expected_domain, "domain for {label}");
    assert!(stored.tags.is_empty(), "migrated tags for {label}");
    let expected_files: Vec<&str> = exp["related_files"]
        .as_array()
        .expect("related_files array")
        .iter()
        .map(|f| f.as_str().expect("related file"))
        .collect();
    assert_eq!(
        stored.related_files, expected_files,
        "related_files for {label}"
    );
    assert_eq!(
        stored.source_agent.0,
        exp["source_agent"].as_str().expect("source agent")
    );
    assert_eq!(
        stored.timestamp.as_millis(),
        exp["timestamp_ms"].as_i64().expect("timestamp_ms"),
        "timestamp for {label}"
    );
    assert_eq!(
        stored.last_reinforced.as_millis(),
        exp["last_reinforced_ms"]
            .as_i64()
            .expect("last_reinforced_ms"),
        "last_reinforced for {label}"
    );
    let embedding_dim = exp["embedding_dim"].as_u64().expect("embedding_dim");
    assert_eq!(
        stored.embedding.len() as u64,
        embedding_dim,
        "stored builtin embedding dimension for {label}"
    );
    assert!(
        stored.embedding.iter().any(|v| *v != 0.0),
        "builtin embedding for {label} is a real vector"
    );
}

fn manifest_collective_id(manifest: &Value) -> CollectiveId {
    let id = manifest["collective"]["id"]
        .as_str()
        .expect("collective id");
    CollectiveId(uuid::Uuid::parse_str(id).expect("collective id parses as UUID"))
}

fn scripted_agent() -> AgentDefinition {
    AgentDefinition {
        name: "migration-agent".into(),
        kind: AgentKind::Llm(Box::new(LlmAgentConfig {
            system_prompt: "Verify the migrated knowledge.".into(),
            tools: vec![],
            lens: Lens::default(),
            llm_config: LlmConfig::new("scripted", "migration-test"),
            experience_extractor: None,
            refresh_every_n_tool_calls: None,
        })),
    }
}

async fn drain_to_completed(
    stream: &mut Pin<Box<dyn Stream<Item = HiveEvent> + Send>>,
) -> AgentOutcome {
    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = stream.next().await {
            if let HiveEvent::AgentCompleted { outcome, .. } = event {
                return outcome;
            }
        }
        panic!("event stream ended without AgentCompleted");
    })
    .await
    .expect("event drain timed out before AgentCompleted")
}

fn assert_first_request_has_legacy_knowledge(provider: &ScriptedProvider, experiences: &[Value]) {
    let requests = provider.requests();
    assert_eq!(requests.len(), 1, "one scripted agent completion");
    let context = requests[0]
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::System { content } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    for experience in experiences {
        let content = experience["content"].as_str().expect("content");
        assert!(
            context.contains(content),
            "first request lacks legacy content: {content}"
        );
    }
}

async fn assert_post_migration_write(hive: &HiveMind, collective_id: CollectiveId) {
    let experiences = hive
        .substrate()
        .get_recent(collective_id, 10)
        .await
        .expect("read migrated and new experiences");
    let stored = experiences
        .iter()
        .find(|experience| experience.content.contains("Migration proof complete."))
        .expect("agent turn records a new experience");
    assert!(!stored.source_agent.0.is_empty());
    assert_eq!(stored.embedding.len(), 384);
    assert!(stored.embedding.iter().any(|value| *value != 0.0));
    assert!(stored.tags.is_empty());
}

#[tokio::test]
async fn legacy_collective_migrates_and_agent_perceives_it() {
    let manifest = load_manifest_and_check_provenance();
    let fixture = fixture_path("collective.db");
    let pristine = std::fs::read(&fixture).expect("collective.db readable");
    assert!(!pristine.is_empty(), "collective.db is non-empty");

    let (_tmp, copy) = copy_fixture(&fixture);
    let provider = ScriptedProvider::new().then_text("Migration proof complete.");
    let hive = HiveMind::builder()
        .substrate_path(&copy)
        .llm_provider("scripted", provider.clone())
        .no_relationship_detector()
        .no_insight_synthesizer()
        .build()
        .expect("HiveMind migrates the fixture copy");
    let collective_id = manifest_collective_id(&manifest);
    let task = Task::with_collective("Use the migrated knowledge", collective_id);

    let mut stream = hive
        .deploy(vec![scripted_agent()], vec![task])
        .await
        .expect("deploy agent against migrated collective");
    let outcome = drain_to_completed(&mut stream).await;
    match outcome {
        AgentOutcome::Complete { response } => {
            assert_eq!(response, "Migration proof complete.");
        }
        other => panic!("expected completed migration turn, got {other:?}"),
    }

    let experiences = manifest["experiences"]
        .as_array()
        .expect("experiences array");
    assert_eq!(experiences.len(), 3, "fixture contains three experiences");
    assert_first_request_has_legacy_knowledge(&provider, experiences);
    assert_collective_identity(&hive, &manifest["collective"]).await;
    for experience in experiences {
        read_back_one_experience(&hive, experience).await;
    }
    assert_post_migration_write(&hive, collective_id).await;
    hive.shutdown();
    drop(hive);

    let after = std::fs::read(&fixture).expect("collective.db still readable");
    assert_eq!(pristine, after, "checked-in fixture must remain unchanged");
}

#[tokio::test]
async fn builtin_identity_and_rollback_artifacts_are_stable() {
    let _manifest = load_manifest_and_check_provenance();
    let fixture = fixture_path("collective.db");
    let pristine = std::fs::read(&fixture).expect("collective.db readable");
    let (_tmp, copy) = copy_fixture(&fixture);

    let hive = HiveMind::builder()
        .substrate_path(&copy)
        .build()
        .expect("first writable open migrates the fixture copy");
    drop(hive);

    let pre_substrate = copy.with_extension("db.pre-substrate.bak");
    let pre_v4 = copy.with_extension("db.pre-v4.bak");
    assert!(pre_substrate.is_file(), "full rollback backup exists");
    assert!(pre_v4.is_file(), "schema-v3 backup exists");
    assert_eq!(
        std::fs::read(&pre_substrate).expect("full rollback backup readable"),
        pristine,
        "pre-substrate backup must retain the exact 0.5.1 bytes"
    );

    let database = PulseDB::open(&copy, Config::with_builtin_embeddings())
        .expect("reopen migrated copy with builtin identity");
    let first_identity = database.provider_identity().expect("provider identity");
    assert_eq!(first_identity.provider, "builtin-onnx");
    assert!(first_identity.model_id.starts_with("onnx-"));
    assert_ne!(first_identity.model_id, "main_graph");
    drop(database);

    let database = PulseDB::open(&copy, Config::with_builtin_embeddings())
        .expect("second reopen with builtin identity");
    assert_eq!(
        database
            .provider_identity()
            .expect("stable provider identity"),
        first_identity
    );
    drop(database);

    assert_eq!(
        std::fs::read(&fixture).expect("checked-in fixture still readable"),
        pristine,
        "identity reopens must not touch the checked-in oracle"
    );
}
