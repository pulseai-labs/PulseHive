//! Event system for real-time observability into agent execution.
//!
//! [`HiveEvent`] covers the full agent lifecycle: LLM calls, tool execution,
//! substrate operations, and perception. [`EventEmitter`] provides a
//! fire-and-forget broadcast mechanism for event distribution.
//!
//! Events are serializable to JSON for transmission to observability tools
//! like PulseVision via [`EventExporter`](crate::export::EventExporter).
//!
//! Built on `tokio::sync::broadcast` for multi-consumer support.

use std::sync::Arc;

use pulsedb::{ExperienceId, InsightId, RelationId};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::agent::{AgentKindTag, AgentOutcome};
use crate::export::EventExporter;

/// Returns the current time as epoch milliseconds.
///
/// Used by event emitters to timestamp events at creation time.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Events emitted during agent execution.
///
/// Covers the full lifecycle: agent start/stop, LLM calls, tool execution,
/// substrate operations, and perception. All variants include `timestamp_ms`
/// (epoch milliseconds) and `agent_id` where applicable for correlation.
///
/// Serializes to tagged JSON: `{"type": "llm_call_completed", "agent_id": "...", ...}`
///
/// Must be `Clone` because [`EventEmitter`] uses `tokio::sync::broadcast`
/// which requires cloneable values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HiveEvent {
    // ── Agent lifecycle ──────────────────────────────────────────────
    /// An agent has started execution.
    AgentStarted {
        timestamp_ms: u64,
        agent_id: String,
        name: String,
        kind: AgentKindTag,
    },
    /// An agent has completed execution.
    AgentCompleted {
        timestamp_ms: u64,
        agent_id: String,
        outcome: AgentOutcome,
    },

    // ── LLM interactions ─────────────────────────────────────────────
    /// An LLM call has been initiated.
    LlmCallStarted {
        timestamp_ms: u64,
        agent_id: String,
        model: String,
        message_count: usize,
    },
    /// An LLM call has completed.
    LlmCallCompleted {
        timestamp_ms: u64,
        agent_id: String,
        model: String,
        duration_ms: u64,
        /// Prompt tokens consumed (0 if not reported by provider).
        input_tokens: u32,
        /// Completion tokens generated (0 if not reported by provider).
        output_tokens: u32,
    },
    /// A token was received from a streaming LLM response.
    LlmTokenStreamed {
        timestamp_ms: u64,
        agent_id: String,
        token: String,
    },

    // ── Tool execution ───────────────────────────────────────────────
    /// A tool call has started.
    ToolCallStarted {
        timestamp_ms: u64,
        agent_id: String,
        tool_name: String,
        /// Tool arguments as a JSON string.
        params: String,
    },
    /// A tool call has completed.
    ToolCallCompleted {
        timestamp_ms: u64,
        agent_id: String,
        tool_name: String,
        duration_ms: u64,
        /// Tool result preview (truncated to 200 chars).
        result_preview: String,
    },
    /// A tool requires human approval before execution.
    ToolApprovalRequested {
        timestamp_ms: u64,
        agent_id: String,
        tool_name: String,
        description: String,
    },

    // ── Substrate operations ─────────────────────────────────────────
    /// An experience was recorded in the substrate.
    ExperienceRecorded {
        timestamp_ms: u64,
        experience_id: ExperienceId,
        agent_id: String,
        /// First 200 characters of the experience content.
        content_preview: String,
        /// Experience type as a string (e.g., "Generic", "Solution").
        experience_type: String,
        /// Importance score (0.0-1.0).
        importance: f32,
    },
    /// A relationship was inferred between experiences.
    RelationshipInferred {
        timestamp_ms: u64,
        relation_id: RelationId,
        /// Agent that triggered the inference.
        agent_id: String,
    },
    /// An insight was synthesized from an experience cluster.
    InsightGenerated {
        timestamp_ms: u64,
        insight_id: InsightId,
        source_count: usize,
        /// Agent that triggered the synthesis.
        agent_id: String,
    },

    // ── Perception ───────────────────────────────────────────────────
    /// An agent perceived the substrate through its lens.
    SubstratePerceived {
        timestamp_ms: u64,
        agent_id: String,
        experience_count: usize,
        insight_count: usize,
    },

    // ── Embedding ─────────────────────────────────────────────────
    /// An embedding was computed via the EmbeddingProvider.
    EmbeddingComputed {
        timestamp_ms: u64,
        agent_id: String,
        dimensions: usize,
        duration_ms: u64,
    },

    // ── Watch system ───────────────────────────────────────────────
    /// A real-time Watch event from the substrate.
    ///
    /// Emitted when experiences are created, updated, archived, or deleted
    /// by other agents in the same collective. Forwarded from PulseDB's
    /// Watch system into the HiveEvent stream.
    WatchNotification {
        timestamp_ms: u64,
        experience_id: ExperienceId,
        collective_id: pulsedb::CollectiveId,
        /// The type of change: "Created", "Updated", "Archived", or "Deleted".
        event_type: String,
    },

    // ── Streaming tool progress ──────────────────────────────────────
    /// A streaming tool emitted a progress event during execution.
    ///
    /// Carries a [`ToolProgress`](crate::tool::ToolProgress) payload
    /// (`Started` / `Progress` / `PartialResult` / `Log` / `Completed`). The
    /// agent loop (v2.1.0) forwards each `ToolProgress` pushed by a
    /// [`StreamingTool`](crate::tool::StreamingTool) as one of these events.
    ///
    /// Serializes with the outer tag `"type":"tool_progress"`; the nested
    /// `progress` retains its own `"kind"` tag.
    ToolProgress {
        timestamp_ms: u64,
        agent_id: String,
        tool_name: String,
        progress: crate::tool::ToolProgress,
    },
}

/// Fire-and-forget event broadcaster.
///
/// Wraps `tokio::sync::broadcast` for multi-consumer event distribution.
/// If no subscribers exist, emitted events are silently dropped.
///
/// `Clone` is cheap — it just clones the broadcast sender handle.
#[derive(Clone)]
pub struct EventEmitter {
    sender: broadcast::Sender<HiveEvent>,
    exporter: Option<Arc<dyn EventExporter>>,
}

impl EventEmitter {
    /// Creates a new emitter with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            exporter: None,
        }
    }

    /// Creates a new emitter with an event exporter for external observability.
    ///
    /// When set, every emitted event is also forwarded to the exporter
    /// via a fire-and-forget `tokio::spawn` — zero latency on the emit path.
    pub fn with_exporter(capacity: usize, exporter: Arc<dyn EventExporter>) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            exporter: Some(exporter),
        }
    }

    /// Emits an event to all subscribers and the exporter (if set).
    /// Fire-and-forget — if no subscribers exist, the event is silently dropped.
    pub fn emit(&self, event: HiveEvent) {
        if let Some(exporter) = &self.exporter {
            let exporter = Arc::clone(exporter);
            let event_clone = event.clone();
            tokio::spawn(async move {
                exporter.export(&event_clone).await;
            });
        }
        let _ = self.sender.send(event);
    }

    /// Creates a new subscriber that receives all future events.
    pub fn subscribe(&self) -> broadcast::Receiver<HiveEvent> {
        self.sender.subscribe()
    }
}

impl Default for EventEmitter {
    fn default() -> Self {
        Self::new(256)
    }
}

impl std::fmt::Debug for EventEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventEmitter")
            .field("subscriber_count", &self.sender.receiver_count())
            .finish()
    }
}

/// Type alias for event fan-out. In Sprint 1, this is just an EventEmitter.
/// May evolve into a more sophisticated bus with filtering in later sprints.
pub type EventBus = EventEmitter;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hive_event_is_debug_clone() {
        let event = HiveEvent::AgentStarted {
            timestamp_ms: now_ms(),
            agent_id: "a1".into(),
            name: "researcher".into(),
            kind: AgentKindTag::Llm,
        };
        let cloned = event.clone();
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("researcher"));
    }

    #[test]
    fn test_hive_event_serializes_to_json() {
        let event = HiveEvent::LlmCallCompleted {
            timestamp_ms: 1711500000000,
            agent_id: "agent-1".into(),
            model: "gpt-4".into(),
            duration_ms: 1500,
            input_tokens: 200,
            output_tokens: 50,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"llm_call_completed\""));
        assert!(json.contains("\"input_tokens\":200"));
        assert!(json.contains("\"output_tokens\":50"));

        // Roundtrip
        let deserialized: HiveEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            HiveEvent::LlmCallCompleted {
                input_tokens: 200,
                output_tokens: 50,
                ..
            }
        ));
    }

    #[test]
    fn test_hive_event_serialize_tool_call() {
        let event = HiveEvent::ToolCallStarted {
            timestamp_ms: now_ms(),
            agent_id: "a1".into(),
            tool_name: "search".into(),
            params: r#"{"query":"test"}"#.into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"params\""));
        assert!(json.contains("\"tool_call_started\""));
    }

    #[test]
    fn test_hive_event_serialize_tool_progress() {
        let event = HiveEvent::ToolProgress {
            timestamp_ms: 1711500000000,
            agent_id: "agent-1".into(),
            tool_name: "backtest".into(),
            progress: crate::tool::ToolProgress::Progress {
                fraction: 0.5,
                message: Some("halfway".into()),
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        // Outer HiveEvent tag.
        assert!(json.contains("\"type\":\"tool_progress\""));
        // Nested ToolProgress keeps its own `kind` tag.
        assert!(json.contains("\"kind\":\"progress\""));

        // Roundtrip preserves the nested variant.
        let deserialized: HiveEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            HiveEvent::ToolProgress {
                progress: crate::tool::ToolProgress::Progress { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn test_event_emitter_send_receive() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(HiveEvent::AgentStarted {
            timestamp_ms: now_ms(),
            agent_id: "a1".into(),
            name: "test".into(),
            kind: AgentKindTag::Llm,
        });

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, HiveEvent::AgentStarted { agent_id, .. } if agent_id == "a1"));
    }

    #[tokio::test]
    async fn test_event_emitter_multiple_subscribers() {
        let emitter = EventEmitter::new(16);
        let mut rx1 = emitter.subscribe();
        let mut rx2 = emitter.subscribe();

        emitter.emit(HiveEvent::ToolCallStarted {
            timestamp_ms: now_ms(),
            agent_id: "a1".into(),
            tool_name: "search".into(),
            params: "{}".into(),
        });

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert!(matches!(e1, HiveEvent::ToolCallStarted { .. }));
        assert!(matches!(e2, HiveEvent::ToolCallStarted { .. }));
    }

    #[test]
    fn test_event_emitter_no_subscribers_no_panic() {
        let emitter = EventEmitter::new(16);
        emitter.emit(HiveEvent::ExperienceRecorded {
            timestamp_ms: now_ms(),
            experience_id: ExperienceId::new(),
            agent_id: "a1".into(),
            content_preview: "test".into(),
            experience_type: "Generic".into(),
            importance: 0.5,
        });
    }

    #[test]
    fn test_event_emitter_clone_is_cheap() {
        let emitter = EventEmitter::default();
        let cloned = emitter.clone();
        let mut rx = cloned.subscribe();
        emitter.emit(HiveEvent::SubstratePerceived {
            timestamp_ms: now_ms(),
            agent_id: "a1".into(),
            experience_count: 10,
            insight_count: 2,
        });
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn test_event_emitter_debug() {
        let emitter = EventEmitter::default();
        let debug = format!("{:?}", emitter);
        assert!(debug.contains("EventEmitter"));
    }

    #[test]
    fn test_all_event_variants_clone() {
        let events: Vec<HiveEvent> = vec![
            HiveEvent::AgentStarted {
                timestamp_ms: 0,
                agent_id: "a".into(),
                name: "n".into(),
                kind: AgentKindTag::Llm,
            },
            HiveEvent::AgentCompleted {
                timestamp_ms: 0,
                agent_id: "a".into(),
                outcome: AgentOutcome::Complete {
                    response: "done".into(),
                },
            },
            HiveEvent::LlmCallStarted {
                timestamp_ms: 0,
                agent_id: "a".into(),
                model: "gpt-4".into(),
                message_count: 3,
            },
            HiveEvent::LlmCallCompleted {
                timestamp_ms: 0,
                agent_id: "a".into(),
                model: "gpt-4".into(),
                duration_ms: 1500,
                input_tokens: 100,
                output_tokens: 50,
            },
            HiveEvent::LlmTokenStreamed {
                timestamp_ms: 0,
                agent_id: "a".into(),
                token: "hello".into(),
            },
            HiveEvent::ToolCallStarted {
                timestamp_ms: 0,
                agent_id: "a".into(),
                tool_name: "search".into(),
                params: "{}".into(),
            },
            HiveEvent::ToolCallCompleted {
                timestamp_ms: 0,
                agent_id: "a".into(),
                tool_name: "search".into(),
                duration_ms: 200,
                result_preview: "found it".into(),
            },
            HiveEvent::ToolApprovalRequested {
                timestamp_ms: 0,
                agent_id: "a".into(),
                tool_name: "delete".into(),
                description: "Delete file".into(),
            },
            HiveEvent::ExperienceRecorded {
                timestamp_ms: 0,
                experience_id: ExperienceId::new(),
                agent_id: "a".into(),
                content_preview: "test".into(),
                experience_type: "Generic".into(),
                importance: 0.5,
            },
            HiveEvent::RelationshipInferred {
                timestamp_ms: 0,
                relation_id: RelationId::new(),
                agent_id: "a".into(),
            },
            HiveEvent::InsightGenerated {
                timestamp_ms: 0,
                insight_id: InsightId::new(),
                source_count: 5,
                agent_id: "a".into(),
            },
            HiveEvent::SubstratePerceived {
                timestamp_ms: 0,
                agent_id: "a".into(),
                experience_count: 10,
                insight_count: 2,
            },
            HiveEvent::EmbeddingComputed {
                timestamp_ms: 0,
                agent_id: "a".into(),
                dimensions: 384,
                duration_ms: 100,
            },
            HiveEvent::WatchNotification {
                timestamp_ms: 0,
                experience_id: ExperienceId::new(),
                collective_id: pulsedb::CollectiveId::new(),
                event_type: "Created".into(),
            },
            HiveEvent::ToolProgress {
                timestamp_ms: 0,
                agent_id: "a".into(),
                tool_name: "search".into(),
                progress: crate::tool::ToolProgress::Progress {
                    fraction: 0.5,
                    message: Some("halfway".into()),
                },
            },
        ];
        let _cloned: Vec<HiveEvent> = events.to_vec();
        assert_eq!(events.len(), 15);
    }

    #[test]
    fn test_now_ms_returns_nonzero() {
        let ts = now_ms();
        assert!(ts > 0, "Timestamp should be non-zero");
        assert!(ts > 1_700_000_000_000, "Timestamp should be after 2023");
    }
}
