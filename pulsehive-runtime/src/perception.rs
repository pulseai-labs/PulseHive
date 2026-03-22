//! Lens-based perception pipeline for the agentic loop.
//!
//! Implements the Perceive phase: query substrate → re-rank through lens → format as
//! intrinsic knowledge. Each agent sees the same substrate differently based on its lens.

use pulsedb::{Activity, CollectiveId, Experience, SubstrateProvider, Timestamp};
use pulsehive_core::error::Result;
use pulsehive_core::lens::{ExperienceTypeTag, Lens, RecencyCurve};
use pulsehive_core::llm::Message;
use tracing::Instrument;

// ── Query Phase (#24) ────────────────────────────────────────────────

/// Query the substrate for perception candidates.
///
/// If `lens.purpose_embedding` is set, uses semantic search via `search_similar`.
/// Otherwise falls back to `get_recent` with domain post-filtering.
pub async fn query_substrate(
    substrate: &dyn SubstrateProvider,
    lens: &Lens,
    collective_id: CollectiveId,
) -> Result<(Vec<Experience>, Vec<Activity>)> {
    let fetch_limit = lens.attention_budget * 2; // Over-fetch for re-ranking headroom

    let experiences = if !lens.purpose_embedding.is_empty() {
        // Semantic search using the lens purpose embedding
        let results = substrate
            .search_similar(collective_id, &lens.purpose_embedding, fetch_limit)
            .await?;
        results.into_iter().map(|(exp, _sim)| exp).collect()
    } else {
        // Fallback: get recent experiences
        substrate.get_recent(collective_id, fetch_limit).await?
    };

    // Post-filter by domain if lens has domain focus
    let experiences = if lens.domain_focus.is_empty() {
        experiences
    } else {
        experiences
            .into_iter()
            .filter(|exp| {
                // Keep if any experience domain matches any lens domain
                exp.domain
                    .iter()
                    .any(|d| lens.domain_focus.iter().any(|ld| ld == d))
                    || exp.domain.is_empty() // Keep domain-less experiences
            })
            .collect()
    };

    // Fetch active agents for awareness
    let activities = substrate
        .get_activities(collective_id)
        .await
        .unwrap_or_default();

    Ok((experiences, activities))
}

// ── Re-Rank Phase (#25) ──────────────────────────────────────────────

/// Re-rank experiences through the lens using domain, type, and temporal weighting.
///
/// Returns experiences sorted by composite score (descending), truncated to
/// `lens.attention_budget`.
pub fn rerank(experiences: Vec<Experience>, lens: &Lens) -> Vec<(Experience, f32)> {
    let now = Timestamp::now();

    let mut scored: Vec<(Experience, f32)> = experiences
        .into_iter()
        .map(|exp| {
            let score = compute_score(&exp, lens, now);
            (exp, score)
        })
        .collect();

    // Sort by score descending
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Truncate to attention budget
    scored.truncate(lens.attention_budget);

    scored
}

/// Compute composite perception score for a single experience.
fn compute_score(exp: &Experience, lens: &Lens, now: Timestamp) -> f32 {
    let domain_weight = compute_domain_weight(exp, lens);
    let type_weight = compute_type_weight(exp, lens);
    let temporal_score = compute_temporal_score(exp, lens, now);

    domain_weight * type_weight * temporal_score
}

/// Domain relevance: 1.5x boost if experience domain overlaps lens focus.
fn compute_domain_weight(exp: &Experience, lens: &Lens) -> f32 {
    if lens.domain_focus.is_empty() {
        return 1.0;
    }
    let matches = exp
        .domain
        .iter()
        .any(|d| lens.domain_focus.iter().any(|ld| ld == d));
    if matches {
        1.5
    } else {
        1.0
    }
}

/// Type weight from lens configuration, default 1.0.
fn compute_type_weight(exp: &Experience, lens: &Lens) -> f32 {
    let tag = ExperienceTypeTag::from_experience_type(&exp.experience_type);
    *lens.type_weights.get(&tag).unwrap_or(&1.0)
}

/// Temporal decay + reinforcement based on recency curve.
fn compute_temporal_score(exp: &Experience, lens: &Lens, now: Timestamp) -> f32 {
    let age_hours = (now.0 - exp.timestamp.0) as f32 / (1000.0 * 3600.0);
    let age_hours = age_hours.max(0.0); // Guard against negative (clock skew)
    let reinforcement = 1.0 + (exp.applications as f32 * 0.1);

    match &lens.recency_curve {
        RecencyCurve::Exponential { half_life_hours } => {
            let decay = 0.5_f32.powf(age_hours / half_life_hours);
            exp.importance * decay * reinforcement
        }
        RecencyCurve::Uniform => exp.importance * reinforcement,
    }
}

// ── Format Phase (#26) ───────────────────────────────────────────────

/// Format perceived experiences as intrinsic knowledge messages.
///
/// Produces "You understand that..." format (not "Retrieved documents say...").
/// Knowledge is woven into the agent's identity as its own understanding.
pub fn format_as_intrinsic_knowledge(
    experiences: &[Experience],
    activities: &[Activity],
) -> Vec<Message> {
    if experiences.is_empty() && activities.is_empty() {
        return vec![];
    }

    let mut parts = Vec::new();

    if !experiences.is_empty() {
        parts.push("Based on your previous experience and knowledge:\n".to_string());
        for exp in experiences {
            // Truncate long content for context window efficiency
            let content = if exp.content.len() > 500 {
                format!("{}...", &exp.content[..500])
            } else {
                exp.content.clone()
            };
            parts.push(format!("• You understand that {content}"));
        }
    }

    if !activities.is_empty() {
        if !parts.is_empty() {
            parts.push(String::new()); // blank line separator
        }
        for activity in activities {
            let task_info = activity
                .current_task
                .as_deref()
                .unwrap_or("an unspecified task");
            parts.push(format!(
                "• You're aware that agent {} is working on {}",
                activity.agent_id, task_info
            ));
        }
    }

    vec![Message::system(parts.join("\n"))]
}

// ── Budget Packing (#32) ─────────────────────────────────────────────

/// Pack ranked experiences within the token budget.
///
/// Greedily selects experiences in score order until the token or count
/// budget is exhausted. Token estimation uses chars/4 + overhead.
pub fn pack_within_budget(
    ranked: Vec<(Experience, f32)>,
    budget: &pulsehive_core::context::ContextBudget,
) -> Vec<Experience> {
    use pulsehive_core::context::estimate_tokens;

    let mut packed = Vec::new();
    let mut tokens_used: u32 = 0;

    for (exp, _score) in ranked {
        if packed.len() >= budget.max_experiences {
            break;
        }
        let est = estimate_tokens(&exp.content);
        if tokens_used + est > budget.max_tokens {
            break;
        }
        tokens_used += est;
        packed.push(exp);
    }

    packed
}

// ── Full Assembly (#33) ──────────────────────────────────────────────

/// Assemble budget-aware context from the substrate through the lens.
///
/// Complete pipeline: query → re-rank → budget pack → format as intrinsic knowledge.
pub async fn assemble_context(
    substrate: &dyn SubstrateProvider,
    lens: &Lens,
    collective_id: CollectiveId,
    budget: &pulsehive_core::context::ContextBudget,
) -> Result<Vec<Message>> {
    let (candidates, activities) = query_substrate(substrate, lens, collective_id)
        .instrument(tracing::debug_span!("query_substrate",
            mode = if !lens.purpose_embedding.is_empty() { "semantic" } else { "recent" },
        ))
        .await?;
    tracing::debug!(candidate_count = candidates.len(), activity_count = activities.len(), "Substrate queried");
    let ranked = rerank(candidates, lens);
    let packed = pack_within_budget(ranked, budget);
    tracing::debug!(packed_count = packed.len(), "Context packed");
    Ok(format_as_intrinsic_knowledge(&packed, &activities))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsedb::{AgentId, ExperienceId, ExperienceType};
    use pulsehive_core::context::ContextBudget;
    use pulsehive_core::lens::ExperienceTypeTag;

    fn make_experience(
        content: &str,
        domain: Vec<&str>,
        importance: f32,
        exp_type: ExperienceType,
        age_hours: f32,
    ) -> Experience {
        let now_ms = Timestamp::now().0;
        let age_ms = (age_hours * 3600.0 * 1000.0) as i64;
        Experience {
            id: ExperienceId::new(),
            collective_id: CollectiveId::new(),
            content: content.into(),
            experience_type: exp_type,
            embedding: vec![],
            importance,
            confidence: 0.8,
            domain: domain.into_iter().map(String::from).collect(),
            related_files: vec![],
            source_agent: AgentId("test".into()),
            source_task: None,
            timestamp: Timestamp(now_ms - age_ms),
            archived: false,
            applications: 0,
        }
    }

    // ── Re-rank tests ────────────────────────────────────────────────

    #[test]
    fn test_rerank_domain_boost() {
        let experiences = vec![
            make_experience(
                "safety issue",
                vec!["safety"],
                0.5,
                ExperienceType::Generic { category: None },
                1.0,
            ),
            make_experience(
                "code pattern",
                vec!["code"],
                0.5,
                ExperienceType::Generic { category: None },
                1.0,
            ),
        ];

        let lens = Lens::new(["safety"]);
        let ranked = rerank(experiences, &lens);

        // Safety-domain experience should rank higher (1.5x boost)
        assert_eq!(ranked[0].0.content, "safety issue");
    }

    #[test]
    fn test_rerank_type_weight() {
        let experiences = vec![
            make_experience(
                "an error",
                vec![],
                0.5,
                ExperienceType::ErrorPattern {
                    signature: "err".into(),
                    fix: "".into(),
                    prevention: "".into(),
                },
                1.0,
            ),
            make_experience(
                "a fact",
                vec![],
                0.5,
                ExperienceType::Fact {
                    statement: "x".into(),
                    source: "y".into(),
                },
                1.0,
            ),
        ];

        let mut lens = Lens::default();
        lens.type_weights
            .insert(ExperienceTypeTag::ErrorPattern, 3.0);
        // Fact has default weight 1.0

        let ranked = rerank(experiences, &lens);
        assert_eq!(ranked[0].0.content, "an error"); // 3x type weight
    }

    #[test]
    fn test_rerank_temporal_decay() {
        let experiences = vec![
            make_experience(
                "old",
                vec![],
                0.8,
                ExperienceType::Generic { category: None },
                200.0, // 200 hours old
            ),
            make_experience(
                "recent",
                vec![],
                0.8,
                ExperienceType::Generic { category: None },
                1.0, // 1 hour old
            ),
        ];

        let lens = Lens::default(); // Exponential 72h half-life
        let ranked = rerank(experiences, &lens);
        assert_eq!(ranked[0].0.content, "recent"); // Recent decays less
    }

    #[test]
    fn test_rerank_truncates_to_budget() {
        let experiences: Vec<Experience> = (0..20)
            .map(|i| {
                make_experience(
                    &format!("exp {i}"),
                    vec![],
                    0.5,
                    ExperienceType::Generic { category: None },
                    i as f32,
                )
            })
            .collect();

        let lens = Lens {
            attention_budget: 5,
            ..Lens::default()
        };

        let ranked = rerank(experiences, &lens);
        assert_eq!(ranked.len(), 5);
    }

    #[test]
    fn test_rerank_uniform_curve() {
        let experiences = vec![
            make_experience(
                "old high importance",
                vec![],
                0.9,
                ExperienceType::Generic { category: None },
                500.0,
            ),
            make_experience(
                "recent low importance",
                vec![],
                0.3,
                ExperienceType::Generic { category: None },
                1.0,
            ),
        ];

        let lens = Lens {
            recency_curve: RecencyCurve::Uniform,
            ..Lens::default()
        };

        let ranked = rerank(experiences, &lens);
        // Uniform: no time decay, importance wins
        assert_eq!(ranked[0].0.content, "old high importance");
    }

    // ── Format tests ─────────────────────────────────────────────────

    #[test]
    fn test_format_empty_returns_empty() {
        let messages = format_as_intrinsic_knowledge(&[], &[]);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_format_experiences_as_intrinsic_knowledge() {
        let experiences = vec![make_experience(
            "Rust's ownership model prevents data races",
            vec!["rust"],
            0.8,
            ExperienceType::Generic { category: None },
            1.0,
        )];

        let messages = format_as_intrinsic_knowledge(&experiences, &[]);
        assert_eq!(messages.len(), 1);
        let content = match &messages[0] {
            Message::System { content } => content.clone(),
            _ => panic!("Expected System message"),
        };
        assert!(content.contains("You understand that"));
        assert!(content.contains("Rust's ownership model"));
    }

    #[test]
    fn test_format_with_activities() {
        let activities = vec![Activity {
            agent_id: "researcher".into(),
            collective_id: CollectiveId::new(),
            current_task: Some("analyzing codebase".into()),
            context_summary: None,
            started_at: Timestamp::now(),
            last_heartbeat: Timestamp::now(),
        }];

        let messages = format_as_intrinsic_knowledge(&[], &activities);
        assert_eq!(messages.len(), 1);
        let content = match &messages[0] {
            Message::System { content } => content.clone(),
            _ => panic!("Expected System message"),
        };
        assert!(content.contains("You're aware that"));
        assert!(content.contains("researcher"));
        assert!(content.contains("analyzing codebase"));
    }

    // ── Budget packing tests ─────────────────────────────────────────

    #[test]
    fn test_pack_within_token_budget() {
        // Each experience has ~100 chars content → ~25+20=45 tokens estimated
        let ranked: Vec<(Experience, f32)> = (0..10)
            .map(|i| {
                (
                    make_experience(
                        &"x".repeat(100),
                        vec![],
                        0.5,
                        ExperienceType::Generic { category: None },
                        i as f32,
                    ),
                    1.0 - (i as f32 * 0.1),
                )
            })
            .collect();

        let budget = ContextBudget {
            max_tokens: 200, // ~4-5 experiences worth
            max_experiences: 50,
            max_insights: 10,
        };

        let packed = pack_within_budget(ranked, &budget);
        assert!(
            packed.len() < 10,
            "Should have been limited by token budget"
        );
        assert!(!packed.is_empty());
    }

    #[test]
    fn test_pack_within_experience_budget() {
        let ranked: Vec<(Experience, f32)> = (0..10)
            .map(|i| {
                (
                    make_experience(
                        "short",
                        vec![],
                        0.5,
                        ExperienceType::Generic { category: None },
                        i as f32,
                    ),
                    1.0,
                )
            })
            .collect();

        let budget = ContextBudget {
            max_tokens: 100_000, // Unlimited tokens
            max_experiences: 3,  // But only 3 experiences
            max_insights: 10,
        };

        let packed = pack_within_budget(ranked, &budget);
        assert_eq!(packed.len(), 3);
    }

    #[test]
    fn test_pack_empty_input() {
        let budget = ContextBudget::default();
        let packed = pack_within_budget(vec![], &budget);
        assert!(packed.is_empty());
    }
}
