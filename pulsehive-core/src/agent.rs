//! Agent definition types and workflow composition.
//!
//! Agents are defined as data ([`AgentDefinition`] + [`AgentKind`]) and composed
//! declaratively. The framework handles execution — products only define
//! what the agent knows and can do.
//!
//! # Example
//! ```rust,ignore
//! let agent = AgentDefinition {
//!     name: "researcher".into(),
//!     kind: AgentKind::Llm(LlmAgentConfig {
//!         system_prompt: "You are a research assistant.".into(),
//!         tools: vec![Arc::new(WebSearch)],
//!         lens: Lens::new(["research", "papers"]),
//!         llm_config: LlmConfig::new("openai", "gpt-4"),
//!         experience_extractor: None,
//!     }),
//! };
//! ```

use std::sync::Arc;

use crate::ids::CollectiveId;
use crate::lens::Lens;
use crate::llm::LlmConfig;
use crate::tool::Tool;

/// Blueprint for creating an agent. Not a running agent — just configuration.
///
/// The framework instantiates and runs agents internally via `HiveMind::deploy()`.
/// `Clone` is supported: tools and extractors use `Arc` for cheap reference-counted sharing.
#[derive(Clone)]
pub struct AgentDefinition {
    /// Human-readable name for this agent.
    pub name: String,
    /// What kind of agent this is (LLM-powered or workflow orchestrator).
    pub kind: AgentKind,
}

/// Determines how an agent executes.
#[derive(Clone)]
pub enum AgentKind {
    /// LLM-powered agent with tools and lens-based perception.
    ///
    /// Boxed because `LlmAgentConfig` is large (~232 bytes) while workflow
    /// variants are small (~24 bytes). Boxing keeps the enum size uniform.
    Llm(Box<LlmAgentConfig>),

    /// Runs sub-agents sequentially — each sees previous agents' experiences.
    Sequential(Vec<AgentDefinition>),

    /// Runs sub-agents in parallel — all share substrate in real-time.
    Parallel(Vec<AgentDefinition>),

    /// Repeats a sub-agent until max_iterations or completion signal.
    Loop {
        agent: Box<AgentDefinition>,
        max_iterations: usize,
    },
}

/// Configuration for an LLM-powered agent.
///
/// `Clone` is supported: tools and extractors are `Arc`-wrapped for cheap sharing
/// across workflow iterations (Loop) and concurrent tasks (Parallel).
#[derive(Clone)]
pub struct LlmAgentConfig {
    /// System prompt that specializes this agent's behavior.
    pub system_prompt: String,

    /// Tools this agent can invoke during the Act phase.
    pub tools: Vec<Arc<dyn Tool>>,

    /// How this agent perceives the substrate.
    pub lens: Lens,

    /// Which LLM provider and model to use.
    pub llm_config: LlmConfig,

    /// Optional override for experience extraction logic.
    /// `None` uses the framework's default extractor.
    pub experience_extractor: Option<Arc<dyn ExperienceExtractor>>,

    /// How often to re-perceive the substrate during the Think→Act loop.
    ///
    /// When set to `Some(n)`, the agent re-runs the Perceive phase every `n` tool calls,
    /// picking up new experiences recorded by other agents in the same collective.
    /// This enables real-time "shared consciousness" in parallel workflows.
    ///
    /// `None` disables mid-task refresh (perceive only once at start).
    pub refresh_every_n_tool_calls: Option<usize>,
}

/// Context passed to the experience extractor during the Record phase.
#[derive(Debug, Clone)]
pub struct ExtractionContext {
    /// ID of the agent whose conversation is being extracted.
    pub agent_id: String,
    /// Collective where extracted experiences will be stored.
    ///
    /// This is the core-owned [`CollectiveId`], not `pulsedb::CollectiveId` —
    /// the two families share no type identity. Both expose public
    /// `as_bytes()`/`from_bytes()`, so an implementor producing
    /// `pulsedb::NewExperience` values converts losslessly with
    /// `pulsedb::CollectiveId::from_bytes(*context.collective_id.as_bytes())`.
    pub collective_id: CollectiveId,
    /// Description of the task the agent was working on.
    pub task_description: String,
}

/// Trait for extracting experiences from an agent's conversation history.
///
/// The default implementation (provided by the framework) uses simple rules
/// to create experiences from the agent's outcome. Products can override
/// this to implement custom extraction logic (e.g., LLM-based summarization).
///
/// Without the `substrate` feature this trait intentionally has zero methods:
/// the one-literal-shape rule keeps [`LlmAgentConfig::experience_extractor`]
/// (and every other API naming this trait) identical across feature sets, so
/// `impl ExperienceExtractor for T {}` compiles substrate-free. Note that
/// Cargo feature unification means such an impl gains a required `extract`
/// method — whose signature mentions `pulsedb` types — when another
/// dependency in the build enables `substrate`.
#[async_trait::async_trait]
pub trait ExperienceExtractor: Send + Sync {
    /// Extract experiences from a completed agent conversation.
    ///
    /// Called after the agentic loop completes. Returns experiences to be
    /// stored in the substrate for future perception by other agents.
    ///
    /// `context.collective_id` is the core-owned [`CollectiveId`]; convert it
    /// to the `pulsedb` ID `NewExperience` expects through the public byte
    /// seam both families expose:
    /// `pulsedb::CollectiveId::from_bytes(*context.collective_id.as_bytes())`.
    #[cfg(feature = "substrate")]
    async fn extract(
        &self,
        conversation: &[crate::llm::Message],
        outcome: &AgentOutcome,
        context: &ExtractionContext,
    ) -> Vec<pulsedb::NewExperience>;
}

/// Outcome of an agent's execution.
///
/// `#[non_exhaustive]` from Release 1 (ADR-014): consumers must keep a
/// wildcard arm in every `match`; new variants land additively.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentOutcome {
    /// Agent completed successfully with a final response.
    Complete { response: String },
    /// Agent encountered an error.
    ///
    /// Uses `String` instead of `PulseHiveError` because `AgentOutcome`
    /// must be `Clone` (used in `HiveEvent` which requires `Clone` for broadcast).
    Error { error: String },
    /// Agent hit the maximum iteration limit without completing.
    MaxIterationsReached,
    /// The caller cancelled the run (ADR-014).
    ///
    /// `partial_response` carries the latest assistant text produced before
    /// the cancel point — empty if the turn ended before any. Partial tool
    /// output is not repeated here: it stays on the `ToolCallCompleted`
    /// events already emitted. For a composite agent, `partial_response`
    /// mirrors what its `Complete` would have carried over the children that
    /// completed before the cancellation.
    Cancelled { partial_response: String },
    /// A composite agent finished with only some children succeeding (#45).
    ///
    /// `responses` collects the completed children's responses in child
    /// declaration order; `errors` describes each child that did not
    /// contribute one in the same order — a child that hit its iteration
    /// cap is listed as `<agent>: max iterations reached`. A nested
    /// `PartialComplete` merges into its parent's `responses` and
    /// `errors`. Sequential and Loop workflows treat a child's
    /// `PartialComplete` as progress and continue.
    PartialComplete {
        /// Responses contributed by children that completed.
        responses: Vec<String>,
        /// One entry per child that did not contribute a response.
        errors: Vec<String>,
    },
}

/// Compact tag for agent kind, used in [`HiveEvent`](crate::event::HiveEvent) variants.
///
/// Carries no data — just identifies the type of agent for event reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKindTag {
    Llm,
    Sequential,
    Parallel,
    Loop,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_outcome_debug_clone() {
        let outcome = AgentOutcome::Complete {
            response: "Done!".into(),
        };
        let cloned = outcome.clone();
        assert!(matches!(cloned, AgentOutcome::Complete { response } if response == "Done!"));

        // Debug works
        let debug = format!("{:?}", outcome);
        assert!(debug.contains("Complete"));
    }

    #[test]
    fn test_agent_outcome_variants() {
        let complete = AgentOutcome::Complete {
            response: "result".into(),
        };
        assert!(matches!(complete, AgentOutcome::Complete { .. }));

        let error = AgentOutcome::Error {
            error: "timeout".into(),
        };
        assert!(matches!(error, AgentOutcome::Error { .. }));

        let max = AgentOutcome::MaxIterationsReached;
        assert!(matches!(max, AgentOutcome::MaxIterationsReached));
    }

    #[test]
    fn test_agent_kind_tag() {
        assert_ne!(AgentKindTag::Llm, AgentKindTag::Sequential);
        assert_eq!(AgentKindTag::Loop, AgentKindTag::Loop);

        // Copy works
        let tag = AgentKindTag::Parallel;
        let copied = tag;
        assert_eq!(tag, copied);
    }

    #[test]
    fn test_sequential_workflow() {
        let workflow = AgentDefinition {
            name: "pipeline".into(),
            kind: AgentKind::Sequential(vec![
                AgentDefinition {
                    name: "step1".into(),
                    kind: AgentKind::Sequential(vec![]), // empty placeholder
                },
                AgentDefinition {
                    name: "step2".into(),
                    kind: AgentKind::Sequential(vec![]),
                },
            ]),
        };
        assert_eq!(workflow.name, "pipeline");
        match workflow.kind {
            AgentKind::Sequential(children) => assert_eq!(children.len(), 2),
            _ => panic!("Expected Sequential"),
        }
    }

    #[test]
    fn test_nested_workflow() {
        // Sequential(Parallel(a, b), Loop(c))
        let workflow = AgentDefinition {
            name: "complex".into(),
            kind: AgentKind::Sequential(vec![
                AgentDefinition {
                    name: "explore".into(),
                    kind: AgentKind::Parallel(vec![
                        AgentDefinition {
                            name: "explorer_a".into(),
                            kind: AgentKind::Sequential(vec![]),
                        },
                        AgentDefinition {
                            name: "explorer_b".into(),
                            kind: AgentKind::Sequential(vec![]),
                        },
                    ]),
                },
                AgentDefinition {
                    name: "refine".into(),
                    kind: AgentKind::Loop {
                        agent: Box::new(AgentDefinition {
                            name: "refiner".into(),
                            kind: AgentKind::Sequential(vec![]),
                        }),
                        max_iterations: 5,
                    },
                },
            ]),
        };
        assert_eq!(workflow.name, "complex");
    }

    #[test]
    fn test_loop_workflow() {
        let looped = AgentDefinition {
            name: "iterator".into(),
            kind: AgentKind::Loop {
                agent: Box::new(AgentDefinition {
                    name: "worker".into(),
                    kind: AgentKind::Sequential(vec![]),
                }),
                max_iterations: 10,
            },
        };
        match looped.kind {
            AgentKind::Loop { max_iterations, .. } => assert_eq!(max_iterations, 10),
            _ => panic!("Expected Loop"),
        }
    }
}
