//! Python bindings for HiveEvent — lifecycle and observability events.
//!
//! Uses a tagged-class pattern: `.event_type` returns a string discriminator,
//! `.data` returns a dict with variant-specific fields.

use std::collections::HashMap;

use pyo3::prelude::*;
use pyo3::types::PyDict;

use pulsehive_core::agent::AgentOutcome;
use pulsehive_core::event::HiveEvent;

/// Lifecycle and observability event from the PulseHive runtime.
///
/// Events are emitted during agent execution and consumed via the event stream.
/// This is a read-only wrapper — Python never constructs events.
///
/// Properties:
///     event_type: String tag identifying the event kind
///     agent_id: Agent ID (if applicable, else None)
///     data: Dict with all event fields
#[pyclass(name = "HiveEvent", frozen)]
pub struct PyHiveEvent {
    event_type: String,
    agent_id: Option<String>,
    fields: HashMap<String, PyEventValue>,
}

/// Internal value type for event fields.
#[derive(Clone)]
enum PyEventValue {
    Str(String),
    Int(u64),
    Uint(usize),
}

impl PyEventValue {
    fn to_py(&self, py: Python<'_>) -> Py<PyAny> {
        match self {
            PyEventValue::Str(s) => s.into_pyobject(py).unwrap().into_any().unbind(),
            PyEventValue::Int(n) => n.into_pyobject(py).unwrap().into_any().unbind(),
            PyEventValue::Uint(n) => n.into_pyobject(py).unwrap().into_any().unbind(),
        }
    }
}

#[pymethods]
impl PyHiveEvent {
    /// Event type as a snake_case string tag.
    ///
    /// Values: "agent_started", "agent_completed", "llm_call_started",
    /// "llm_call_completed", "llm_token_streamed", "tool_call_started",
    /// "tool_call_completed", "tool_approval_requested", "experience_recorded",
    /// "relationship_inferred", "insight_generated", "substrate_perceived",
    /// "watch_notification"
    #[getter]
    fn event_type(&self) -> &str {
        &self.event_type
    }

    /// Agent ID associated with this event (None for some event types).
    #[getter]
    fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    /// All event fields as a dictionary.
    #[getter]
    fn data<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (key, value) in &self.fields {
            dict.set_item(key, value.to_py(py))?;
        }
        Ok(dict)
    }

    fn __repr__(&self) -> String {
        let details: Vec<String> = self
            .fields
            .iter()
            .take(3)
            .map(|(k, v)| match v {
                PyEventValue::Str(s) if s.len() > 30 => format!("{k}='{}'...", &s[..30]),
                PyEventValue::Str(s) => format!("{k}='{s}'"),
                PyEventValue::Int(n) => format!("{k}={n}"),
                PyEventValue::Uint(n) => format!("{k}={n}"),
            })
            .collect();

        format!("HiveEvent({}, {})", self.event_type, details.join(", "))
    }
}

impl From<HiveEvent> for PyHiveEvent {
    fn from(event: HiveEvent) -> Self {
        let mut fields = HashMap::new();

        let (event_type, agent_id) = match event {
            HiveEvent::AgentStarted {
                agent_id,
                name,
                kind,
            } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("name".into(), PyEventValue::Str(name));
                fields.insert("kind".into(), PyEventValue::Str(format!("{kind:?}")));
                ("agent_started", Some(agent_id))
            }
            HiveEvent::AgentCompleted { agent_id, outcome } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                match &outcome {
                    AgentOutcome::Complete { response } => {
                        fields.insert("outcome".into(), PyEventValue::Str("complete".into()));
                        fields.insert("response".into(), PyEventValue::Str(response.clone()));
                    }
                    AgentOutcome::Error { error } => {
                        fields.insert("outcome".into(), PyEventValue::Str("error".into()));
                        fields.insert("error".into(), PyEventValue::Str(error.clone()));
                    }
                    AgentOutcome::MaxIterationsReached => {
                        fields.insert(
                            "outcome".into(),
                            PyEventValue::Str("max_iterations_reached".into()),
                        );
                    }
                }
                ("agent_completed", Some(agent_id))
            }
            HiveEvent::LlmCallStarted {
                agent_id,
                model,
                message_count,
            } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("model".into(), PyEventValue::Str(model));
                fields.insert("message_count".into(), PyEventValue::Uint(message_count));
                ("llm_call_started", Some(agent_id))
            }
            HiveEvent::LlmCallCompleted {
                agent_id,
                model,
                duration_ms,
            } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("model".into(), PyEventValue::Str(model));
                fields.insert("duration_ms".into(), PyEventValue::Int(duration_ms));
                ("llm_call_completed", Some(agent_id))
            }
            HiveEvent::LlmTokenStreamed { agent_id, token } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("token".into(), PyEventValue::Str(token));
                ("llm_token_streamed", Some(agent_id))
            }
            HiveEvent::ToolCallStarted {
                agent_id,
                tool_name,
            } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("tool_name".into(), PyEventValue::Str(tool_name));
                ("tool_call_started", Some(agent_id))
            }
            HiveEvent::ToolCallCompleted {
                agent_id,
                tool_name,
                duration_ms,
            } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("tool_name".into(), PyEventValue::Str(tool_name));
                fields.insert("duration_ms".into(), PyEventValue::Int(duration_ms));
                ("tool_call_completed", Some(agent_id))
            }
            HiveEvent::ToolApprovalRequested {
                agent_id,
                tool_name,
                description,
            } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("tool_name".into(), PyEventValue::Str(tool_name));
                fields.insert("description".into(), PyEventValue::Str(description));
                ("tool_approval_requested", Some(agent_id))
            }
            HiveEvent::ExperienceRecorded {
                experience_id,
                agent_id,
            } => {
                fields.insert(
                    "experience_id".into(),
                    PyEventValue::Str(experience_id.to_string()),
                );
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                ("experience_recorded", Some(agent_id))
            }
            HiveEvent::RelationshipInferred { relation_id } => {
                fields.insert(
                    "relation_id".into(),
                    PyEventValue::Str(relation_id.to_string()),
                );
                ("relationship_inferred", None)
            }
            HiveEvent::InsightGenerated {
                insight_id,
                source_count,
            } => {
                fields.insert(
                    "insight_id".into(),
                    PyEventValue::Str(insight_id.to_string()),
                );
                fields.insert("source_count".into(), PyEventValue::Uint(source_count));
                ("insight_generated", None)
            }
            HiveEvent::SubstratePerceived {
                agent_id,
                experience_count,
                insight_count,
            } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert(
                    "experience_count".into(),
                    PyEventValue::Uint(experience_count),
                );
                fields.insert("insight_count".into(), PyEventValue::Uint(insight_count));
                ("substrate_perceived", Some(agent_id))
            }
            HiveEvent::EmbeddingComputed {
                agent_id,
                dimensions,
                duration_ms,
            } => {
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("dimensions".into(), PyEventValue::Uint(dimensions));
                fields.insert("duration_ms".into(), PyEventValue::Int(duration_ms));
                ("embedding_computed", Some(agent_id))
            }
            HiveEvent::WatchNotification {
                experience_id,
                collective_id,
                event_type,
            } => {
                fields.insert(
                    "experience_id".into(),
                    PyEventValue::Str(experience_id.to_string()),
                );
                fields.insert(
                    "collective_id".into(),
                    PyEventValue::Str(collective_id.to_string()),
                );
                fields.insert("event_type".into(), PyEventValue::Str(event_type));
                ("watch_notification", None)
            }
        };

        Self {
            event_type: event_type.to_string(),
            agent_id,
            fields,
        }
    }
}

/// Register event classes with the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyHiveEvent>()?;
    Ok(())
}
