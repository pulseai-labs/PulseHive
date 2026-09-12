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
        conversation: &[Message],
        outcome: &AgentOutcome,
        context: &ExtractionContext,
    ) -> Vec<NewExperience> {
        match outcome {
            AgentOutcome::Complete { response } => {
                if response.is_empty() {
                    return vec![];
                }
                let mut exp = new_experience(context);
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
                extract_error_experiences(conversation, error, context)
            }
            AgentOutcome::MaxIterationsReached => {
                let mut exp = new_experience(context);
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

fn new_experience(context: &ExtractionContext) -> NewExperience {
    NewExperience {
        collective_id: context.collective_id,
        content: String::new(),
        experience_type: ExperienceType::Generic { category: None },
        embedding: None, // Builtin computes
        importance: 0.5,
        confidence: 0.5,
        domain: vec![],
        tags: Default::default(),
        source_agent: AgentId(context.agent_id.clone()),
        source_task: None,
        related_files: vec![],
    }
}

fn extract_error_experiences(
    conversation: &[Message],
    error: &str,
    context: &ExtractionContext,
) -> Vec<NewExperience> {
    let tool_results = extract_tool_summaries(conversation);
    let mut experiences = Vec::new();
    if !tool_results.is_empty() {
        let mut partial = new_experience(context);
        let summaries = tool_results
            .iter()
            .map(|summary| format!("- {summary}"))
            .collect::<Vec<_>>()
            .join("\n");
        partial.content = format!(
            "Task: {}\n\nPartial progress ({} tool calls completed):\n{}\n\nFailed with: {}",
            context.task_description,
            tool_results.len(),
            summaries,
            error,
        );
        partial.experience_type = ExperienceType::Generic {
            category: Some("partial_completion".into()),
        };
        partial.importance = 0.6;
        partial.confidence = 0.6;
        experiences.push(partial);
    }

    let mut exp = new_experience(context);
    exp.content = format!("Task: {}\n\nError: {}", context.task_description, error);
    exp.experience_type = ExperienceType::ErrorPattern {
        signature: truncate(error, 500),
        fix: String::new(),
        prevention: String::new(),
    };
    exp.importance = 0.5;
    exp.confidence = 0.5;
    experiences.push(exp);
    experiences
}

/// Extract summaries of successful tool results from the conversation.
///
/// Filters out error results (prefixed with "Error:") and truncates each
/// summary for inclusion in partial completion experiences.
fn extract_tool_summaries(conversation: &[Message]) -> Vec<String> {
    conversation
        .iter()
        .filter_map(|msg| {
            if let Message::ToolResult { content, .. } = msg {
                // Skip error results (from denied tools, tool failures, etc.)
                if content.starts_with("Error:") {
                    None
                } else {
                    Some(truncate(content, 200))
                }
            } else {
                None
            }
        })
        .collect()
}

/// Truncate a string to at most `max_len` bytes, appending "..." if truncated.
///
/// The cut index is moved back to the nearest UTF-8 character boundary so a
/// multibyte character straddling `max_len` never panics and the truncated
/// prefix stays valid UTF-8.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let mut end = max_len;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
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

    // ── Partial experience recording tests ───────────────────────────

    #[tokio::test]
    async fn test_extract_error_with_partial_progress() {
        let extractor = DefaultExperienceExtractor;
        let conversation = vec![
            Message::user("Do the task"),
            Message::assistant_with_tool_calls(vec![]),
            Message::tool_result("call_1", "Search results: found 3 items"),
            Message::tool_result("call_2", "Processed 2 of 3 items"),
        ];
        let outcome = AgentOutcome::Error {
            error: "LLM timeout on third call".into(),
        };

        let experiences = extractor
            .extract(&conversation, &outcome, &test_context())
            .await;

        // Should produce 2 experiences: partial_completion + error
        assert_eq!(
            experiences.len(),
            2,
            "Expected 2 experiences (partial + error)"
        );

        // First: partial completion
        assert!(matches!(
            &experiences[0].experience_type,
            ExperienceType::Generic { category: Some(c) } if c == "partial_completion"
        ));
        assert!(experiences[0].content.contains("2 tool calls completed"));
        assert!(experiences[0].content.contains("Search results"));
        assert!(experiences[0].content.contains("Processed 2"));
        assert!(experiences[0].content.contains("LLM timeout"));
        assert!((experiences[0].importance - 0.6).abs() < f32::EPSILON);

        // Second: error pattern (unchanged behavior)
        assert!(matches!(
            &experiences[1].experience_type,
            ExperienceType::ErrorPattern { signature, .. } if signature == "LLM timeout on third call"
        ));
    }

    #[tokio::test]
    async fn test_extract_error_no_prior_tools_backward_compatible() {
        let extractor = DefaultExperienceExtractor;
        let outcome = AgentOutcome::Error {
            error: "Immediate failure".into(),
        };

        // Empty conversation — no tool results
        let experiences = extractor.extract(&[], &outcome, &test_context()).await;

        // Should produce exactly 1 experience (error only), same as before
        assert_eq!(experiences.len(), 1);
        assert!(matches!(
            &experiences[0].experience_type,
            ExperienceType::ErrorPattern { .. }
        ));
    }

    #[tokio::test]
    async fn test_extract_error_skips_error_tool_results() {
        let extractor = DefaultExperienceExtractor;
        let conversation = vec![
            Message::tool_result("call_1", "Error: Tool execution denied: restricted"),
            Message::tool_result("call_2", "Successfully fetched data"),
        ];
        let outcome = AgentOutcome::Error {
            error: "Failed after partial work".into(),
        };

        let experiences = extractor
            .extract(&conversation, &outcome, &test_context())
            .await;

        // Partial should only include the non-error tool result
        assert_eq!(experiences.len(), 2);
        assert!(experiences[0].content.contains("1 tool calls completed"));
        assert!(experiences[0].content.contains("Successfully fetched"));
        assert!(!experiences[0].content.contains("denied"));
    }

    // ── UTF-8-safe truncation tests ──────────────────────────────────

    #[test]
    fn test_truncate_multibyte_cut_floors_to_char_boundary() {
        // "あ" is 3 bytes: an 11-byte cut lands mid-character.
        let s = "あ".repeat(10);
        assert_eq!(truncate(&s, 11), format!("{}...", "あ".repeat(3)));
        // U+1F600 is 4 bytes: a 6-byte cut splits the second emoji.
        assert_eq!(truncate("😀😀😀", 6), "😀...");
    }

    #[test]
    fn test_truncate_exact_boundary_and_short_inputs() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 3), "hel...");
        // A cut exactly on a character boundary keeps that character.
        assert_eq!(truncate("あいう", 6), format!("{}...", "あい"));
    }

    #[test]
    fn test_truncate_never_panics_across_all_cut_points() {
        let s = "a漢b😀cé"; // 1-, 2-, 3- and 4-byte characters
        for cut in 0..=s.len() {
            let out = truncate(s, cut);
            assert!(out.ends_with("...") || out.len() == s.len());
        }
    }

    #[tokio::test]
    async fn test_extract_complete_multibyte_response_cuts_safely() {
        // 15000 bytes truncated at 8192: 8192 % 3 == 2, a mid-character cut
        // that panicked before the UTF-8-safe truncation.
        let extractor = DefaultExperienceExtractor;
        let outcome = AgentOutcome::Complete {
            response: "あ".repeat(5000),
        };

        let experiences = extractor.extract(&[], &outcome, &test_context()).await;
        let expected_tail = format!("Result: {}...", "あ".repeat(2730));
        assert!(experiences[0].content.ends_with(&expected_tail));
    }

    #[tokio::test]
    async fn test_extract_error_signature_multibyte_cuts_safely() {
        // 600 bytes truncated at 500: 500 % 3 == 2, a mid-character cut.
        let extractor = DefaultExperienceExtractor;
        let outcome = AgentOutcome::Error {
            error: "あ".repeat(200),
        };

        let experiences = extractor.extract(&[], &outcome, &test_context()).await;
        assert!(matches!(
            &experiences[0].experience_type,
            ExperienceType::ErrorPattern { signature, .. }
                if *signature == format!("{}...", "あ".repeat(166))
        ));
    }

    #[tokio::test]
    async fn test_extract_tool_summary_multibyte_cuts_safely() {
        // 210 bytes truncated at 200: 200 % 3 == 2, a mid-character cut.
        let extractor = DefaultExperienceExtractor;
        let conversation = vec![Message::tool_result("call_1", "あ".repeat(70))];
        let outcome = AgentOutcome::Error {
            error: "boom".into(),
        };

        let experiences = extractor
            .extract(&conversation, &outcome, &test_context())
            .await;
        assert!(experiences[0]
            .content
            .contains(&format!("- {}...", "あ".repeat(66))));
    }
}
