//! Automatic relationship inference between experiences.
//!
//! When a new experience is recorded, the [`RelationshipDetector`] searches for
//! semantically similar experiences and creates typed relations based on
//! ExperienceType pair heuristics (e.g., Difficulty + Solution → Supports).

use pulsedb::{Experience, NewExperienceRelation, RelationType, SubstrateProvider};
use tracing::Instrument;

/// Configuration for automatic relationship detection.
#[derive(Debug, Clone)]
pub struct RelationshipDetectorConfig {
    /// Similarity threshold for automatic relation creation.
    /// Pairs above this threshold get relations created automatically.
    /// Default: 0.85
    pub auto_threshold: f32,

    /// Lower bound for suggested relations (used with LLM classification).
    /// Pairs between suggest_threshold and auto_threshold may be classified by LLM.
    /// Default: 0.65
    pub suggest_threshold: f32,

    /// Whether to use LLM classification for pairs in the suggest range.
    /// Default: false
    pub use_llm_classification: bool,
}

impl Default for RelationshipDetectorConfig {
    fn default() -> Self {
        Self {
            auto_threshold: 0.85,
            suggest_threshold: 0.65,
            use_llm_classification: false,
        }
    }
}

/// Detects relationships between experiences based on semantic similarity
/// and ExperienceType heuristics.
///
/// Created via [`RelationshipDetector::new()`] with a [`RelationshipDetectorConfig`].
pub struct RelationshipDetector {
    config: RelationshipDetectorConfig,
}

impl RelationshipDetector {
    /// Create a new detector with the given configuration.
    pub fn new(config: RelationshipDetectorConfig) -> Self {
        Self { config }
    }

    /// Create a new detector with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(RelationshipDetectorConfig::default())
    }

    /// Access the configuration.
    pub fn config(&self) -> &RelationshipDetectorConfig {
        &self.config
    }

    /// Find semantically similar experiences and create relations for high-similarity pairs.
    ///
    /// Searches for the top 20 similar experiences in the same collective. For each pair
    /// with similarity above `auto_threshold`, creates a [`NewExperienceRelation`] with
    /// the similarity score as strength.
    ///
    /// Returns the relations to be stored — the caller is responsible for calling
    /// `substrate.store_relation()` and emitting events.
    pub async fn infer_relations(
        &self,
        experience: &Experience,
        substrate: &dyn SubstrateProvider,
    ) -> Vec<NewExperienceRelation> {
        // Search for top-20 similar experiences
        let similar = match substrate
            .search_similar(experience.collective_id, &experience.embedding, 20)
            .instrument(tracing::debug_span!("infer_relations", experience_id = %experience.id))
            .await
        {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!(error = %e, "RelationshipDetector: search_similar failed");
                return Vec::new();
            }
        };

        similar
            .into_iter()
            .filter(|(target, similarity)| {
                // Exclude self-matches and below-threshold pairs
                target.id != experience.id && *similarity >= self.config.auto_threshold
            })
            .map(|(target, similarity)| {
                let relation_type =
                    classify_relation_type(&experience.experience_type, &target.experience_type);

                NewExperienceRelation {
                    source_id: experience.id,
                    target_id: target.id,
                    relation_type,
                    strength: similarity,
                    metadata: None,
                }
            })
            .collect()
    }
}

/// Classify the relation type based on ExperienceType pair heuristics.
///
/// Rules (from SRS FR-018):
/// - Difficulty + Solution → Supports
/// - ErrorPattern + ErrorPattern → Supersedes
/// - ArchitecturalDecision + TechInsight → Implies
/// - Default → RelatedTo
fn classify_relation_type(
    source: &pulsedb::ExperienceType,
    target: &pulsedb::ExperienceType,
) -> RelationType {
    use pulsedb::ExperienceType;

    match (source, target) {
        (ExperienceType::Difficulty { .. }, ExperienceType::Solution { .. })
        | (ExperienceType::Solution { .. }, ExperienceType::Difficulty { .. }) => {
            RelationType::Supports
        }
        (ExperienceType::ErrorPattern { .. }, ExperienceType::ErrorPattern { .. }) => {
            RelationType::Supersedes
        }
        (ExperienceType::ArchitecturalDecision { .. }, ExperienceType::TechInsight { .. })
        | (ExperienceType::TechInsight { .. }, ExperienceType::ArchitecturalDecision { .. }) => {
            RelationType::Implies
        }
        _ => RelationType::RelatedTo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsedb::{ExperienceType, RelationType, Severity};

    #[test]
    fn test_classify_difficulty_solution_supports() {
        let source = ExperienceType::Difficulty {
            description: "network timeout".into(),
            severity: Severity::Medium,
        };
        let target = ExperienceType::Solution {
            problem_ref: None,
            approach: "add retry".into(),
            worked: true,
        };
        assert_eq!(
            classify_relation_type(&source, &target),
            RelationType::Supports
        );
        assert_eq!(
            classify_relation_type(&target, &source),
            RelationType::Supports
        );
    }

    #[test]
    fn test_classify_error_error_supersedes() {
        let source = ExperienceType::ErrorPattern {
            signature: "timeout".into(),
            fix: "retry".into(),
            prevention: "set timeout".into(),
        };
        let target = ExperienceType::ErrorPattern {
            signature: "timeout_v2".into(),
            fix: "circuit breaker".into(),
            prevention: "backoff".into(),
        };
        assert_eq!(
            classify_relation_type(&source, &target),
            RelationType::Supersedes
        );
    }

    #[test]
    fn test_classify_decision_insight_implies() {
        let source = ExperienceType::ArchitecturalDecision {
            decision: "use circuit breaker".into(),
            rationale: "resilience".into(),
        };
        let target = ExperienceType::TechInsight {
            technology: "tokio".into(),
            insight: "spawn_blocking for CPU".into(),
        };
        assert_eq!(
            classify_relation_type(&source, &target),
            RelationType::Implies
        );
        assert_eq!(
            classify_relation_type(&target, &source),
            RelationType::Implies
        );
    }

    #[test]
    fn test_classify_default_related_to() {
        let source = ExperienceType::Generic { category: None };
        let target = ExperienceType::Generic { category: None };
        assert_eq!(
            classify_relation_type(&source, &target),
            RelationType::RelatedTo
        );
    }

    #[test]
    fn test_config_defaults() {
        let config = RelationshipDetectorConfig::default();
        assert!((config.auto_threshold - 0.85).abs() < f32::EPSILON);
        assert!((config.suggest_threshold - 0.65).abs() < f32::EPSILON);
        assert!(!config.use_llm_classification);
    }
}
