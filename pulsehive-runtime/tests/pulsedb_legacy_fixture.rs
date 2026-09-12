//! Baseline readback test for the checked-in PulseDB 0.5.1 legacy fixture.
//!
//! The fixture at `tests/fixtures/pulsedb-0.5.1-pulsehive-2.0.2/` is the
//! immutable pre-upgrade oracle for the PulseDB 0.7 migration: real database
//! bytes written by PulseHive 2.0.2 crates against `pulsehive-db` 0.5.1
//! through the builtin embedding path. This test proves the current 0.5
//! builtin configuration can still read the collective and every seeded
//! experience with its recorded content and metadata, and that the run left
//! the checked-in bytes untouched. The migration work item must reproduce
//! these assertions under 0.7 after migrating the same bytes.

use pulsedb::ExperienceId;
use pulsehive_runtime::hivemind::HiveMind;
use serde_json::Value;

const FIXTURE_DIR: &str = "tests/fixtures/pulsedb-0.5.1-pulsehive-2.0.2";

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURE_DIR)
        .join(name)
}

/// Loads `manifest.json` and asserts it carries exactly the provenance the
/// oracle claims.
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
    manifest
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

#[tokio::test]
async fn legacy_collective_fixture_reads_back_under_builtin_0_5() {
    let manifest = load_manifest_and_check_provenance();
    let db_path = fixture_path("collective.db");

    // The checked-in bytes are an oracle pinned by SHA-256 in the manifest;
    // snapshot them so the run can prove it never wrote to the fixture.
    let before = std::fs::read(&db_path).expect("collective.db readable");
    assert!(!before.is_empty(), "collective.db is non-empty");

    let (_tmp, copy) = copy_fixture(&db_path);
    // Substrate path only: the builder opens the copy with the current 0.5
    // builtin embedding configuration, matching how the fixture was written.
    let hive = HiveMind::builder()
        .substrate_path(&copy)
        .build()
        .expect("HiveMind opens the fixture copy");

    assert_collective_identity(&hive, &manifest["collective"]).await;
    let experiences = manifest["experiences"]
        .as_array()
        .expect("experiences array");
    assert!(
        experiences.len() >= 2,
        "fixture seeds at least two experiences"
    );
    for exp in experiences {
        read_back_one_experience(&hive, exp).await;
    }
    hive.shutdown();

    // Prove the run left the checked-in fixture bytes — and therefore the
    // manifest's pinned SHA-256 — unchanged.
    let after = std::fs::read(&db_path).expect("collective.db still readable");
    assert_eq!(
        before, after,
        "checked-in collective.db must be byte-identical after the run"
    );
}
