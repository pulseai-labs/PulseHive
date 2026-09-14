//! Agent type bindings: AgentKind, AgentDefinition, AgentOutcome.
//!
//! Uses the tagged-class pattern since napi-rs doesn't support Rust enums with data fields.
//! AgentKind variants are created via static factory methods (`.llm()`, `.sequential()`, etc.).

use std::sync::Arc;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig};

#[cfg(feature = "napi")]
use napi_derive::napi;

#[cfg(feature = "napi")]
use crate::tool::{JsToolBridge, JsToolHandle};
use crate::types::{JsLens, JsLlmConfig};

// ── AgentKind ────────────────────────────────────────────────────────

/// Agent execution kind — determines how an agent operates.
///
/// Create using static factory methods:
///   - `AgentKind.llm(systemPrompt, lens, llmConfig)` — LLM-powered agent
///   - `AgentKind.sequential(agents)` — run children in order
///   - `AgentKind.parallel(agents)` — run children concurrently
///   - `AgentKind.loop(agent, maxIterations)` — repeat agent N times
#[cfg_attr(feature = "napi", napi)]
pub struct JsAgentKind {
    pub(crate) inner: AgentKind,
}

#[cfg_attr(feature = "napi", napi)]
impl JsAgentKind {
    /// Create an LLM-powered agent kind.
    ///
    /// @param systemPrompt - System prompt configuring agent behavior
    /// @param lens - Perception filter for substrate access
    /// @param llmConfig - LLM provider selection and generation parameters
    /// @param refreshEveryNToolCalls - Re-perceive substrate every N tool calls (optional)
    /// @param tools - Array of Tool instances for the agent to use (optional)
    #[cfg_attr(feature = "napi", napi(factory))]
    #[allow(unused_variables)]
    pub fn llm(
        system_prompt: String,
        lens: &JsLens,
        llm_config: &JsLlmConfig,
        refresh_every_n_tool_calls: Option<u32>,
        #[cfg(feature = "napi")] tools: Option<Vec<&JsToolHandle>>,
    ) -> Self {
        #[cfg(feature = "napi")]
        let rust_tools: Vec<Arc<dyn pulsehive_core::tool::Tool>> = tools
            .unwrap_or_default()
            .iter()
            .map(|h| Arc::new(JsToolBridge::from_handle(h)) as Arc<dyn pulsehive_core::tool::Tool>)
            .collect();

        #[cfg(not(feature = "napi"))]
        let rust_tools: Vec<Arc<dyn pulsehive_core::tool::Tool>> = vec![];

        Self {
            inner: AgentKind::Llm(Box::new(LlmAgentConfig {
                system_prompt,
                tools: rust_tools,
                lens: lens.inner.clone(),
                llm_config: llm_config.inner.clone(),
                experience_extractor: None,
                refresh_every_n_tool_calls: refresh_every_n_tool_calls.map(|n| n as usize),
            })),
        }
    }

    /// Create a sequential workflow — children execute in order.
    ///
    /// Each child perceives experiences recorded by all previous children
    /// through the shared substrate (shared consciousness model).
    #[cfg_attr(feature = "napi", napi(factory))]
    pub fn sequential(agents: Vec<&JsAgentDefinition>) -> Self {
        Self {
            inner: AgentKind::Sequential(agents.into_iter().map(|a| a.inner.clone()).collect()),
        }
    }

    /// Create a parallel workflow — children execute concurrently.
    ///
    /// Children share the substrate and can perceive each other's
    /// experiences as they're written in real-time.
    #[cfg_attr(feature = "napi", napi(factory))]
    pub fn parallel(agents: Vec<&JsAgentDefinition>) -> Self {
        Self {
            inner: AgentKind::Parallel(agents.into_iter().map(|a| a.inner.clone()).collect()),
        }
    }

    /// Create a loop workflow — repeats agent up to maxIterations times.
    ///
    /// Each iteration perceives cumulative experiences from prior iterations.
    /// Terminates early if the agent's response contains `[LOOP_DONE]`.
    #[cfg_attr(feature = "napi", napi(factory, js_name = "loop"))]
    pub fn loop_kind(agent: &JsAgentDefinition, max_iterations: u32) -> Self {
        Self {
            inner: AgentKind::Loop {
                agent: Box::new(agent.inner.clone()),
                max_iterations: max_iterations as usize,
            },
        }
    }

    /// Returns the kind tag as a string ("llm", "sequential", "parallel", "loop").
    #[cfg_attr(feature = "napi", napi(getter, js_name = "kindTag"))]
    pub fn kind_tag(&self) -> String {
        match &self.inner {
            AgentKind::Llm(_) => "llm".to_string(),
            AgentKind::Sequential(_) => "sequential".to_string(),
            AgentKind::Parallel(_) => "parallel".to_string(),
            AgentKind::Loop { .. } => "loop".to_string(),
        }
    }

    /// String representation for debugging.
    #[cfg_attr(feature = "napi", napi(js_name = "toString"))]
    pub fn to_string_js(&self) -> String {
        match &self.inner {
            AgentKind::Llm(config) => {
                format!(
                    "AgentKind.llm(model='{}', prompt='{}')",
                    config.llm_config.model,
                    truncate(&config.system_prompt, 40),
                )
            }
            AgentKind::Sequential(children) => {
                format!("AgentKind.sequential([{} agents])", children.len())
            }
            AgentKind::Parallel(children) => {
                format!("AgentKind.parallel([{} agents])", children.len())
            }
            AgentKind::Loop {
                agent,
                max_iterations,
            } => {
                format!(
                    "AgentKind.loop('{}', maxIterations={})",
                    agent.name, max_iterations
                )
            }
        }
    }
}

// ── AgentDefinition ──────────────────────────────────────────────────

/// Agent blueprint — a name paired with an execution kind.
#[cfg_attr(feature = "napi", napi)]
pub struct JsAgentDefinition {
    pub(crate) inner: AgentDefinition,
}

#[cfg_attr(feature = "napi", napi)]
impl JsAgentDefinition {
    /// Create a new AgentDefinition.
    ///
    /// @param name - Human-readable agent name (used in events and logging)
    /// @param kind - AgentKind determining how the agent executes
    #[cfg_attr(feature = "napi", napi(constructor))]
    pub fn new(name: String, kind: &JsAgentKind) -> Self {
        Self {
            inner: AgentDefinition {
                name,
                kind: kind.inner.clone(),
            },
        }
    }

    /// Agent name.
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }

    /// Agent kind as a string tag.
    #[cfg_attr(feature = "napi", napi(getter, js_name = "kindTag"))]
    pub fn kind_tag(&self) -> String {
        match &self.inner.kind {
            AgentKind::Llm(_) => "llm".to_string(),
            AgentKind::Sequential(_) => "sequential".to_string(),
            AgentKind::Parallel(_) => "parallel".to_string(),
            AgentKind::Loop { .. } => "loop".to_string(),
        }
    }

    /// String representation for debugging.
    #[cfg_attr(feature = "napi", napi(js_name = "toString"))]
    pub fn to_string_js(&self) -> String {
        format!(
            "AgentDefinition('{}', {})",
            self.inner.name,
            self.kind_tag()
        )
    }
}

// ── AgentOutcome ─────────────────────────────────────────────────────

/// Result of agent execution — complete, error, cancelled, partial, or
/// max iterations reached.
#[cfg_attr(feature = "napi", napi)]
pub struct JsAgentOutcome {
    pub(crate) inner: AgentOutcome,
}

#[cfg_attr(feature = "napi", napi)]
impl JsAgentOutcome {
    /// Outcome kind: "complete", "error", "cancelled", "partial_complete",
    /// or "max_iterations_reached".
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn kind(&self) -> String {
        match &self.inner {
            AgentOutcome::Complete { .. } => "complete".to_string(),
            AgentOutcome::Error { .. } => "error".to_string(),
            AgentOutcome::MaxIterationsReached => "max_iterations_reached".to_string(),
            AgentOutcome::Cancelled { .. } => "cancelled".to_string(),
            AgentOutcome::PartialComplete { .. } => "partial_complete".to_string(),
            _ => "unknown".to_string(),
        }
    }

    /// Agent's final response (undefined if not complete).
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn response(&self) -> Option<String> {
        match &self.inner {
            AgentOutcome::Complete { response } => Some(response.clone()),
            _ => None,
        }
    }

    /// Error description (undefined if not error).
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn error(&self) -> Option<String> {
        match &self.inner {
            AgentOutcome::Error { error } => Some(error.clone()),
            _ => None,
        }
    }

    /// String representation for debugging.
    #[cfg_attr(feature = "napi", napi(js_name = "toString"))]
    pub fn to_string_js(&self) -> String {
        match &self.inner {
            AgentOutcome::Complete { response } => {
                format!("AgentOutcome(complete, '{}')", truncate(response, 60))
            }
            AgentOutcome::Error { error } => {
                format!("AgentOutcome(error, '{}')", truncate(error, 60))
            }
            AgentOutcome::MaxIterationsReached => {
                "AgentOutcome(max_iterations_reached)".to_string()
            }
            AgentOutcome::Cancelled { partial_response } => {
                format!(
                    "AgentOutcome(cancelled, '{}')",
                    truncate(partial_response, 60)
                )
            }
            AgentOutcome::PartialComplete { responses, errors } => {
                format!(
                    "AgentOutcome(partial_complete, {} responses, {} errors)",
                    responses.len(),
                    errors.len()
                )
            }
            _ => "AgentOutcome(unknown)".to_string(),
        }
    }
}

impl From<AgentOutcome> for JsAgentOutcome {
    fn from(inner: AgentOutcome) -> Self {
        Self { inner }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
