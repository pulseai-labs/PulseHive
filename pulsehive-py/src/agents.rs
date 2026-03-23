//! Python bindings for AgentDefinition, AgentKind, and AgentOutcome.
//!
//! Uses the tagged-class pattern since PyO3 doesn't support Rust enums with data fields.
//! AgentKind variants are created via static factory methods (`.llm()`, `.sequential()`, etc.).

use pyo3::prelude::*;

use pulsehive_core::agent::{
    AgentDefinition, AgentKind, AgentOutcome, LlmAgentConfig,
};

use crate::types::{PyLens, PyLlmConfig};

// ── AgentKind ────────────────────────────────────────────────────────

/// Agent execution kind — determines how an agent operates.
///
/// Create using static factory methods:
///   - ``AgentKind.llm(system_prompt, lens, llm_config)`` — LLM-powered agent
///   - ``AgentKind.sequential(agents)`` — run children in order
///   - ``AgentKind.parallel(agents)`` — run children concurrently
///   - ``AgentKind.loop_(agent, max_iterations)`` — repeat agent N times
#[pyclass(name = "AgentKind", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyAgentKind {
    pub(crate) inner: AgentKind,
}

#[pymethods]
impl PyAgentKind {
    /// Create an LLM-powered agent kind.
    ///
    /// Args:
    ///     system_prompt: System prompt configuring agent behavior
    ///     lens: Perception filter for substrate access
    ///     llm_config: LLM provider selection and generation parameters
    ///     refresh_every_n_tool_calls: Re-perceive substrate every N tool calls (optional)
    #[staticmethod]
    #[pyo3(signature = (system_prompt, lens, llm_config, refresh_every_n_tool_calls=None))]
    fn llm(
        system_prompt: String,
        lens: PyLens,
        llm_config: PyLlmConfig,
        refresh_every_n_tool_calls: Option<usize>,
    ) -> Self {
        Self {
            inner: AgentKind::Llm(Box::new(LlmAgentConfig {
                system_prompt,
                tools: vec![], // Tools deferred to Sprint 11
                lens: lens.inner,
                llm_config: llm_config.inner,
                experience_extractor: None,
                refresh_every_n_tool_calls,
            })),
        }
    }

    /// Create a sequential workflow — children execute in order.
    ///
    /// Each child perceives experiences recorded by all previous children
    /// through the shared substrate (shared consciousness model).
    #[staticmethod]
    fn sequential(agents: Vec<PyAgentDefinition>) -> Self {
        Self {
            inner: AgentKind::Sequential(
                agents.into_iter().map(|a| a.inner).collect(),
            ),
        }
    }

    /// Create a parallel workflow — children execute concurrently.
    ///
    /// Children share the substrate and can perceive each other's
    /// experiences as they're written in real-time.
    #[staticmethod]
    fn parallel(agents: Vec<PyAgentDefinition>) -> Self {
        Self {
            inner: AgentKind::Parallel(
                agents.into_iter().map(|a| a.inner).collect(),
            ),
        }
    }

    /// Create a loop workflow — repeats agent up to max_iterations times.
    ///
    /// Each iteration perceives cumulative experiences from prior iterations.
    /// Terminates early if the agent's response contains ``[LOOP_DONE]``.
    #[staticmethod]
    fn loop_(agent: PyAgentDefinition, max_iterations: usize) -> Self {
        Self {
            inner: AgentKind::Loop {
                agent: Box::new(agent.inner),
                max_iterations,
            },
        }
    }

    /// Returns the kind tag as a string ("llm", "sequential", "parallel", "loop").
    #[getter]
    fn kind_tag(&self) -> &str {
        match &self.inner {
            AgentKind::Llm(_) => "llm",
            AgentKind::Sequential(_) => "sequential",
            AgentKind::Parallel(_) => "parallel",
            AgentKind::Loop { .. } => "loop",
        }
    }

    fn __repr__(&self) -> String {
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
                    "AgentKind.loop_('{}', max_iterations={})",
                    agent.name, max_iterations
                )
            }
        }
    }
}

// ── AgentDefinition ──────────────────────────────────────────────────

/// Agent blueprint — a name paired with an execution kind.
///
/// Args:
///     name: Human-readable agent name (used in events and logging)
///     kind: AgentKind determining how the agent executes
#[pyclass(name = "AgentDefinition", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyAgentDefinition {
    pub(crate) inner: AgentDefinition,
}

#[pymethods]
impl PyAgentDefinition {
    #[new]
    fn new(name: String, kind: PyAgentKind) -> Self {
        Self {
            inner: AgentDefinition {
                name,
                kind: kind.inner,
            },
        }
    }

    /// Agent name.
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    /// Agent kind as a string tag.
    #[getter]
    fn kind_tag(&self) -> &str {
        match &self.inner.kind {
            AgentKind::Llm(_) => "llm",
            AgentKind::Sequential(_) => "sequential",
            AgentKind::Parallel(_) => "parallel",
            AgentKind::Loop { .. } => "loop",
        }
    }

    fn __repr__(&self) -> String {
        format!("AgentDefinition('{}', {})", self.inner.name, self.kind_tag())
    }
}

// ── AgentOutcome ─────────────────────────────────────────────────────

/// Result of agent execution — complete, error, or max iterations reached.
///
/// Properties:
///     kind: "complete", "error", or "max_iterations_reached"
///     response: Agent's final response (only for "complete")
///     error: Error description (only for "error")
#[pyclass(name = "AgentOutcome", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyAgentOutcome {
    pub(crate) inner: AgentOutcome,
}

#[pymethods]
impl PyAgentOutcome {
    /// Outcome kind: "complete", "error", or "max_iterations_reached".
    #[getter]
    fn kind(&self) -> &str {
        match &self.inner {
            AgentOutcome::Complete { .. } => "complete",
            AgentOutcome::Error { .. } => "error",
            AgentOutcome::MaxIterationsReached => "max_iterations_reached",
        }
    }

    /// Agent's final response (None if not complete).
    #[getter]
    fn response(&self) -> Option<&str> {
        match &self.inner {
            AgentOutcome::Complete { response } => Some(response),
            _ => None,
        }
    }

    /// Error description (None if not error).
    #[getter]
    fn error(&self) -> Option<&str> {
        match &self.inner {
            AgentOutcome::Error { error } => Some(error),
            _ => None,
        }
    }

    fn __repr__(&self) -> String {
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
        }
    }
}

impl From<AgentOutcome> for PyAgentOutcome {
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

/// Register agent classes with the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAgentKind>()?;
    m.add_class::<PyAgentDefinition>()?;
    m.add_class::<PyAgentOutcome>()?;
    Ok(())
}
