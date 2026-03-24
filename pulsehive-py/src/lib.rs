//! Python bindings for PulseHive — shared consciousness SDK for multi-agent AI systems.
//!
//! This crate provides PyO3-based Python bindings for the core PulseHive types,
//! enabling Python developers to build multi-agent AI systems with Rust performance.

use pyo3::prelude::*;

pub mod agents;
pub mod events;
pub mod hivemind;
pub mod stream;
pub mod tool;
pub mod types;

/// PulseHive Python module — shared consciousness SDK for multi-agent AI systems.
///
/// Core types:
///   - HiveMind: orchestrator for deploying agents
///   - AgentDefinition: agent blueprint (name + kind)
///   - AgentKind: Llm, Sequential, Parallel, or Loop
///   - Lens: perception filter for agent context
///   - LlmConfig: LLM provider selection and generation parameters
///   - Task: task description for agent deployment
///   - HiveEvent: lifecycle and observability events
///
/// Provider factories:
///   - openai_provider(api_key, base_url=None)
///   - anthropic_provider(api_key)
#[pymodule]
fn _pulsehive_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    types::register(m)?;
    agents::register(m)?;
    events::register(m)?;
    hivemind::register(m)?;
    stream::register(m)?;
    tool::register(m)?;
    Ok(())
}

/// Returns the PulseHive SDK version.
#[pyfunction]
fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
