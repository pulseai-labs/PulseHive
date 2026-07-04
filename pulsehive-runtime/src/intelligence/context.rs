//! Context optimization with temporal decay and insights-first priority ordering.
//!
//! The [`ContextOptimizer`] computes decayed importance for experiences and assembles
//! context with the priority: insights > high-importance experiences > recent experiences.
//! This ensures agents receive the most relevant, consolidated knowledge.

use pulsedb::{Activity, DerivedInsight, Experience, Timestamp};
use pulsehive_core::context::{estimate_tokens, ContextBudget};
use pulsehive_core::llm::Message;

/// Configuration for the context optimizer.
#[derive(Debug, Clone)]
pub struct ContextOptimizerConfig {
    /// Half-life for exponential decay in hours.
    /// After this many hours, an experience's importance decays to 50%.
    /// Default: 72.0 (3 days)
    pub decay_half_life_hours: f32,

    /// Boost per application/reinforcement.
    /// Each time an experience is applied, its effective importance
    /// increases by this factor (multiplicative).
    /// Default: 0.1 (10% per application)
    pub reinforcement_boost: f32,
}

impl Default for ContextOptimizerConfig {
    fn default() -> Self {
        Self {
            decay_half_life_hours: 72.0,
            reinforcement_boost: 0.1,
        }
    }
}

/// Optimizes context assembly with temporal decay and insights-first priority.
///
/// Implements FR-020: decayed importance formula with configurable half-life
/// and reinforcement boost. Context is assembled in priority order:
/// 1. Insights (consolidated knowledge from clusters)
/// 2. High-importance experiences (after decay)
/// 3. Recent experiences (by timestamp)
pub struct ContextOptimizer {
    config: ContextOptimizerConfig,
}

impl ContextOptimizer {
    /// Create with the given configuration.
    pub fn new(config: ContextOptimizerConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(ContextOptimizerConfig::default())
    }

    /// Access the configuration.
    pub fn config(&self) -> &ContextOptimizerConfig {
        &self.config
    }

    /// Compute decayed importance for an experience.
    ///
    /// Formula (FR-020):
    /// `importance * 0.5^(elapsed_hours / half_life) * (1 + applications * reinforcement_boost)`
    pub fn compute_decayed_importance(&self, experience: &Experience, now: Timestamp) -> f32 {
        let age_hours = (now.0 - experience.timestamp.0) as f32 / (1000.0 * 3600.0);
        let age_hours = age_hours.max(0.0);

        let decay = 0.5_f32.powf(age_hours / self.config.decay_half_life_hours);
        let reinforcement =
            1.0 + (experience.applications() as f32 * self.config.reinforcement_boost);

        experience.importance * decay * reinforcement
    }

    /// Assemble context with insights-first priority ordering.
    ///
    /// Priority: insights > high-importance experiences > recent.
    /// Packs within the given budget and formats as intrinsic knowledge.
    pub fn assemble_prioritized(
        &self,
        experiences: Vec<Experience>,
        insights: Vec<DerivedInsight>,
        activities: Vec<Activity>,
        budget: &ContextBudget,
        now: Timestamp,
    ) -> Vec<Message> {
        let mut parts = Vec::new();
        let mut token_count: u32 = 0;

        // 1. INSIGHTS FIRST (always prioritized)
        if !insights.is_empty() {
            let insight_limit = budget.max_insights.min(insights.len());
            let mut insight_lines = Vec::new();
            for insight in insights.iter().take(insight_limit) {
                let tokens = estimate_tokens(&insight.content);
                if token_count + tokens > budget.max_tokens {
                    break;
                }
                insight_lines.push(format!("- {}", insight.content));
                token_count += tokens;
            }
            if !insight_lines.is_empty() {
                parts.push(format!(
                    "Key insights you've synthesized:\n{}",
                    insight_lines.join("\n")
                ));
            }
        }

        // 2. EXPERIENCES sorted by decayed importance
        if !experiences.is_empty() {
            let mut scored: Vec<(Experience, f32)> = experiences
                .into_iter()
                .map(|exp| {
                    let score = self.compute_decayed_importance(&exp, now);
                    (exp, score)
                })
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let mut exp_lines = Vec::new();
            let exp_limit = budget.max_experiences;
            for (exp, _score) in scored.into_iter().take(exp_limit) {
                let tokens = estimate_tokens(&exp.content);
                if token_count + tokens > budget.max_tokens {
                    break;
                }
                exp_lines.push(format!("- You understand that {}", exp.content));
                token_count += tokens;
            }
            if !exp_lines.is_empty() {
                parts.push(format!(
                    "Based on your experience and knowledge:\n{}",
                    exp_lines.join("\n")
                ));
            }
        }

        // 3. ACTIVITY AWARENESS
        if !activities.is_empty() {
            let activity_lines: Vec<String> = activities
                .iter()
                .filter_map(|a| {
                    a.current_task.as_ref().map(|task| {
                        format!(
                            "- You're aware that agent {} is working on: {}",
                            a.agent_id, task
                        )
                    })
                })
                .collect();
            if !activity_lines.is_empty() {
                parts.push(format!(
                    "Current team activity:\n{}",
                    activity_lines.join("\n")
                ));
            }
        }

        if parts.is_empty() {
            return vec![];
        }

        vec![Message::system(parts.join("\n\n"))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn make_experience(importance: f32, age_hours: f32, applications: u32) -> Experience {
        let now_ms = 1_700_000_000_000_i64;
        let age_ms = (age_hours * 3600.0 * 1000.0) as i64;
        Experience {
            id: pulsedb::ExperienceId::new(),
            collective_id: pulsedb::CollectiveId::new(),
            content: format!("Experience with importance {importance}"),
            embedding: vec![],
            experience_type: pulsedb::ExperienceType::Generic { category: None },
            importance,
            confidence: 0.8,
            applications: BTreeMap::from([(pulsedb::InstanceId::new(), applications)]),
            last_reinforced: Timestamp(now_ms - age_ms),
            domain: vec![],
            related_files: vec![],
            source_agent: pulsedb::AgentId("test".into()),
            source_task: None,
            timestamp: Timestamp(now_ms - age_ms),
            archived: false,
        }
    }

    #[test]
    fn test_72h_decay_to_50_percent() {
        let opt = ContextOptimizer::with_defaults();
        let now = Timestamp(1_700_000_000_000);
        let exp = make_experience(1.0, 72.0, 0);
        let decayed = opt.compute_decayed_importance(&exp, now);
        assert!(
            (decayed - 0.5).abs() < 0.01,
            "72h decay should be ~0.5, got {decayed}"
        );
    }

    #[test]
    fn test_zero_age_full_importance() {
        let opt = ContextOptimizer::with_defaults();
        let now = Timestamp(1_700_000_000_000);
        let exp = make_experience(0.8, 0.0, 0);
        let decayed = opt.compute_decayed_importance(&exp, now);
        assert!(
            (decayed - 0.8).abs() < 0.01,
            "Zero age should be full importance, got {decayed}"
        );
    }

    #[test]
    fn test_reinforcement_boost() {
        let opt = ContextOptimizer::with_defaults();
        let now = Timestamp(1_700_000_000_000);
        let exp = make_experience(1.0, 0.0, 5); // 5 applications → 1.5x
        let decayed = opt.compute_decayed_importance(&exp, now);
        assert!(
            (decayed - 1.5).abs() < 0.01,
            "5 applications should give 1.5x, got {decayed}"
        );
    }

    #[test]
    fn test_config_defaults() {
        let config = ContextOptimizerConfig::default();
        assert!((config.decay_half_life_hours - 72.0).abs() < f32::EPSILON);
        assert!((config.reinforcement_boost - 0.1).abs() < f32::EPSILON);
    }
}
