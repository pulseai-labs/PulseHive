//! Lens-based perception for agent substrate access.
//!
//! A [`Lens`] controls how an agent perceives the shared substrate. Different agents
//! see the same data differently based on their domain focus, type weights, and
//! recency preferences.
//!
//! # Example
//! ```
//! use pulsehive_core::lens::{Lens, ExperienceTypeTag};
//!
//! // A safety-focused agent
//! let mut lens = Lens::new(["safety", "clinical"]);
//! lens.type_weights.insert(ExperienceTypeTag::ErrorPattern, 2.0);
//! lens.type_weights.insert(ExperienceTypeTag::SuccessPattern, 0.5);
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Compact tag for experience types, mirroring PulseDB's 9 [`ExperienceType`] variants.
///
/// Used as a key in [`Lens::type_weights`] to control how strongly the agent
/// attends to each category of experience. Unlike PulseDB's `ExperienceType`,
/// this enum carries no inner data — it's purely a discriminant for weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExperienceTypeTag {
    Difficulty,
    Solution,
    ErrorPattern,
    SuccessPattern,
    UserPreference,
    ArchitecturalDecision,
    TechInsight,
    Fact,
    Generic,
}

impl ExperienceTypeTag {
    /// Returns all 9 variants in declaration order.
    pub fn all() -> &'static [Self] {
        &[
            Self::Difficulty,
            Self::Solution,
            Self::ErrorPattern,
            Self::SuccessPattern,
            Self::UserPreference,
            Self::ArchitecturalDecision,
            Self::TechInsight,
            Self::Fact,
            Self::Generic,
        ]
    }

    /// Maps a PulseDB `ExperienceType` to its compact tag for lens weighting.
    pub fn from_experience_type(et: &pulsedb::ExperienceType) -> Self {
        match et {
            pulsedb::ExperienceType::Difficulty { .. } => Self::Difficulty,
            pulsedb::ExperienceType::Solution { .. } => Self::Solution,
            pulsedb::ExperienceType::ErrorPattern { .. } => Self::ErrorPattern,
            pulsedb::ExperienceType::SuccessPattern { .. } => Self::SuccessPattern,
            pulsedb::ExperienceType::UserPreference { .. } => Self::UserPreference,
            pulsedb::ExperienceType::ArchitecturalDecision { .. } => Self::ArchitecturalDecision,
            pulsedb::ExperienceType::TechInsight { .. } => Self::TechInsight,
            pulsedb::ExperienceType::Fact { .. } => Self::Fact,
            pulsedb::ExperienceType::Generic { .. } => Self::Generic,
        }
    }
}

/// Time decay function controlling how recency affects perception.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecencyCurve {
    /// Recent experiences weighted heavily, old ones decay exponentially.
    ///
    /// Formula: `weight = 0.5^(age_hours / half_life_hours)`
    Exponential {
        /// Hours until an experience's weight drops to 50%.
        half_life_hours: f32,
    },
    /// All experiences equally weighted regardless of age.
    Uniform,
}

impl Default for RecencyCurve {
    fn default() -> Self {
        RecencyCurve::Exponential {
            half_life_hours: 72.0,
        }
    }
}

/// Perception filter that shapes how an agent sees the substrate.
///
/// Each agent has its own Lens, meaning different agents perceive the same
/// shared substrate differently based on their role and focus.
///
/// The perception pipeline:
/// 1. **Pre-search**: Query embedding is warped through the lens focus
/// 2. **Search**: PulseDB returns nearest neighbors via HNSW
/// 3. **Post-search**: Results re-ranked by domain, type weights, and temporal decay
/// 4. **Budget**: Top `attention_budget` results kept
#[derive(Debug, Clone)]
pub struct Lens {
    /// Domains this agent attends to (e.g., `["safety", "clinical"]`).
    ///
    /// Used for pre-search filtering and post-search domain relevance scoring.
    pub domain_focus: Vec<String>,

    /// Attention weights for different experience types.
    ///
    /// Higher weight = more attention. Default weight (when absent) is 1.0.
    pub type_weights: HashMap<ExperienceTypeTag, f32>,

    /// Time decay function controlling recency bias.
    pub recency_curve: RecencyCurve,

    /// Semantic focus embedding representing the agent's current purpose.
    ///
    /// Used to warp query embeddings before substrate search. Empty means no warping.
    pub purpose_embedding: Vec<f32>,

    /// Maximum number of experiences to perceive per cycle.
    pub attention_budget: usize,
}

impl Lens {
    /// Creates a lens focused on the given domains with sensible defaults.
    ///
    /// - Recency: Exponential decay with 72-hour half-life
    /// - Budget: 50 experiences
    /// - No type weights (all types equally weighted)
    /// - No purpose embedding (no query warping)
    ///
    /// # Example
    /// ```
    /// use pulsehive_core::lens::Lens;
    ///
    /// let lens = Lens::new(["safety", "clinical"]);
    /// assert_eq!(lens.domain_focus, vec!["safety", "clinical"]);
    /// assert_eq!(lens.attention_budget, 50);
    /// ```
    pub fn new(domains: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            domain_focus: domains.into_iter().map(Into::into).collect(),
            type_weights: HashMap::new(),
            recency_curve: RecencyCurve::default(),
            purpose_embedding: Vec::new(),
            attention_budget: 50,
        }
    }
}

impl Default for Lens {
    /// Creates a lens with no domain focus and sensible defaults.
    fn default() -> Self {
        Self::new(Vec::<String>::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lens_new_with_str_slices() {
        let lens = Lens::new(["safety", "clinical"]);
        assert_eq!(lens.domain_focus, vec!["safety", "clinical"]);
        assert_eq!(lens.attention_budget, 50);
        assert!(lens.type_weights.is_empty());
        assert!(lens.purpose_embedding.is_empty());
        assert!(matches!(
            lens.recency_curve,
            RecencyCurve::Exponential {
                half_life_hours
            } if (half_life_hours - 72.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn test_lens_new_with_owned_strings() {
        let domains = vec!["code".to_string(), "architecture".to_string()];
        let lens = Lens::new(domains);
        assert_eq!(lens.domain_focus, vec!["code", "architecture"]);
    }

    #[test]
    fn test_lens_default() {
        let lens = Lens::default();
        assert!(lens.domain_focus.is_empty());
        assert_eq!(lens.attention_budget, 50);
        assert!(matches!(
            lens.recency_curve,
            RecencyCurve::Exponential { half_life_hours } if (half_life_hours - 72.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn test_lens_clone() {
        let mut lens = Lens::new(["test"]);
        lens.type_weights
            .insert(ExperienceTypeTag::ErrorPattern, 2.0);
        let cloned = lens.clone();
        assert_eq!(cloned.domain_focus, lens.domain_focus);
        assert_eq!(
            cloned.type_weights.get(&ExperienceTypeTag::ErrorPattern),
            Some(&2.0)
        );
    }

    #[test]
    fn test_recency_curve_default() {
        let curve = RecencyCurve::default();
        assert!(matches!(
            curve,
            RecencyCurve::Exponential { half_life_hours } if (half_life_hours - 72.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn test_experience_type_tag_all_nine_variants() {
        let all = ExperienceTypeTag::all();
        assert_eq!(all.len(), 9);

        // Verify all are distinct via HashSet
        let set: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(set.len(), 9);
    }

    #[test]
    fn test_experience_type_tag_as_hashmap_key() {
        let mut weights = HashMap::new();
        weights.insert(ExperienceTypeTag::Difficulty, 1.5);
        weights.insert(ExperienceTypeTag::Solution, 2.0);
        weights.insert(ExperienceTypeTag::Generic, 0.5);

        assert_eq!(weights.get(&ExperienceTypeTag::Difficulty), Some(&1.5));
        assert_eq!(weights.get(&ExperienceTypeTag::Solution), Some(&2.0));
        assert_eq!(weights.get(&ExperienceTypeTag::Fact), None);
    }

    #[test]
    fn test_recency_curve_serialization() {
        let curve = RecencyCurve::Exponential {
            half_life_hours: 48.0,
        };
        let json = serde_json::to_string(&curve).unwrap();
        let deserialized: RecencyCurve = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            RecencyCurve::Exponential { half_life_hours } if (half_life_hours - 48.0).abs() < f32::EPSILON
        ));

        let uniform = RecencyCurve::Uniform;
        let json = serde_json::to_string(&uniform).unwrap();
        let deserialized: RecencyCurve = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, RecencyCurve::Uniform));
    }
}
