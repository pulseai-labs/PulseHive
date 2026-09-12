//! Core-owned public identifier types (ADR-013).
//!
//! PulseHive owns its four public UUID identifiers: [`CollectiveId`],
//! [`ExperienceId`], [`InsightId`], and [`RelationId`]. Each is a transparent
//! newtype around [`uuid::Uuid`] preserving the observable PulseDB 0.7
//! contract — UUID-v7 `new()`, nil `Default`/`nil()`, `as_bytes()`,
//! `from_bytes()`, hyphenated `Display`, and transparent Serde — plus a
//! `FromStr` text round-trip. Conversion to and from PulseDB's own ID types
//! happens only inside the runtime's private adapter; there is no public
//! conversion API, no alias, and no fifth ID type.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Collective identifier (UUID v7 for time-ordering).
///
/// Collectives are isolated namespaces for agent experiences, typically one
/// per project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CollectiveId(pub Uuid);

impl CollectiveId {
    /// Creates a new CollectiveId with a UUID v7 (time-ordered).
    #[inline]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates a nil (all zeros) CollectiveId, useful as a sentinel.
    #[inline]
    pub fn nil() -> Self {
        Self(Uuid::nil())
    }

    /// Returns the raw 16 UUID bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Creates a CollectiveId from raw 16 UUID bytes.
    #[inline]
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }
}

impl Default for CollectiveId {
    /// Returns a nil (all zeros) CollectiveId.
    ///
    /// For a new unique ID, use [`CollectiveId::new()`].
    fn default() -> Self {
        Self::nil()
    }
}

impl fmt::Display for CollectiveId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for CollectiveId {
    type Err = uuid::Error;

    /// Parses the canonical hyphenated UUID text form.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::from_str(s).map(Self)
    }
}

/// Experience identifier (UUID v7 for time-ordering).
///
/// Experiences are the core unit of learned knowledge in PulseHive; each
/// belongs to exactly one collective.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExperienceId(pub Uuid);

impl ExperienceId {
    /// Creates a new ExperienceId with a UUID v7 (time-ordered).
    #[inline]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates a nil (all zeros) ExperienceId, useful as a sentinel.
    #[inline]
    pub fn nil() -> Self {
        Self(Uuid::nil())
    }

    /// Returns the raw 16 UUID bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Creates an ExperienceId from raw 16 UUID bytes.
    #[inline]
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }
}

impl Default for ExperienceId {
    /// Returns a nil (all zeros) ExperienceId.
    ///
    /// For a new unique ID, use [`ExperienceId::new()`].
    fn default() -> Self {
        Self::nil()
    }
}

impl fmt::Display for ExperienceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ExperienceId {
    type Err = uuid::Error;

    /// Parses the canonical hyphenated UUID text form.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::from_str(s).map(Self)
    }
}

/// Insight identifier (UUID v7 for time-ordering).
///
/// Insights are derived knowledge synthesized from experience clusters; each
/// belongs to exactly one collective.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InsightId(pub Uuid);

impl InsightId {
    /// Creates a new InsightId with a UUID v7 (time-ordered).
    #[inline]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates a nil (all zeros) InsightId, useful as a sentinel.
    #[inline]
    pub fn nil() -> Self {
        Self(Uuid::nil())
    }

    /// Returns the raw 16 UUID bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Creates an InsightId from raw 16 UUID bytes.
    #[inline]
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }
}

impl Default for InsightId {
    /// Returns a nil (all zeros) InsightId.
    ///
    /// For a new unique ID, use [`InsightId::new()`].
    fn default() -> Self {
        Self::nil()
    }
}

impl fmt::Display for InsightId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for InsightId {
    type Err = uuid::Error;

    /// Parses the canonical hyphenated UUID text form.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::from_str(s).map(Self)
    }
}

/// Relation identifier (UUID v7 for time-ordering).
///
/// Relations connect two experiences within the same collective, enabling
/// agents to perceive how knowledge connects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelationId(pub Uuid);

impl RelationId {
    /// Creates a new RelationId with a UUID v7 (time-ordered).
    #[inline]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates a nil (all zeros) RelationId, useful as a sentinel.
    #[inline]
    pub fn nil() -> Self {
        Self(Uuid::nil())
    }

    /// Returns the raw 16 UUID bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Creates a RelationId from raw 16 UUID bytes.
    #[inline]
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }
}

impl Default for RelationId {
    /// Returns a nil (all zeros) RelationId.
    ///
    /// For a new unique ID, use [`RelationId::new()`].
    fn default() -> Self {
        Self::nil()
    }
}

impl fmt::Display for RelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RelationId {
    type Err = uuid::Error;

    /// Parses the canonical hyphenated UUID text form.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::from_str(s).map(Self)
    }
}
