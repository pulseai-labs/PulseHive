//! Default experience extraction from agent conversations.
//!
//! [`DefaultExperienceExtractor`] provides simple rule-based extraction
//! for the Record phase. Products can implement [`ExperienceExtractor`]
//! for custom logic (e.g., LLM-based summarization).

use async_trait::async_trait;

use pulsedb::{AgentId, ExperienceType, NewExperience, Severity};
use pulsehive_core::agent::{AgentOutcome, ExperienceExtractor, ExtractionContext};
use pulsehive_core::llm::Message;

/// Default experience extractor using simple rule-based logic.
///
/// Extraction rules:
/// - `Complete` → `Generic` experience with the agent's final response
/// - `Error` → `ErrorPattern` with the error description
/// - `MaxIterationsReached` → `Difficulty` noting the iteration limit
///
/// For richer extraction (e.g., extracting patterns, decisions, insights
/// from the full conversation), implement a custom [`ExperienceExtractor`].
#[derive(Debug, Clone, Default)]
pub struct DefaultExperienceExtractor;

#[async_trait]
impl ExperienceExtractor for DefaultExperienceExtractor {
    async fn extract(
        &self,
        _conversation: &[Message],
        outcome: &AgentOutcome,
        context: &ExtractionContext,
    ) -> Vec<NewExperience> {
        let base = || NewExperience {
            collective_id: context.collective_id,
            content: String::new(),
            experience_type: ExperienceType::Generic { category: None },
            embedding: None, // Builtin computes
            importance: 0.5,
            confidence: 0.5,
            domain: vec![],
            source_agent: AgentId(context.agent_id.clone()),
            source_task: None,
            related_files: vec![],
        };

        match outcome {
            AgentOutcome::Complete { response } => {
                if response.is_empty() {
                    return vec![];
                }
                let mut exp = base();
                exp.content = format!(
                    "Task: {}\n\nResult: {}",
                    context.task_description,
                    truncate(response, 8192)
                );
                exp.experience_type = ExperienceType::Generic {
                    category: Some("task_completion".into()),
                };
                exp.importance = 0.7;
                exp.confidence = 0.8;
                vec![exp]
            }
            AgentOutcome::Error { error } => {
                let mut exp = base();
                exp.content = format!("Task: {}\n\nError: {}", context.task_description, error);
                exp.experience_type = ExperienceType::ErrorPattern {
                    signature: truncate(error, 500),
                    fix: String::new(),
                    prevention: String::new(),
                };
                exp.importance = 0.5;
                exp.confidence = 0.5;
                vec![exp]
            }
            AgentOutcome::MaxIterationsReached => {
                let mut exp = base();
                exp.content = format!(
                    "Task: {}\n\nAgent reached maximum iterations without completing.",
                    context.task_description
                );
                exp.experience_type = ExperienceType::Difficulty {
                    description: "Agent reached max iterations".into(),
                    severity: Severity::Medium,
                };
                exp.importance = 0.6;
                exp.confidence = 0.7;
                vec![exp]
            }
        }
    }
}

/// Truncate a string to max_len, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsedb::CollectiveId;

    fn test_context() -> ExtractionContext {
        ExtractionContext {
            agent_id: "agent-1".into(),
            collective_id: CollectiveId::new(),
            task_description: "Analyze the codebase".into(),
        }
    }

    #[tokio::test]
    async fn test_extract_complete_outcome() {
        let extractor = DefaultExperienceExtractor;
        let outcome = AgentOutcome::Complete {
            response: "Found 3 issues in the code.".into(),
        };

        let experiences = extractor.extract(&[], &outcome, &test_context()).await;
        assert_eq!(experiences.len(), 1);
        assert!(experiences[0].content.contains("Found 3 issues"));
        assert!(experiences[0].content.contains("Analyze the codebase"));
        assert!((experiences[0].importance - 0.7).abs() < f32::EPSILON);
        assert!(matches!(
            &experiences[0].experience_type,
            ExperienceType::Generic { category: Some(c) } if c == "task_completion"
        ));
    }

    #[tokio::test]
    async fn test_extract_error_outcome() {
        let extractor = DefaultExperienceExtractor;
        let outcome = AgentOutcome::Error {
            error: "LLM timeout".into(),
        };

        let experiences = extractor.extract(&[], &outcome, &test_context()).await;
        assert_eq!(experiences.len(), 1);
        assert!(experiences[0].content.contains("LLM timeout"));
        assert!(matches!(
            &experiences[0].experience_type,
            ExperienceType::ErrorPattern { signature, .. } if signature == "LLM timeout"
        ));
    }

    #[tokio::test]
    async fn test_extract_max_iterations() {
        let extractor = DefaultExperienceExtractor;
        let outcome = AgentOutcome::MaxIterationsReached;

        let experiences = extractor.extract(&[], &outcome, &test_context()).await;
        assert_eq!(experiences.len(), 1);
        assert!(matches!(
            &experiences[0].experience_type,
            ExperienceType::Difficulty {
                severity: Severity::Medium,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn test_extract_empty_response_skipped() {
        let extractor = DefaultExperienceExtractor;
        let outcome = AgentOutcome::Complete {
            response: "".into(),
        };

        let experiences = extractor.extract(&[], &outcome, &test_context()).await;
        assert!(experiences.is_empty());
    }

    #[tokio::test]
    async fn test_extract_sets_context_fields() {
        let extractor = DefaultExperienceExtractor;
        let ctx = test_context();
        let outcome = AgentOutcome::Complete {
            response: "result".into(),
        };

        let experiences = extractor.extract(&[], &outcome, &ctx).await;
        assert_eq!(experiences[0].collective_id, ctx.collective_id);
        assert_eq!(experiences[0].source_agent.0, "agent-1");
        assert!(experiences[0].embedding.is_none()); // Builtin computes
    }
}
