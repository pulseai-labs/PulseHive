//! Python bindings for HiveEvent — lifecycle and observability events.
//!
//! Uses a tagged-class pattern: `.event_type` returns a string discriminator,
//! `.data` returns a dict with variant-specific fields.

use std::collections::HashMap;

use pyo3::prelude::*;
use pyo3::types::PyDict;

use pulsehive_core::agent::AgentOutcome;
use pulsehive_core::event::HiveEvent;
use pulsehive_core::tool::ToolProgress;

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
    Float(f32),
}

impl PyEventValue {
    fn to_py(&self, py: Python<'_>) -> Py<PyAny> {
        match self {
            PyEventValue::Str(s) => s.into_pyobject(py).unwrap().into_any().unbind(),
            PyEventValue::Int(n) => n.into_pyobject(py).unwrap().into_any().unbind(),
            PyEventValue::Uint(n) => n.into_pyobject(py).unwrap().into_any().unbind(),
            PyEventValue::Float(f) => f.into_pyobject(py).unwrap().into_any().unbind(),
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
                PyEventValue::Float(f) => format!("{k}={f}"),
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
                timestamp_ms,
                agent_id,
                name,
                kind,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("name".into(), PyEventValue::Str(name));
                fields.insert("kind".into(), PyEventValue::Str(format!("{kind:?}")));
                ("agent_started", Some(agent_id))
            }
            HiveEvent::AgentCompleted {
                timestamp_ms,
                agent_id,
                outcome,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
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
                timestamp_ms,
                agent_id,
                model,
                message_count,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("model".into(), PyEventValue::Str(model));
                fields.insert("message_count".into(), PyEventValue::Uint(message_count));
                ("llm_call_started", Some(agent_id))
            }
            HiveEvent::LlmCallCompleted {
                timestamp_ms,
                agent_id,
                model,
                duration_ms,
                input_tokens,
                output_tokens,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("model".into(), PyEventValue::Str(model));
                fields.insert("duration_ms".into(), PyEventValue::Int(duration_ms));
                fields.insert(
                    "input_tokens".into(),
                    PyEventValue::Int(input_tokens as u64),
                );
                fields.insert(
                    "output_tokens".into(),
                    PyEventValue::Int(output_tokens as u64),
                );
                ("llm_call_completed", Some(agent_id))
            }
            HiveEvent::LlmTokenStreamed {
                timestamp_ms,
                agent_id,
                token,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("token".into(), PyEventValue::Str(token));
                ("llm_token_streamed", Some(agent_id))
            }
            HiveEvent::ToolCallStarted {
                timestamp_ms,
                agent_id,
                tool_name,
                params,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("tool_name".into(), PyEventValue::Str(tool_name));
                fields.insert("params".into(), PyEventValue::Str(params));
                ("tool_call_started", Some(agent_id))
            }
            HiveEvent::ToolCallCompleted {
                timestamp_ms,
                agent_id,
                tool_name,
                duration_ms,
                result_preview,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("tool_name".into(), PyEventValue::Str(tool_name));
                fields.insert("duration_ms".into(), PyEventValue::Int(duration_ms));
                fields.insert("result_preview".into(), PyEventValue::Str(result_preview));
                ("tool_call_completed", Some(agent_id))
            }
            HiveEvent::ToolApprovalRequested {
                timestamp_ms,
                agent_id,
                tool_name,
                description,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("tool_name".into(), PyEventValue::Str(tool_name));
                fields.insert("description".into(), PyEventValue::Str(description));
                ("tool_approval_requested", Some(agent_id))
            }
            HiveEvent::ExperienceRecorded {
                timestamp_ms,
                experience_id,
                agent_id,
                content_preview,
                experience_type,
                importance,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert(
                    "experience_id".into(),
                    PyEventValue::Str(experience_id.to_string()),
                );
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("content_preview".into(), PyEventValue::Str(content_preview));
                fields.insert("experience_type".into(), PyEventValue::Str(experience_type));
                fields.insert("importance".into(), PyEventValue::Float(importance));
                ("experience_recorded", Some(agent_id))
            }
            HiveEvent::RelationshipInferred {
                timestamp_ms,
                relation_id,
                agent_id,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert(
                    "relation_id".into(),
                    PyEventValue::Str(relation_id.to_string()),
                );
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                ("relationship_inferred", Some(agent_id))
            }
            HiveEvent::InsightGenerated {
                timestamp_ms,
                insight_id,
                source_count,
                agent_id,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert(
                    "insight_id".into(),
                    PyEventValue::Str(insight_id.to_string()),
                );
                fields.insert("source_count".into(), PyEventValue::Uint(source_count));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                ("insight_generated", Some(agent_id))
            }
            HiveEvent::SubstratePerceived {
                timestamp_ms,
                agent_id,
                experience_count,
                insight_count,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert(
                    "experience_count".into(),
                    PyEventValue::Uint(experience_count),
                );
                fields.insert("insight_count".into(), PyEventValue::Uint(insight_count));
                ("substrate_perceived", Some(agent_id))
            }
            HiveEvent::EmbeddingComputed {
                timestamp_ms,
                agent_id,
                dimensions,
                duration_ms,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("dimensions".into(), PyEventValue::Uint(dimensions));
                fields.insert("duration_ms".into(), PyEventValue::Int(duration_ms));
                ("embedding_computed", Some(agent_id))
            }
            HiveEvent::WatchNotification {
                timestamp_ms,
                experience_id,
                collective_id,
                event_type,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
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
            HiveEvent::ToolProgress {
                timestamp_ms,
                agent_id,
                tool_name,
                progress,
            } => {
                fields.insert("timestamp_ms".into(), PyEventValue::Int(timestamp_ms));
                fields.insert("agent_id".into(), PyEventValue::Str(agent_id.clone()));
                fields.insert("tool_name".into(), PyEventValue::Str(tool_name));
                // The nested `progress` enum has no scalar map representation, so
                // emit a `progress_kind` discriminator plus the full payload as a
                // JSON string (audit ⑥ default; the flatten-scalars form is deferred).
                let progress_kind = match &progress {
                    ToolProgress::Started { .. } => "started",
                    ToolProgress::Progress { .. } => "progress",
                    ToolProgress::PartialResult { .. } => "partial_result",
                    ToolProgress::Log { .. } => "log",
                    ToolProgress::Completed { .. } => "completed",
                };
                fields.insert(
                    "progress_kind".into(),
                    PyEventValue::Str(progress_kind.to_string()),
                );
                fields.insert(
                    "progress".into(),
                    PyEventValue::Str(serde_json::to_string(&progress).unwrap_or_default()),
                );
                ("tool_progress", Some(agent_id))
            }
            // Forward-compat: `HiveEvent` is `#[non_exhaustive]`. Future variants
            // (VS-1.1.2+) map to an inert `unknown` event instead of failing to build.
            _ => ("unknown", None),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyhiveevent_tool_progress_maps_event_type() {
        let event = HiveEvent::ToolProgress {
            timestamp_ms: 1,
            agent_id: "a1".into(),
            tool_name: "backtest".into(),
            progress: ToolProgress::Progress {
                fraction: 0.5,
                message: Some("halfway".into()),
            },
        };
        let py_event = PyHiveEvent::from(event);
        assert_eq!(py_event.event_type, "tool_progress");
        assert_eq!(py_event.agent_id.as_deref(), Some("a1"));
        // Nested payload is serialized as a JSON string + a discriminator field.
        assert!(py_event.fields.contains_key("progress"));
        assert!(matches!(
            py_event.fields.get("progress_kind"),
            Some(PyEventValue::Str(s)) if s == "progress"
        ));
    }
}
