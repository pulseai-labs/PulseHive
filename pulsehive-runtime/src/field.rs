//! Field dynamics — attractor-based perception warping.
//!
//! [`AttractorDynamics`] models how high-importance experiences pull nearby queries
//! toward themselves, creating "gravitational wells" in embedding space. This enables
//! strong knowledge patterns to attract agent attention proportional to their strength.
//!
//! # Example
//! ```rust,ignore
//! let config = AttractorConfig::default();
//! let attractor = AttractorDynamics::from_experience(&experience, &config);
//! let influence = attractor.influence_at(&query_embedding, &experience.embedding);
//! ```

use pulsedb::{Experience, ExperienceId};

/// Configuration for attractor dynamics computation.
#[derive(Debug, Clone)]
pub struct AttractorConfig {
    /// Default influence radius in embedding space (cosine distance).
    /// Experiences beyond this radius have zero attractor influence.
    pub default_radius: f32,
    /// How strongly attractors pull nearby queries.
    /// Higher values make high-strength experiences more dominant.
    pub default_warp_factor: f32,
    /// Boost per application count when computing strength.
    /// `strength = importance * confidence * (1 + applications * boost)`
    pub reinforcement_boost: f32,
}

impl Default for AttractorConfig {
    fn default() -> Self {
        Self {
            default_radius: 0.3,
            default_warp_factor: 1.0,
            reinforcement_boost: 0.1,
        }
    }
}

/// Attractor dynamics for a single experience — its gravitational properties
/// in embedding space.
///
/// Computed at query time from experience fields (not pre-stored).
#[derive(Debug, Clone)]
pub struct AttractorDynamics {
    /// Experience this attractor represents.
    pub experience_id: ExperienceId,
    /// Combined strength: `importance * confidence * reinforcement`.
    pub strength: f32,
    /// Influence radius in embedding space (cosine distance).
    pub radius: f32,
    /// How strongly this attractor pulls nearby queries.
    pub warp_factor: f32,
}

impl AttractorDynamics {
    /// Compute attractor dynamics from an experience's fields.
    ///
    /// Strength formula: `importance * confidence * (1 + applications * reinforcement_boost)`
    pub fn from_experience(exp: &Experience, config: &AttractorConfig) -> Self {
        let reinforcement = 1.0 + (exp.applications as f32 * config.reinforcement_boost);
        Self {
            experience_id: exp.id,
            strength: exp.importance * exp.confidence * reinforcement,
            radius: config.default_radius,
            warp_factor: config.default_warp_factor,
        }
    }

    /// Compute the influence of this attractor on a query at the given position.
    ///
    /// Returns 0.0 if the query is beyond the attractor's radius.
    /// Returns `strength * warp_factor` at distance 0.0.
    /// Linear falloff between 0 and radius.
    pub fn influence_at(&self, query_embedding: &[f32], experience_embedding: &[f32]) -> f32 {
        if query_embedding.is_empty() || experience_embedding.is_empty() {
            return 0.0;
        }
        let distance = cosine_distance(query_embedding, experience_embedding);
        if distance > self.radius {
            return 0.0;
        }
        self.strength * (1.0 - distance / self.radius) * self.warp_factor
    }
}

/// Compute cosine distance between two vectors: `1.0 - cosine_similarity`.
///
/// Returns 1.0 (maximum distance) for empty, zero-length, or orthogonal vectors.
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 1.0;
    }

    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;

    for (ai, bi) in a.iter().zip(b.iter()) {
        dot += ai * bi;
        norm_a += ai * ai;
        norm_b += bi * bi;
    }

    let denominator = norm_a.sqrt() * norm_b.sqrt();
    if denominator < f32::EPSILON {
        return 1.0;
    }

    let similarity = (dot / denominator).clamp(-1.0, 1.0);
    1.0 - similarity
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsedb::{AgentId, CollectiveId, ExperienceType, Timestamp};

    fn mock_experience(importance: f32, confidence: f32, applications: u32) -> Experience {
        Experience {
            id: ExperienceId::new(),
            collective_id: CollectiveId::new(),
            content: "test".to_string(),
            experience_type: ExperienceType::Generic { category: None },
            embedding: vec![1.0, 0.0, 0.0],
            importance,
            confidence,
            applications,
            domain: vec![],
            source_agent: AgentId("test".to_string()),
            source_task: None,
            related_files: vec![],
            timestamp: Timestamp::now(),
            archived: false,
        }
    }

    // ── cosine_distance tests ─────────────────────────────────────────

    #[test]
    fn test_cosine_distance_identical_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let dist = cosine_distance(&a, &a);
        assert!(
            dist.abs() < 0.001,
            "Identical vectors should have distance ~0, got {dist}"
        );
    }

    #[test]
    fn test_cosine_distance_orthogonal_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let dist = cosine_distance(&a, &b);
        assert!(
            (dist - 1.0).abs() < 0.001,
            "Orthogonal vectors should have distance ~1, got {dist}"
        );
    }

    #[test]
    fn test_cosine_distance_opposite_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let dist = cosine_distance(&a, &b);
        assert!(
            (dist - 2.0).abs() < 0.001,
            "Opposite vectors should have distance ~2, got {dist}"
        );
    }

    #[test]
    fn test_cosine_distance_empty_vectors() {
        assert_eq!(cosine_distance(&[], &[1.0]), 1.0);
        assert_eq!(cosine_distance(&[1.0], &[]), 1.0);
        assert_eq!(cosine_distance(&[], &[]), 1.0);
    }

    #[test]
    fn test_cosine_distance_mismatched_lengths() {
        assert_eq!(cosine_distance(&[1.0], &[1.0, 2.0]), 1.0);
    }

    #[test]
    fn test_cosine_distance_zero_vectors() {
        let zero = vec![0.0, 0.0, 0.0];
        let dist = cosine_distance(&zero, &[1.0, 0.0, 0.0]);
        assert_eq!(dist, 1.0);
    }

    // ── AttractorDynamics tests ──────────────────────────────────────

    #[test]
    fn test_from_experience_strength_formula() {
        let config = AttractorConfig {
            reinforcement_boost: 0.1,
            ..Default::default()
        };
        let exp = mock_experience(0.8, 0.9, 5);
        let attractor = AttractorDynamics::from_experience(&exp, &config);

        // strength = 0.8 * 0.9 * (1 + 5 * 0.1) = 0.72 * 1.5 = 1.08
        assert!(
            (attractor.strength - 1.08).abs() < 0.001,
            "Expected strength ~1.08, got {}",
            attractor.strength
        );
    }

    #[test]
    fn test_from_experience_zero_applications() {
        let config = AttractorConfig::default();
        let exp = mock_experience(0.5, 0.5, 0);
        let attractor = AttractorDynamics::from_experience(&exp, &config);
        // strength = 0.5 * 0.5 * 1.0 = 0.25
        assert!(
            (attractor.strength - 0.25).abs() < 0.001,
            "Expected strength ~0.25, got {}",
            attractor.strength
        );
    }

    #[test]
    fn test_influence_at_zero_distance() {
        let config = AttractorConfig::default();
        let exp = mock_experience(1.0, 1.0, 0);
        let attractor = AttractorDynamics::from_experience(&exp, &config);

        let emb = vec![1.0, 0.0, 0.0];
        let influence = attractor.influence_at(&emb, &emb);
        // distance = 0, influence = strength * (1 - 0/radius) * warp = 1.0 * 1.0 * 1.0 = 1.0
        assert!(
            (influence - 1.0).abs() < 0.001,
            "Expected influence ~1.0 at zero distance, got {influence}"
        );
    }

    #[test]
    fn test_influence_at_beyond_radius() {
        let config = AttractorConfig {
            default_radius: 0.1,
            ..Default::default()
        };
        let exp = mock_experience(1.0, 1.0, 0);
        let attractor = AttractorDynamics::from_experience(&exp, &config);

        let q = vec![1.0, 0.0, 0.0];
        let e = vec![0.0, 1.0, 0.0]; // orthogonal = distance 1.0
        assert_eq!(
            attractor.influence_at(&q, &e),
            0.0,
            "Beyond radius should return 0"
        );
    }

    #[test]
    fn test_influence_at_empty_embedding() {
        let config = AttractorConfig::default();
        let exp = mock_experience(1.0, 1.0, 0);
        let attractor = AttractorDynamics::from_experience(&exp, &config);

        assert_eq!(attractor.influence_at(&[], &[1.0]), 0.0);
        assert_eq!(attractor.influence_at(&[1.0], &[]), 0.0);
    }

    #[test]
    fn test_influence_linear_falloff() {
        let config = AttractorConfig {
            default_radius: 1.0,
            default_warp_factor: 1.0,
            reinforcement_boost: 0.0,
        };
        let exp = mock_experience(1.0, 1.0, 0);
        let attractor = AttractorDynamics::from_experience(&exp, &config);

        // At distance 0.5 from center, influence should be ~0.5
        // Use vectors that produce cosine distance ~0.5
        let q = vec![1.0, 0.0];
        let e = vec![0.707, 0.707]; // ~45 degrees, cosine_sim ~0.707, distance ~0.293
        let influence = attractor.influence_at(&q, &e);
        // Influence at distance 0.293 with radius 1.0 = 1.0 * (1 - 0.293) * 1.0 ≈ 0.707
        assert!(
            influence > 0.5 && influence < 1.0,
            "Expected partial influence, got {influence}"
        );
    }

    #[test]
    fn test_warp_factor_scales_influence() {
        let config_low = AttractorConfig {
            default_warp_factor: 0.5,
            ..Default::default()
        };
        let config_high = AttractorConfig {
            default_warp_factor: 2.0,
            ..Default::default()
        };

        let exp = mock_experience(1.0, 1.0, 0);
        let a_low = AttractorDynamics::from_experience(&exp, &config_low);
        let a_high = AttractorDynamics::from_experience(&exp, &config_high);

        let emb = vec![1.0, 0.0, 0.0];
        let inf_low = a_low.influence_at(&emb, &emb);
        let inf_high = a_high.influence_at(&emb, &emb);

        assert!(
            inf_high > inf_low,
            "Higher warp_factor should produce stronger influence: {inf_high} vs {inf_low}"
        );
        assert!(
            (inf_high / inf_low - 4.0).abs() < 0.001,
            "Should scale by 4x"
        );
    }
}
