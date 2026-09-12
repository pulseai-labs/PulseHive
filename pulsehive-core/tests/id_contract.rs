//! Contract tests for the core-owned identifier types in `pulsehive_core::ids`.
//!
//! These tests pin the observable contract each public ID type carries:
//! deterministic byte round-trips, nil/default equivalence, hyphenated
//! `Display`, `FromStr` parse round-trips, transparent Serde representation,
//! and UUID-v7 generation — matching the PulseDB 0.7 counterparts byte-for-byte
//! and text-for-text so the event wire format is unchanged.

use pulsehive_core::ids::{CollectiveId, ExperienceId, InsightId, RelationId};

/// Fixed 16-byte fixture: deterministic, non-nil, version-agnostic.
const BYTES: [u8; 16] = [
    0x01, 0x93, 0xa8, 0x1f, 0x2b, 0x7c, 0x7a, 0x3e, 0x9d, 0x42, 0xb1, 0x0e, 0x5f, 0x77, 0x0c, 0xd4,
];

/// The same bytes as a canonical hyphenated UUID string.
const UUID_TEXT: &str = "0193a81f-2b7c-7a3e-9d42-b10e5f770cd4";

macro_rules! id_contract_tests {
    ($mod_name:ident, $core:ident, $db_crate:ident :: $db:ident) => {
        mod $mod_name {
            use super::*;

            #[test]
            fn from_bytes_as_bytes_round_trip() {
                let id = $core::from_bytes(BYTES);
                assert_eq!(*id.as_bytes(), BYTES);
            }

            #[test]
            fn nil_is_all_zero_bytes() {
                assert_eq!(*$core::nil().as_bytes(), [0u8; 16]);
            }

            #[test]
            fn default_is_nil() {
                assert_eq!($core::default(), $core::nil());
            }

            #[test]
            fn display_is_hyphenated_uuid() {
                let id = $core::from_bytes(BYTES);
                assert_eq!(id.to_string(), UUID_TEXT);
                // Wire-equivalent to the PulseDB counterpart's Display.
                assert_eq!(
                    id.to_string(),
                    $db_crate::$db::from_bytes(BYTES).to_string()
                );
            }

            #[test]
            fn from_str_round_trips() {
                let id: $core = UUID_TEXT.parse().expect("canonical UUID parses");
                assert_eq!(id, $core::from_bytes(BYTES));
                assert_eq!(id.to_string(), UUID_TEXT);
            }

            #[test]
            fn from_str_rejects_invalid() {
                assert!("not-a-uuid".parse::<$core>().is_err());
            }

            #[test]
            fn serde_json_is_transparent_uuid_string() {
                let id = $core::from_bytes(BYTES);
                let json = serde_json::to_string(&id).unwrap();
                assert_eq!(json, format!("\"{UUID_TEXT}\""));
                // Identical wire bytes to the PulseDB counterpart.
                assert_eq!(
                    json,
                    serde_json::to_string(&$db_crate::$db::from_bytes(BYTES)).unwrap()
                );
                let back: $core = serde_json::from_str(&json).unwrap();
                assert_eq!(back, id);
            }

            #[test]
            fn new_is_uuid_v7() {
                let id = $core::new();
                let uuid = uuid::Uuid::from_bytes(*id.as_bytes());
                assert_eq!(uuid.get_version_num(), 7, "new() must mint a UUID v7");
            }

            #[test]
            fn equality_and_hash() {
                use std::collections::HashSet;
                let a = $core::from_bytes(BYTES);
                let b = $core::from_bytes(BYTES);
                let c = $core::nil();
                assert_eq!(a, b);
                assert_ne!(a, c);
                let mut set = HashSet::new();
                set.insert(a);
                assert!(set.contains(&b));
                assert!(!set.contains(&c));
            }
        }
    };
}

id_contract_tests!(collective, CollectiveId, pulsedb::CollectiveId);
id_contract_tests!(experience, ExperienceId, pulsedb::ExperienceId);
id_contract_tests!(insight, InsightId, pulsedb::InsightId);
id_contract_tests!(relation, RelationId, pulsedb::RelationId);
