//! Automatic insight synthesis from experience clusters.
//!
//! When a cluster of related experiences exceeds the density threshold,
//! the [`InsightSynthesizer`] uses an LLM to generate a consolidated
//! [`DerivedInsight`](pulsedb::DerivedInsight) that captures the key pattern.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::Instant;

use pulsedb::{
    CollectiveId, Experience, ExperienceId, InsightType, NewDerivedInsight, SubstrateProvider,
};
use pulsehive_core::llm::{LlmConfig, LlmProvider, Message};
use tracing::Instrument;

/// Configuration for automatic insight synthesis.
#[derive(Debug, Clone)]
pub struct InsightSynthesizerConfig {
    /// Minimum cluster size to trigger synthesis.
    /// Default: 5
    pub relation_density_threshold: usize,

    /// Minimum seconds between synthesis attempts for the same collective.
    /// Prevents redundant LLM calls when many experiences arrive rapidly.
    /// Default: 60
    pub debounce_seconds: u64,
}

impl Default for InsightSynthesizerConfig {
    fn default() -> Self {
        Self {
            relation_density_threshold: 5,
            debounce_seconds: 60,
        }
    }
}

/// Synthesizes insights from clusters of related experiences using an LLM.
///
/// Created via [`InsightSynthesizer::new()`] with an [`InsightSynthesizerConfig`].
pub struct InsightSynthesizer {
    config: InsightSynthesizerConfig,
    /// Tracks last synthesis time per collective for debouncing.
    last_synthesis: Mutex<HashMap<CollectiveId, Instant>>,
}

impl InsightSynthesizer {
    /// Create a new synthesizer with the given configuration.
    pub fn new(config: InsightSynthesizerConfig) -> Self {
        Self {
            config,
            last_synthesis: Mutex::new(HashMap::new()),
        }
    }

    /// Create a new synthesizer with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(InsightSynthesizerConfig::default())
    }

    /// Access the configuration.
    pub fn config(&self) -> &InsightSynthesizerConfig {
        &self.config
    }

    /// Check if synthesis should be attempted based on cluster size.
    pub fn should_synthesize(&self, cluster_size: usize) -> bool {
        cluster_size >= self.config.relation_density_threshold
    }

    /// Check if a collective is still within the debounce window.
    pub fn is_debounced(&self, collective_id: CollectiveId) -> bool {
        let guard = self.last_synthesis.lock().unwrap();
        if let Some(last) = guard.get(&collective_id) {
            last.elapsed().as_secs() < self.config.debounce_seconds
        } else {
            false
        }
    }

    /// Record that synthesis was performed for a collective (updates debounce timer).
    pub fn mark_synthesized(&self, collective_id: CollectiveId) {
        let mut guard = self.last_synthesis.lock().unwrap();
        guard.insert(collective_id, Instant::now());
    }

    /// Find all experiences connected to `start_id` via relations (BFS traversal).
    ///
    /// Traverses the relation graph starting from the given experience,
    /// collecting all reachable experiences. Capped at 50 to prevent
    /// runaway traversal on dense graphs.
    pub async fn find_cluster(
        &self,
        start_id: ExperienceId,
        substrate: &dyn SubstrateProvider,
    ) -> Vec<Experience> {
        const MAX_CLUSTER_SIZE: usize = 50;

        let mut visited: HashSet<ExperienceId> = HashSet::new();
        let mut queue: VecDeque<ExperienceId> = VecDeque::new();
        let mut cluster: Vec<Experience> = Vec::new();

        queue.push_back(start_id);
        visited.insert(start_id);

        while let Some(exp_id) = queue.pop_front() {
            if cluster.len() >= MAX_CLUSTER_SIZE {
                break;
            }

            // Get all related experiences
            let related = match substrate.get_related(exp_id).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "InsightSynthesizer: get_related failed");
                    continue;
                }
            };

            for (experience, _relation) in related {
                if !visited.contains(&experience.id) {
                    visited.insert(experience.id);
                    queue.push_back(experience.id);
                    cluster.push(experience);
                }
            }
        }

        tracing::debug!(cluster_size = cluster.len(), experience_id = %start_id, "Cluster found");
        cluster
    }

    /// Synthesize a cluster of related experiences into a consolidated insight using an LLM.
    ///
    /// Builds a prompt from experience contents, calls the LLM, and returns a
    /// `NewDerivedInsight` ready to store. Returns `None` if synthesis fails.
    pub async fn synthesize_cluster(
        &self,
        cluster: &[Experience],
        collective_id: CollectiveId,
        provider: &dyn LlmProvider,
        llm_config: &LlmConfig,
    ) -> Option<NewDerivedInsight> {
        if cluster.is_empty() {
            return None;
        }

        // Build synthesis prompt from cluster
        let mut experience_list = String::new();
        for (i, exp) in cluster.iter().enumerate() {
            experience_list.push_str(&format!(
                "{}. [{}] {}\n",
                i + 1,
                format!("{:?}", exp.experience_type)
                    .split('{')
                    .next()
                    .unwrap_or("Unknown")
                    .trim(),
                exp.content
            ));
        }

        let prompt = format!(
            "You are analyzing a cluster of {} related experiences from an AI agent system. \
             Synthesize them into a single concise insight (2-3 sentences) that captures \
             the key pattern or learning.\n\nExperiences:\n{}",
            cluster.len(),
            experience_list
        );

        let messages = vec![
            Message::system(
                "You are a knowledge synthesis expert. Provide concise, actionable insights.",
            ),
            Message::user(&prompt),
        ];

        // Call LLM for synthesis
        let response = match provider
            .chat(messages, vec![], llm_config)
            .instrument(tracing::debug_span!(
                "synthesize_insight",
                cluster_size = cluster.len()
            ))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "InsightSynthesizer: LLM call failed");
                return None;
            }
        };

        let content = response.content.unwrap_or_default();
        if content.is_empty() {
            return None;
        }

        // Compute average confidence from sources
        let avg_confidence = if cluster.is_empty() {
            0.5
        } else {
            cluster.iter().map(|e| e.confidence).sum::<f32>() / cluster.len() as f32
        };

        // Collect unique domains from all sources
        let domains: Vec<String> = cluster
            .iter()
            .flat_map(|e| e.domain.iter().cloned())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        Some(NewDerivedInsight {
            collective_id,
            content,
            embedding: None, // PulseDB builtin embeddings compute this
            source_experience_ids: cluster.iter().map(|e| e.id).collect(),
            insight_type: InsightType::Synthesis,
            confidence: avg_confidence,
            domain: domains,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_synthesize_below_threshold() {
        let synth = InsightSynthesizer::with_defaults(); // threshold = 5
        assert!(!synth.should_synthesize(3));
        assert!(!synth.should_synthesize(4));
    }

    #[test]
    fn test_should_synthesize_at_threshold() {
        let synth = InsightSynthesizer::with_defaults();
        assert!(synth.should_synthesize(5));
        assert!(synth.should_synthesize(10));
    }

    #[test]
    fn test_debounce_blocks_immediate_retry() {
        let synth = InsightSynthesizer::with_defaults(); // debounce = 60s
        let cid = CollectiveId::new();

        assert!(
            !synth.is_debounced(cid),
            "Should not be debounced initially"
        );

        synth.mark_synthesized(cid);
        assert!(synth.is_debounced(cid), "Should be debounced after marking");
    }

    #[test]
    fn test_debounce_allows_different_collective() {
        let synth = InsightSynthesizer::with_defaults();
        let cid_a = CollectiveId::new();
        let cid_b = CollectiveId::new();

        synth.mark_synthesized(cid_a);
        assert!(synth.is_debounced(cid_a));
        assert!(
            !synth.is_debounced(cid_b),
            "Different collective should not be debounced"
        );
    }

    #[test]
    fn test_config_defaults() {
        let config = InsightSynthesizerConfig::default();
        assert_eq!(config.relation_density_threshold, 5);
        assert_eq!(config.debounce_seconds, 60);
    }

    #[test]
    fn test_zero_debounce_never_blocks() {
        let synth = InsightSynthesizer::new(InsightSynthesizerConfig {
            relation_density_threshold: 5,
            debounce_seconds: 0,
        });
        let cid = CollectiveId::new();

        synth.mark_synthesized(cid);
        // With 0s debounce, should not be debounced
        assert!(!synth.is_debounced(cid));
    }
}
