//! Event wire-preservation tests for the core-owned identifier types.
//!
//! ADR-013 moved `CollectiveId`, `ExperienceId`, `InsightId`, and `RelationId`
//! from `pulsedb` re-exports into `pulsehive_core::ids`. Both sides are
//! transparent `Uuid` newtypes, so `HiveEvent`'s tagged-JSON wire form must be
//! byte-for-byte unchanged: each ID field is a plain hyphenated UUID string,
//! and `to_string()` — the value the Python and JavaScript bindings emit —
//! stays the canonical text form.

use pulsehive_core::agent::{AgentKindTag, AgentOutcome};
use pulsehive_core::event::HiveEvent;
use pulsehive_core::ids::{CollectiveId, ExperienceId, InsightId, RelationId};
use serde_json::json;

/// Fixed 16-byte fixture: deterministic, non-nil, version-agnostic.
const BYTES: [u8; 16] = [
    0x01, 0x93, 0xa8, 0x1f, 0x2b, 0x7c, 0x7a, 0x3e, 0x9d, 0x42, 0xb1, 0x0e, 0x5f, 0x77, 0x0c, 0xd4,
];

/// The same bytes as a canonical hyphenated UUID string.
const UUID_TEXT: &str = "0193a81f-2b7c-7a3e-9d42-b10e5f770cd4";

const TS: u64 = 1_711_500_000_000;

#[test]
fn experience_recorded_wire_is_unchanged() {
    let event = HiveEvent::ExperienceRecorded {
        timestamp_ms: TS,
        experience_id: ExperienceId::from_bytes(BYTES),
        agent_id: "agent-1".into(),
        content_preview: "learned rust".into(),
        experience_type: "Generic".into(),
        importance: 0.8,
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "experience_recorded",
            "timestamp_ms": TS,
            "experience_id": UUID_TEXT,
            "agent_id": "agent-1",
            "content_preview": "learned rust",
            "experience_type": "Generic",
            // f32 widens to f64 in serde_json's Number — the pre-existing
            // wire representation, pinned here intentionally.
            "importance": f64::from(0.8_f32),
        })
    );

    let back: HiveEvent = serde_json::from_value(value).unwrap();
    assert!(matches!(
        back,
        HiveEvent::ExperienceRecorded { experience_id, .. } if experience_id == ExperienceId::from_bytes(BYTES)
    ));
}

#[test]
fn relationship_inferred_wire_is_unchanged() {
    let event = HiveEvent::RelationshipInferred {
        timestamp_ms: TS,
        relation_id: RelationId::from_bytes(BYTES),
        agent_id: "agent-1".into(),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "relationship_inferred",
            "timestamp_ms": TS,
            "relation_id": UUID_TEXT,
            "agent_id": "agent-1",
        })
    );

    let back: HiveEvent = serde_json::from_value(value).unwrap();
    assert!(matches!(
        back,
        HiveEvent::RelationshipInferred { relation_id, .. } if relation_id == RelationId::from_bytes(BYTES)
    ));
}

#[test]
fn insight_generated_wire_is_unchanged() {
    let event = HiveEvent::InsightGenerated {
        timestamp_ms: TS,
        insight_id: InsightId::from_bytes(BYTES),
        source_count: 3,
        agent_id: "agent-1".into(),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "insight_generated",
            "timestamp_ms": TS,
            "insight_id": UUID_TEXT,
            "source_count": 3,
            "agent_id": "agent-1",
        })
    );

    let back: HiveEvent = serde_json::from_value(value).unwrap();
    assert!(matches!(
        back,
        HiveEvent::InsightGenerated { insight_id, .. } if insight_id == InsightId::from_bytes(BYTES)
    ));
}

#[test]
fn watch_notification_wire_is_unchanged() {
    let event = HiveEvent::WatchNotification {
        timestamp_ms: TS,
        experience_id: ExperienceId::from_bytes(BYTES),
        collective_id: CollectiveId::from_bytes(BYTES),
        event_type: "Created".into(),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "watch_notification",
            "timestamp_ms": TS,
            "experience_id": UUID_TEXT,
            "collective_id": UUID_TEXT,
            "event_type": "Created",
        })
    );

    let back: HiveEvent = serde_json::from_value(value).unwrap();
    assert!(matches!(
        back,
        HiveEvent::WatchNotification { experience_id, collective_id, .. }
            if experience_id == ExperienceId::from_bytes(BYTES)
                && collective_id == CollectiveId::from_bytes(BYTES)
    ));
}

#[test]
fn agent_lifecycle_wire_is_unchanged() {
    let started = HiveEvent::AgentStarted {
        timestamp_ms: TS,
        agent_id: "agent-1".into(),
        name: "researcher".into(),
        kind: AgentKindTag::Llm,
        collective_id: CollectiveId::from_bytes(BYTES),
        task_description: "analyze".into(),
    };
    let value = serde_json::to_value(&started).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "agent_started",
            "timestamp_ms": TS,
            "agent_id": "agent-1",
            "name": "researcher",
            "kind": "llm",
            "collective_id": UUID_TEXT,
            "task_description": "analyze",
        })
    );

    let completed = HiveEvent::AgentCompleted {
        timestamp_ms: TS,
        agent_id: "agent-1".into(),
        outcome: AgentOutcome::Complete {
            response: "done".into(),
        },
        collective_id: CollectiveId::from_bytes(BYTES),
        task_description: "analyze".into(),
    };
    let value = serde_json::to_value(&completed).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "agent_completed",
            "timestamp_ms": TS,
            "agent_id": "agent-1",
            "outcome": { "status": "complete", "response": "done" },
            "collective_id": UUID_TEXT,
            "task_description": "analyze",
        })
    );
}

#[test]
fn core_id_wire_bytes_equal_pulsedb_wire_bytes() {
    // The pre-change wire used PulseDB's transparent-Uuid newtypes; the
    // post-change wire must serialize identically, byte for byte.
    assert_eq!(
        serde_json::to_string(&CollectiveId::from_bytes(BYTES)).unwrap(),
        serde_json::to_string(&pulsedb::CollectiveId::from_bytes(BYTES)).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&ExperienceId::from_bytes(BYTES)).unwrap(),
        serde_json::to_string(&pulsedb::ExperienceId::from_bytes(BYTES)).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&InsightId::from_bytes(BYTES)).unwrap(),
        serde_json::to_string(&pulsedb::InsightId::from_bytes(BYTES)).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&RelationId::from_bytes(BYTES)).unwrap(),
        serde_json::to_string(&pulsedb::RelationId::from_bytes(BYTES)).unwrap()
    );
}

#[test]
fn to_string_is_the_binding_wire_value() {
    // pulsehive-py and pulsehive-js stringify event ID fields; the emitted
    // text must equal the PulseDB counterpart's Display byte for byte.
    assert_eq!(
        CollectiveId::from_bytes(BYTES).to_string(),
        pulsedb::CollectiveId::from_bytes(BYTES).to_string()
    );
    assert_eq!(CollectiveId::from_bytes(BYTES).to_string(), UUID_TEXT);
    assert_eq!(
        ExperienceId::from_bytes(BYTES).to_string(),
        pulsedb::ExperienceId::from_bytes(BYTES).to_string()
    );
    assert_eq!(
        InsightId::from_bytes(BYTES).to_string(),
        pulsedb::InsightId::from_bytes(BYTES).to_string()
    );
    assert_eq!(
        RelationId::from_bytes(BYTES).to_string(),
        pulsedb::RelationId::from_bytes(BYTES).to_string()
    );
}
