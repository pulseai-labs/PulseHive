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
                EventValue::Str(s) if s.len() > 30 => {
                    format!("{k}='{}'...", truncate_chars(s, 30))
                }
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

        // `HiveEvent` is `#[non_exhaustive]`: a variant the binding does not
        // know falls through every group mapper to `unknown_event`, which
        // preserves it rather than discarding it.
        let (event_type, agent_id) = match map_agent(&event, &mut fields)
            .or_else(|| map_llm(&event, &mut fields))
            .or_else(|| map_tool(&event, &mut fields))
            .or_else(|| map_substrate(&event, &mut fields))
            .or_else(|| map_infrastructure(&event, &mut fields))
        {
            Some((tag, agent_id)) => (tag.to_string(), agent_id),
            None => unknown_event(&event, &mut fields),
        };

        Self {
            event_type,
            agent_id,
            fields,
        }
    }
}

/// Maps the agent-lifecycle events (`AgentStarted`, `AgentCompleted`); the
/// outcome payload is filled in by [`map_outcome`]. Returns `None` when
/// `event` belongs to another group.
fn map_agent(
    event: &HiveEvent,
    fields: &mut HashMap<String, EventValue>,
) -> Option<(&'static str, Option<String>)> {
    let (tag, agent_id) = match event {
        HiveEvent::AgentStarted {
            timestamp_ms,
            agent_id,
            name,
            kind,
            collective_id,
            task_description,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert("name".into(), EventValue::Str(name.clone()));
            fields.insert("kind".into(), EventValue::Str(format!("{kind:?}")));
            fields.insert(
                "collectiveId".into(),
                EventValue::Str(collective_id.to_string()),
            );
            fields.insert(
                "taskDescription".into(),
                EventValue::Str(task_description.clone()),
            );
            ("agent_started", Some(agent_id.clone()))
        }
        HiveEvent::AgentCompleted {
            timestamp_ms,
            agent_id,
            outcome,
            collective_id,
            task_description,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert(
                "collectiveId".into(),
                EventValue::Str(collective_id.to_string()),
            );
            fields.insert(
                "taskDescription".into(),
                EventValue::Str(task_description.clone()),
            );
            map_outcome(fields, outcome);
            ("agent_completed", Some(agent_id.clone()))
        }
        _ => return None,
    };
    Some((tag, agent_id))
}

/// Maps an `AgentOutcome` into the `outcome` discriminator plus its payload
/// fields. `AgentOutcome` is `#[non_exhaustive]` (ADR-014): future variants
/// surface as an inert `unknown` outcome.
fn map_outcome(fields: &mut HashMap<String, EventValue>, outcome: &AgentOutcome) {
    match outcome {
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
        AgentOutcome::Cancelled { partial_response } => {
            fields.insert("outcome".into(), EventValue::Str("cancelled".into()));
            fields.insert(
                "partialResponse".into(),
                EventValue::Str(partial_response.clone()),
            );
        }
        AgentOutcome::PartialComplete { responses, errors } => {
            fields.insert("outcome".into(), EventValue::Str("partial_complete".into()));
            // No list variant in EventValue — same JSON-string
            // convention as the ToolProgress `progress` field.
            fields.insert(
                "responses".into(),
                EventValue::Str(serde_json::to_string(responses).unwrap_or_default()),
            );
            fields.insert(
                "errors".into(),
                EventValue::Str(serde_json::to_string(errors).unwrap_or_default()),
            );
        }
        _ => {
            fields.insert("outcome".into(), EventValue::Str("unknown".into()));
        }
    }
}

/// Maps the LLM-call events (`LlmCallStarted`, `LlmCallCompleted`,
/// `LlmTokenStreamed`). Returns `None` for events in other groups.
fn map_llm(
    event: &HiveEvent,
    fields: &mut HashMap<String, EventValue>,
) -> Option<(&'static str, Option<String>)> {
    let (tag, agent_id) = match event {
        HiveEvent::LlmCallStarted {
            timestamp_ms,
            agent_id,
            model,
            message_count,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert("model".into(), EventValue::Str(model.clone()));
            fields.insert(
                "messageCount".into(),
                EventValue::Num(*message_count as u64),
            );
            ("llm_call_started", Some(agent_id.clone()))
        }
        HiveEvent::LlmCallCompleted {
            timestamp_ms,
            agent_id,
            model,
            duration_ms,
            input_tokens,
            output_tokens,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert("model".into(), EventValue::Str(model.clone()));
            fields.insert("durationMs".into(), EventValue::Num(*duration_ms));
            fields.insert("inputTokens".into(), EventValue::Num(*input_tokens as u64));
            fields.insert(
                "outputTokens".into(),
                EventValue::Num(*output_tokens as u64),
            );
            ("llm_call_completed", Some(agent_id.clone()))
        }
        HiveEvent::LlmTokenStreamed {
            timestamp_ms,
            agent_id,
            token,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert("token".into(), EventValue::Str(token.clone()));
            ("llm_token_streamed", Some(agent_id.clone()))
        }
        _ => return None,
    };
    Some((tag, agent_id))
}

/// Maps the tool events (`ToolCallStarted`, `ToolCallCompleted`,
/// `ToolApprovalRequested`, `ToolProgress`). Returns `None` for events in
/// other groups.
fn map_tool(
    event: &HiveEvent,
    fields: &mut HashMap<String, EventValue>,
) -> Option<(&'static str, Option<String>)> {
    let (tag, agent_id) = match event {
        HiveEvent::ToolCallStarted {
            timestamp_ms,
            agent_id,
            tool_name,
            params,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert("toolName".into(), EventValue::Str(tool_name.clone()));
            fields.insert("params".into(), EventValue::Str(params.clone()));
            ("tool_call_started", Some(agent_id.clone()))
        }
        HiveEvent::ToolCallCompleted {
            timestamp_ms,
            agent_id,
            tool_name,
            duration_ms,
            result_preview,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert("toolName".into(), EventValue::Str(tool_name.clone()));
            fields.insert("durationMs".into(), EventValue::Num(*duration_ms));
            fields.insert(
                "resultPreview".into(),
                EventValue::Str(result_preview.clone()),
            );
            ("tool_call_completed", Some(agent_id.clone()))
        }
        HiveEvent::ToolApprovalRequested {
            timestamp_ms,
            agent_id,
            tool_name,
            description,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert("toolName".into(), EventValue::Str(tool_name.clone()));
            fields.insert("description".into(), EventValue::Str(description.clone()));
            ("tool_approval_requested", Some(agent_id.clone()))
        }
        HiveEvent::ToolProgress {
            timestamp_ms,
            agent_id,
            tool_name,
            progress,
        } => map_tool_progress(fields, *timestamp_ms, agent_id, tool_name, progress),
        _ => return None,
    };
    Some((tag, agent_id))
}

/// Maps a `ToolProgress` event's fields and returns its tag and agent id.
/// The nested `progress` enum has no scalar map representation, so this emits
/// a `progressKind` discriminator plus the full payload as a JSON string
/// (audit ⑥ default; the flatten-scalars form is deferred).
fn map_tool_progress(
    fields: &mut HashMap<String, EventValue>,
    timestamp_ms: u64,
    agent_id: &str,
    tool_name: &str,
    progress: &ToolProgress,
) -> (&'static str, Option<String>) {
    fields.insert("timestampMs".into(), EventValue::Num(timestamp_ms));
    fields.insert("agentId".into(), EventValue::Str(agent_id.to_string()));
    fields.insert("toolName".into(), EventValue::Str(tool_name.to_string()));
    let progress_kind = match progress {
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
        EventValue::Str(serde_json::to_string(progress).unwrap_or_default()),
    );
    ("tool_progress", Some(agent_id.to_string()))
}

/// Maps the substrate events (`ExperienceRecorded`, `RelationshipInferred`,
/// `InsightGenerated`, `SubstratePerceived`). Returns `None` for events in
/// other groups.
fn map_substrate(
    event: &HiveEvent,
    fields: &mut HashMap<String, EventValue>,
) -> Option<(&'static str, Option<String>)> {
    let (tag, agent_id) = match event {
        HiveEvent::ExperienceRecorded {
            timestamp_ms,
            experience_id,
            agent_id,
            content_preview,
            experience_type,
            importance,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert(
                "experienceId".into(),
                EventValue::Str(experience_id.to_string()),
            );
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert(
                "contentPreview".into(),
                EventValue::Str(content_preview.clone()),
            );
            fields.insert(
                "experienceType".into(),
                EventValue::Str(experience_type.clone()),
            );
            fields.insert("importance".into(), EventValue::Float(*importance as f64));
            ("experience_recorded", Some(agent_id.clone()))
        }
        HiveEvent::RelationshipInferred {
            timestamp_ms,
            relation_id,
            agent_id,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert(
                "relationId".into(),
                EventValue::Str(relation_id.to_string()),
            );
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            ("relationship_inferred", Some(agent_id.clone()))
        }
        HiveEvent::InsightGenerated {
            timestamp_ms,
            insight_id,
            source_count,
            agent_id,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("insightId".into(), EventValue::Str(insight_id.to_string()));
            fields.insert("sourceCount".into(), EventValue::Num(*source_count as u64));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            ("insight_generated", Some(agent_id.clone()))
        }
        HiveEvent::SubstratePerceived {
            timestamp_ms,
            agent_id,
            experience_count,
            insight_count,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert(
                "experienceCount".into(),
                EventValue::Num(*experience_count as u64),
            );
            fields.insert(
                "insightCount".into(),
                EventValue::Num(*insight_count as u64),
            );
            ("substrate_perceived", Some(agent_id.clone()))
        }
        _ => return None,
    };
    Some((tag, agent_id))
}

/// Maps the infrastructure events (`EmbeddingComputed`, `WatchNotification`).
/// Returns `None` for events in other groups.
fn map_infrastructure(
    event: &HiveEvent,
    fields: &mut HashMap<String, EventValue>,
) -> Option<(&'static str, Option<String>)> {
    let (tag, agent_id) = match event {
        HiveEvent::EmbeddingComputed {
            timestamp_ms,
            agent_id,
            dimensions,
            duration_ms,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert("agentId".into(), EventValue::Str(agent_id.clone()));
            fields.insert("dimensions".into(), EventValue::Num(*dimensions as u64));
            fields.insert("durationMs".into(), EventValue::Num(*duration_ms));
            ("embedding_computed", Some(agent_id.clone()))
        }
        HiveEvent::WatchNotification {
            timestamp_ms,
            experience_id,
            collective_id,
            event_type,
        } => {
            fields.insert("timestampMs".into(), EventValue::Num(*timestamp_ms));
            fields.insert(
                "experienceId".into(),
                EventValue::Str(experience_id.to_string()),
            );
            fields.insert(
                "collectiveId".into(),
                EventValue::Str(collective_id.to_string()),
            );
            fields.insert("eventType".into(), EventValue::Str(event_type.clone()));
            ("watch_notification", None)
        }
        _ => return None,
    };
    Some((tag, agent_id))
}

/// Maps an event variant the binding does not know: the serde `type` tag
/// becomes `event_type` and the full event, serialized with serde, travels
/// under `raw` as a JSON string — the same JSON-string convention as the
/// `progress` field.
fn unknown_event(
    event: &HiveEvent,
    fields: &mut HashMap<String, EventValue>,
) -> (String, Option<String>) {
    let raw = serde_json::to_value(event).unwrap_or_default();
    let event_type = raw
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown")
        .to_string();
    fields.insert("raw".into(), EventValue::Str(raw.to_string()));
    (event_type, None)
}

/// Truncates to at most `max_bytes`, cutting at the nearest UTF-8
/// character boundary so multibyte content never panics the debug
/// renderer.
pub(crate) fn truncate_chars(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsehive_core::agent::AgentKindTag;
    use pulsehive_core::prelude::CollectiveId;

    #[test]
    fn to_string_js_never_panics_on_multibyte_fields() {
        // Eight four-byte emoji: byte 30 falls mid-character.
        let event = HiveEvent::AgentStarted {
            timestamp_ms: 1,
            agent_id: "a1".into(),
            name: "emoji-agent".into(),
            kind: AgentKindTag::Llm,
            collective_id: CollectiveId::new(),
            task_description: "😀".repeat(8),
        };
        let js_event = JsHiveEvent::from(event);
        // The panic the regression guards against is the slice itself; the
        // field ordering in the HashMap preview is arbitrary, so only the
        // shape of the rendering is asserted.
        let rendered = js_event.to_string_js();
        assert!(rendered.starts_with("HiveEvent(agent_started"));
    }

    #[test]
    fn truncate_chars_floors_to_char_boundary() {
        assert_eq!(truncate_chars("hello", 30), "hello");
        assert_eq!(truncate_chars("😀😀😀", 5), "😀");
    }

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
