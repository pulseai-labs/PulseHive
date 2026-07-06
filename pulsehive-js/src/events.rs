//! Event type binding: HiveEvent.
//!
//! Uses a tagged-class pattern: `.eventType` returns a string discriminator,
//! `.data` returns a plain object with variant-specific fields.

use std::collections::HashMap;

use pulsehive_core::agent::AgentOutcome;
use pulsehive_core::event::HiveEvent;
use pulsehive_core::tool::ToolProgress;

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
    Float(f64),
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
                    EventValue::Float(f) => f.to_string(),
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
                EventValue::Float(f) => format!("{k}={f}"),
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
                timestamp_ms,
                agent_id,
                name,
                kind,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("name".into(), EventValue::Str(name));
                fields.insert("kind".into(), EventValue::Str(format!("{kind:?}")));
                ("agent_started", Some(agent_id))
            }
            HiveEvent::AgentCompleted {
                timestamp_ms,
                agent_id,
                outcome,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
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
                timestamp_ms,
                agent_id,
                model,
                message_count,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("model".into(), EventValue::Str(model));
                fields.insert("messageCount".into(), EventValue::Num(message_count as u64));
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
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("model".into(), EventValue::Str(model));
                fields.insert("durationMs".into(), EventValue::Num(duration_ms));
                fields.insert("inputTokens".into(), EventValue::Num(input_tokens as u64));
                fields.insert("outputTokens".into(), EventValue::Num(output_tokens as u64));
                ("llm_call_completed", Some(agent_id))
            }
            HiveEvent::LlmTokenStreamed {
                timestamp_ms,
                agent_id,
                token,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("token".into(), EventValue::Str(token));
                ("llm_token_streamed", Some(agent_id))
            }
            HiveEvent::ToolCallStarted {
                timestamp_ms,
                agent_id,
                tool_name,
                params,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("toolName".into(), EventValue::Str(tool_name));
                fields.insert("params".into(), EventValue::Str(params));
                ("tool_call_started", Some(agent_id))
            }
            HiveEvent::ToolCallCompleted {
                timestamp_ms,
                agent_id,
                tool_name,
                duration_ms,
                result_preview,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("toolName".into(), EventValue::Str(tool_name));
                fields.insert("durationMs".into(), EventValue::Num(duration_ms));
                fields.insert("resultPreview".into(), EventValue::Str(result_preview));
                ("tool_call_completed", Some(agent_id))
            }
            HiveEvent::ToolApprovalRequested {
                timestamp_ms,
                agent_id,
                tool_name,
                description,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("toolName".into(), EventValue::Str(tool_name));
                fields.insert("description".into(), EventValue::Str(description));
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
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert(
                    "experienceId".into(),
                    EventValue::Str(experience_id.to_string()),
                );
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("contentPreview".into(), EventValue::Str(content_preview));
                fields.insert("experienceType".into(), EventValue::Str(experience_type));
                fields.insert("importance".into(), EventValue::Float(importance as f64));
                ("experience_recorded", Some(agent_id))
            }
            HiveEvent::RelationshipInferred {
                timestamp_ms,
                relation_id,
                agent_id,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert(
                    "relationId".into(),
                    EventValue::Str(relation_id.to_string()),
                );
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                ("relationship_inferred", Some(agent_id))
            }
            HiveEvent::InsightGenerated {
                timestamp_ms,
                insight_id,
                source_count,
                agent_id,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("insightId".into(), EventValue::Str(insight_id.to_string()));
                fields.insert("sourceCount".into(), EventValue::Num(source_count as u64));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                ("insight_generated", Some(agent_id))
            }
            HiveEvent::SubstratePerceived {
                timestamp_ms,
                agent_id,
                experience_count,
                insight_count,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert(
                    "experienceCount".into(),
                    EventValue::Num(experience_count as u64),
                );
                fields.insert("insightCount".into(), EventValue::Num(insight_count as u64));
                ("substrate_perceived", Some(agent_id))
            }
            HiveEvent::EmbeddingComputed {
                timestamp_ms,
                agent_id,
                dimensions,
                duration_ms,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("dimensions".into(), EventValue::Num(dimensions as u64));
                fields.insert("durationMs".into(), EventValue::Num(duration_ms));
                ("embedding_computed", Some(agent_id))
            }
            HiveEvent::WatchNotification {
                timestamp_ms,
                experience_id,
                collective_id,
                event_type,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
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
            HiveEvent::ToolProgress {
                timestamp_ms,
                agent_id,
                tool_name,
                progress,
            } => {
                fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
                fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
                fields.insert("toolName".into(), EventValue::Str(tool_name));
                // The nested `progress` enum has no scalar map representation, so
                // emit a `progressKind` discriminator plus the full payload as a
                // JSON string (audit ⑥ default; the flatten-scalars form is deferred).
                let progress_kind = match &progress {
                    ToolProgress::Started { .. } => "started",
                    ToolProgress::Progress { .. } => "progress",
                    ToolProgress::PartialResult { .. } => "partial_result",
                    ToolProgress::Log { .. } => "log",
                    ToolProgress::Completed { .. } => "completed",
                };
                fields.insert(
                    "progressKind".into(),
                    EventValue::Str(progress_kind.to_string()),
                );
                fields.insert(
                    "progress".into(),
                    EventValue::Str(serde_json::to_string(&progress).unwrap_or_default()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jshiveevent_tool_progress_maps_event_type() {
        let event = HiveEvent::ToolProgress {
            timestamp_ms: 1,
            agent_id: "a1".into(),
            tool_name: "backtest".into(),
            progress: ToolProgress::Progress {
                fraction: 0.5,
                message: Some("halfway".into()),
            },
        };
        let js_event = JsHiveEvent::from(event);
        assert_eq!(js_event.event_type, "tool_progress");
        assert_eq!(js_event.agent_id.as_deref(), Some("a1"));
        // Nested payload is serialized as a JSON string + a discriminator field.
        assert!(js_event.fields.contains_key("progress"));
        assert!(matches!(
            js_event.fields.get("progressKind"),
            Some(EventValue::Str(s)) if s == "progress"
        ));
    }
}
