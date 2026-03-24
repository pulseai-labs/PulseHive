//! Event system for real-time observability into agent execution.
//!
//! [`HiveEvent`] covers the full agent lifecycle: LLM calls, tool execution,
//! substrate operations, and perception. [`EventEmitter`] provides a
//! fire-and-forget broadcast mechanism for event distribution.
//!
//! Built on `tokio::sync::broadcast` for multi-consumer support.

use pulsedb::{ExperienceId, InsightId, RelationId};
use tokio::sync::broadcast;

use crate::agent::{AgentKindTag, AgentOutcome};

/// Events emitted during agent execution.
///
/// Covers the full lifecycle: agent start/stop, LLM calls, tool execution,
/// substrate operations, and perception. All variants include `agent_id`
/// where applicable for correlation.
///
/// Must be `Clone` because [`EventEmitter`] uses `tokio::sync::broadcast`
/// which requires cloneable values.
#[derive(Debug, Clone)]
pub enum HiveEvent {
    // ── Agent lifecycle ──────────────────────────────────────────────
    /// An agent has started execution.
    AgentStarted {
        agent_id: String,
        name: String,
        kind: AgentKindTag,
    },
    /// An agent has completed execution.
    AgentCompleted {
        agent_id: String,
        outcome: AgentOutcome,
    },

    // ── LLM interactions ─────────────────────────────────────────────
    /// An LLM call has been initiated.
    LlmCallStarted {
        agent_id: String,
        model: String,
        message_count: usize,
    },
    /// An LLM call has completed.
    LlmCallCompleted {
        agent_id: String,
        model: String,
        duration_ms: u64,
    },
    /// A token was received from a streaming LLM response.
    LlmTokenStreamed { agent_id: String, token: String },

    // ── Tool execution ───────────────────────────────────────────────
    /// A tool call has started.
    ToolCallStarted { agent_id: String, tool_name: String },
    /// A tool call has completed.
    ToolCallCompleted {
        agent_id: String,
        tool_name: String,
        duration_ms: u64,
    },
    /// A tool requires human approval before execution.
    ToolApprovalRequested {
        agent_id: String,
        tool_name: String,
        description: String,
    },

    // ── Substrate operations ─────────────────────────────────────────
    /// An experience was recorded in the substrate.
    ExperienceRecorded {
        experience_id: ExperienceId,
        agent_id: String,
    },
    /// A relationship was inferred between experiences.
    RelationshipInferred { relation_id: RelationId },
    /// An insight was synthesized from an experience cluster.
    InsightGenerated {
        insight_id: InsightId,
        source_count: usize,
    },

    // ── Perception ───────────────────────────────────────────────────
    /// An agent perceived the substrate through its lens.
    SubstratePerceived {
        agent_id: String,
        experience_count: usize,
        insight_count: usize,
    },

    // ── Embedding ─────────────────────────────────────────────────
    /// An embedding was computed via the EmbeddingProvider.
    EmbeddingComputed {
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
        experience_id: ExperienceId,
        collective_id: pulsedb::CollectiveId,
        /// The type of change: "Created", "Updated", "Archived", or "Deleted".
        event_type: String,
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
}

impl EventEmitter {
    /// Creates a new emitter with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Emits an event to all subscribers. Fire-and-forget — if no
    /// subscribers exist, the event is silently dropped.
    pub fn emit(&self, event: HiveEvent) {
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
            agent_id: "a1".into(),
            name: "researcher".into(),
            kind: AgentKindTag::Llm,
        };
        let cloned = event.clone();
        let debug = format!("{:?}", cloned);
        assert!(debug.contains("researcher"));
    }

    #[tokio::test]
    async fn test_event_emitter_send_receive() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(HiveEvent::AgentStarted {
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
            agent_id: "a1".into(),
            tool_name: "search".into(),
        });

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert!(matches!(e1, HiveEvent::ToolCallStarted { .. }));
        assert!(matches!(e2, HiveEvent::ToolCallStarted { .. }));
    }

    #[test]
    fn test_event_emitter_no_subscribers_no_panic() {
        let emitter = EventEmitter::new(16);
        // Should not panic even with no subscribers
        emitter.emit(HiveEvent::ExperienceRecorded {
            experience_id: ExperienceId::new(),
            agent_id: "a1".into(),
        });
    }

    #[test]
    fn test_event_emitter_clone_is_cheap() {
        let emitter = EventEmitter::default();
        let cloned = emitter.clone();
        // Both share the same channel
        let mut rx = cloned.subscribe();
        emitter.emit(HiveEvent::SubstratePerceived {
            agent_id: "a1".into(),
            experience_count: 10,
            insight_count: 2,
        });
        // rx should receive the event emitted via the original
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
        // Verify every variant can be cloned (compile-time check)
        let events: Vec<HiveEvent> = vec![
            HiveEvent::AgentStarted {
                agent_id: "a".into(),
                name: "n".into(),
                kind: AgentKindTag::Llm,
            },
            HiveEvent::AgentCompleted {
                agent_id: "a".into(),
                outcome: AgentOutcome::Complete {
                    response: "done".into(),
                },
            },
            HiveEvent::LlmCallStarted {
                agent_id: "a".into(),
                model: "gpt-4".into(),
                message_count: 3,
            },
            HiveEvent::LlmCallCompleted {
                agent_id: "a".into(),
                model: "gpt-4".into(),
                duration_ms: 1500,
            },
            HiveEvent::LlmTokenStreamed {
                agent_id: "a".into(),
                token: "hello".into(),
            },
            HiveEvent::ToolCallStarted {
                agent_id: "a".into(),
                tool_name: "search".into(),
            },
            HiveEvent::ToolCallCompleted {
                agent_id: "a".into(),
                tool_name: "search".into(),
                duration_ms: 200,
            },
            HiveEvent::ToolApprovalRequested {
                agent_id: "a".into(),
                tool_name: "delete".into(),
                description: "Delete file".into(),
            },
            HiveEvent::ExperienceRecorded {
                experience_id: ExperienceId::new(),
                agent_id: "a".into(),
            },
            HiveEvent::RelationshipInferred {
                relation_id: RelationId::new(),
            },
            HiveEvent::InsightGenerated {
                insight_id: InsightId::new(),
                source_count: 5,
            },
            HiveEvent::SubstratePerceived {
                agent_id: "a".into(),
                experience_count: 10,
                insight_count: 2,
            },
            HiveEvent::WatchNotification {
                experience_id: ExperienceId::new(),
                collective_id: pulsedb::CollectiveId::new(),
                event_type: "Created".into(),
            },
        ];
        // Clone all — if any variant isn't Clone, this won't compile
        let _cloned: Vec<HiveEvent> = events.to_vec();
        assert_eq!(events.len(), 13);
    }
}
