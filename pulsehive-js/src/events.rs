//! Event type binding: HiveEvent.
//!
//! Uses a tagged-class pattern: `.eventType` returns a string discriminator,
//! `.data` returns a plain object with variant-specific fields.

use std::collections::HashMap;

use pulsehive_core::agent::AgentOutcome;
use pulsehive_core::event::HiveEvent;

#[cfg(feature = "napi")]
use napi_derive::napi;

/// Lifecycle and observability event from the PulseHive runtime.
///
/// Events are emitted during agent execution and consumed via the event stream.
/// This is a read-only wrapper — JavaScript never constructs events.
#[cfg_attr(feature = "napi", napi)]
pub struct JsHiveEvent {
    event_type: String,
    agent_id: Option<String>,
    fields: HashMap<String, EventValue>,
}

/// Internal value type for event fields.
#[derive(Clone)]
pub(crate) enum EventValue {
    Str(String),
    Num(u64),
}

#[cfg_attr(feature = "napi", napi)]
impl JsHiveEvent {
    /// Event type as a snake_case string tag.
    ///
    /// Values: "agent_started", "agent_completed", "llm_call_started",
    /// "llm_call_completed", "llm_token_streamed", "tool_call_started",
    /// "tool_call_completed", "tool_approval_requested", "experience_recorded",
    /// "relationship_inferred", "insight_generated", "substrate_perceived",
    /// "watch_notification"
    #[cfg_attr(feature = "napi", napi(getter, js_name = "eventType"))]
    pub fn event_type(&self) -> String {
        self.event_type.clone()
    }

    /// Agent ID associated with this event (undefined for some event types).
    #[cfg_attr(feature = "napi", napi(getter, js_name = "agentId"))]
    pub fn agent_id(&self) -> Option<String> {
        self.agent_id.clone()
    }

    /// All event fields as a plain object.
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn data(&self) -> HashMap<String, String> {
        self.fields
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    EventValue::Str(s) => s.clone(),
                    EventValue::Num(n) => n.to_string(),
                };
                (k.clone(), val)
            })
            .collect()
    }

    /// String representation for debugging.
    #[cfg_attr(feature = "napi", napi(js_name = "toString"))]
    pub fn to_string_js(&self) -> String {
        let details: Vec<String> = self
            .fields
            .iter()
            .take(3)
            .map(|(k, v)| match v {
                EventValue::Str(s) if s.len() > 30 => format!("{k}='{}'...", &s[..30]),
                EventValue::Str(s) => format!("{k}='{s}'"),
                EventValue::Num(n) => format!("{k}={n}"),
            })
            .collect();

        format!("HiveEvent({}, {})", self.event_type, details.join(", "))
    }
}

impl From<HiveEvent> for JsHiveEvent {
    fn from(event: HiveEvent) -> Self {
        let mut fields = HashMap::new();

        let (event_type, agent_id) = match event {
            HiveEvent::AgentStarted {
                agent_id,
                name,
                kind,
            } => {
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("name".into(), EventValue::Str(name));
                fields.insert("kind".into(), EventValue::Str(format!("{kind:?}")));
                ("agent_started", Some(agent_id))
            }
            HiveEvent::AgentCompleted { agent_id, outcome } => {
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                match &outcome {
                    AgentOutcome::Complete { response } => {
                        fields.insert("outcome".into(), EventValue::Str("complete".into()));
                        fields.insert("response".into(), EventValue::Str(response.clone()));
                    }
                    AgentOutcome::Error { error } => {
                        fields.insert("outcome".into(), EventValue::Str("error".into()));
                        fields.insert("error".into(), EventValue::Str(error.clone()));
                    }
                    AgentOutcome::MaxIterationsReached => {
                        fields.insert(
                            "outcome".into(),
                            EventValue::Str("max_iterations_reached".into()),
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
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("model".into(), EventValue::Str(model));
                fields.insert("messageCount".into(), EventValue::Num(message_count as u64));
                ("llm_call_started", Some(agent_id))
            }
            HiveEvent::LlmCallCompleted {
                agent_id,
                model,
                duration_ms,
            } => {
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("model".into(), EventValue::Str(model));
                fields.insert("durationMs".into(), EventValue::Num(duration_ms));
                ("llm_call_completed", Some(agent_id))
            }
            HiveEvent::LlmTokenStreamed { agent_id, token } => {
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("token".into(), EventValue::Str(token));
                ("llm_token_streamed", Some(agent_id))
            }
            HiveEvent::ToolCallStarted {
                agent_id,
                tool_name,
            } => {
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("toolName".into(), EventValue::Str(tool_name));
                ("tool_call_started", Some(agent_id))
            }
            HiveEvent::ToolCallCompleted {
                agent_id,
                tool_name,
                duration_ms,
            } => {
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("toolName".into(), EventValue::Str(tool_name));
                fields.insert("durationMs".into(), EventValue::Num(duration_ms));
                ("tool_call_completed", Some(agent_id))
            }
            HiveEvent::ToolApprovalRequested {
                agent_id,
                tool_name,
                description,
            } => {
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("toolName".into(), EventValue::Str(tool_name));
                fields.insert("description".into(), EventValue::Str(description));
                ("tool_approval_requested", Some(agent_id))
            }
            HiveEvent::ExperienceRecorded {
                experience_id,
                agent_id,
            } => {
                fields.insert(
                    "experienceId".into(),
                    EventValue::Str(experience_id.to_string()),
                );
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                ("experience_recorded", Some(agent_id))
            }
            HiveEvent::RelationshipInferred { relation_id } => {
                fields.insert(
                    "relationId".into(),
                    EventValue::Str(relation_id.to_string()),
                );
                ("relationship_inferred", None)
            }
            HiveEvent::InsightGenerated {
                insight_id,
                source_count,
            } => {
                fields.insert("insightId".into(), EventValue::Str(insight_id.to_string()));
                fields.insert("sourceCount".into(), EventValue::Num(source_count as u64));
                ("insight_generated", None)
            }
            HiveEvent::SubstratePerceived {
                agent_id,
                experience_count,
                insight_count,
            } => {
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert(
                    "experienceCount".into(),
                    EventValue::Num(experience_count as u64),
                );
                fields.insert("insightCount".into(), EventValue::Num(insight_count as u64));
                ("substrate_perceived", Some(agent_id))
            }
            HiveEvent::EmbeddingComputed {
                agent_id,
                dimensions,
                duration_ms,
            } => {
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("dimensions".into(), EventValue::Num(dimensions as u64));
                fields.insert("durationMs".into(), EventValue::Num(duration_ms));
                ("embedding_computed", Some(agent_id))
            }
            HiveEvent::WatchNotification {
                experience_id,
                collective_id,
                event_type,
            } => {
                fields.insert(
                    "experienceId".into(),
                    EventValue::Str(experience_id.to_string()),
                );
                fields.insert(
                    "collectiveId".into(),
                    EventValue::Str(collective_id.to_string()),
                );
                fields.insert("eventType".into(), EventValue::Str(event_type));
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
