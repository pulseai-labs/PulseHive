//! Private adapter between core-owned public IDs and PulseDB's ID types.
//!
//! ADR-013: `pulsehive-core` owns [`CollectiveId`], [`ExperienceId`],
//! [`InsightId`], and [`RelationId`]; `pulsedb` owns its own identically-named
//! types. The orphan rule forbids `From` impls between two foreign types, and
//! the spine contract forbids a public conversion API or compatibility alias —
//! so the lossless boundary lives here, in one crate-private module, as byte
//! conversions through `as_bytes`/`from_bytes`. Every `SubstrateProvider` call
//! receives a DB ID through `to_db_*`; every DB ID emitted through a core
//! event or public core contract converts back once through `from_db_*`.
//!
//! The module keeps the full symmetric surface — all four IDs, both
//! directions — so every boundary crossing routes through exactly one
//! function. Directions not yet exercised by production call sites are
//! covered by this module's round-trip tests.
#![allow(dead_code)]

use pulsehive_core::ids;

/// Converts a core [`ids::CollectiveId`] to its PulseDB counterpart.
pub(crate) fn to_db_collective_id(id: ids::CollectiveId) -> pulsedb::CollectiveId {
    pulsedb::CollectiveId::from_bytes(*id.as_bytes())
}

/// Converts a PulseDB `CollectiveId` back to the core-owned type.
pub(crate) fn from_db_collective_id(id: pulsedb::CollectiveId) -> ids::CollectiveId {
    ids::CollectiveId::from_bytes(*id.as_bytes())
}

/// Converts a core [`ids::ExperienceId`] to its PulseDB counterpart.
pub(crate) fn to_db_experience_id(id: ids::ExperienceId) -> pulsedb::ExperienceId {
    pulsedb::ExperienceId::from_bytes(*id.as_bytes())
}

/// Converts a PulseDB `ExperienceId` back to the core-owned type.
pub(crate) fn from_db_experience_id(id: pulsedb::ExperienceId) -> ids::ExperienceId {
    ids::ExperienceId::from_bytes(*id.as_bytes())
}

/// Converts a core [`ids::InsightId`] to its PulseDB counterpart.
pub(crate) fn to_db_insight_id(id: ids::InsightId) -> pulsedb::InsightId {
    pulsedb::InsightId::from_bytes(*id.as_bytes())
}

/// Converts a PulseDB `InsightId` back to the core-owned type.
pub(crate) fn from_db_insight_id(id: pulsedb::InsightId) -> ids::InsightId {
    ids::InsightId::from_bytes(*id.as_bytes())
}

/// Converts a core [`ids::RelationId`] to its PulseDB counterpart.
pub(crate) fn to_db_relation_id(id: ids::RelationId) -> pulsedb::RelationId {
    pulsedb::RelationId::from_bytes(*id.as_bytes())
}

/// Converts a PulseDB `RelationId` back to the core-owned type.
pub(crate) fn from_db_relation_id(id: pulsedb::RelationId) -> ids::RelationId {
    ids::RelationId::from_bytes(*id.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed 16-byte fixture covering every byte position.
    const BYTES: [u8; 16] = [
        0xde, 0xad, 0xbe, 0xef, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa,
        0xbb,
    ];

    #[test]
    fn collective_id_to_db_preserves_all_16_bytes() {
        let core = ids::CollectiveId::from_bytes(BYTES);
        let db = to_db_collective_id(core);
        assert_eq!(*db.as_bytes(), BYTES);
    }

    #[test]
    fn collective_id_from_db_preserves_all_16_bytes() {
        let db = pulsedb::CollectiveId::from_bytes(BYTES);
        let core = from_db_collective_id(db);
        assert_eq!(*core.as_bytes(), BYTES);
    }

    #[test]
    fn collective_id_round_trips_both_directions() {
        let core = ids::CollectiveId::from_bytes(BYTES);
        assert_eq!(from_db_collective_id(to_db_collective_id(core)), core);
        let db = pulsedb::CollectiveId::from_bytes(BYTES);
        assert_eq!(to_db_collective_id(from_db_collective_id(db)), db);
    }

    #[test]
    fn experience_id_to_db_preserves_all_16_bytes() {
        let core = ids::ExperienceId::from_bytes(BYTES);
        let db = to_db_experience_id(core);
        assert_eq!(*db.as_bytes(), BYTES);
    }

    #[test]
    fn experience_id_from_db_preserves_all_16_bytes() {
        let db = pulsedb::ExperienceId::from_bytes(BYTES);
        let core = from_db_experience_id(db);
        assert_eq!(*core.as_bytes(), BYTES);
    }

    #[test]
    fn experience_id_round_trips_both_directions() {
        let core = ids::ExperienceId::from_bytes(BYTES);
        assert_eq!(from_db_experience_id(to_db_experience_id(core)), core);
        let db = pulsedb::ExperienceId::from_bytes(BYTES);
        assert_eq!(to_db_experience_id(from_db_experience_id(db)), db);
    }

    #[test]
    fn insight_id_to_db_preserves_all_16_bytes() {
        let core = ids::InsightId::from_bytes(BYTES);
        let db = to_db_insight_id(core);
        assert_eq!(*db.as_bytes(), BYTES);
    }

    #[test]
    fn insight_id_from_db_preserves_all_16_bytes() {
        let db = pulsedb::InsightId::from_bytes(BYTES);
        let core = from_db_insight_id(db);
        assert_eq!(*core.as_bytes(), BYTES);
    }

    #[test]
    fn insight_id_round_trips_both_directions() {
        let core = ids::InsightId::from_bytes(BYTES);
        assert_eq!(from_db_insight_id(to_db_insight_id(core)), core);
        let db = pulsedb::InsightId::from_bytes(BYTES);
        assert_eq!(to_db_insight_id(from_db_insight_id(db)), db);
    }

    #[test]
    fn relation_id_to_db_preserves_all_16_bytes() {
        let core = ids::RelationId::from_bytes(BYTES);
        let db = to_db_relation_id(core);
        assert_eq!(*db.as_bytes(), BYTES);
    }

    #[test]
    fn relation_id_from_db_preserves_all_16_bytes() {
        let db = pulsedb::RelationId::from_bytes(BYTES);
        let core = from_db_relation_id(db);
        assert_eq!(*core.as_bytes(), BYTES);
    }

    #[test]
    fn relation_id_round_trips_both_directions() {
        let core = ids::RelationId::from_bytes(BYTES);
        assert_eq!(from_db_relation_id(to_db_relation_id(core)), core);
        let db = pulsedb::RelationId::from_bytes(BYTES);
        assert_eq!(to_db_relation_id(from_db_relation_id(db)), db);
    }
}
