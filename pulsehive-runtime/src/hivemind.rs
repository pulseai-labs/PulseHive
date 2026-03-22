//! HiveMind orchestrator and builder.
//!
//! [`HiveMind`] is the central entry point of PulseHive. It owns the substrate,
//! LLM providers, approval handler, and event bus. Products construct it via
//! the builder pattern and deploy agents through it.
//!
//! # Example
//! ```rust,ignore
//! let hive = HiveMind::builder()
//!     .substrate_path("/tmp/my_project.db")
//!     .llm_provider("openai", my_openai_provider)
//!     .build()?;
//!
//! let events = hive.deploy(agents, tasks).await?;
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream;
use futures::{Stream, StreamExt};
use pulsedb::{
    CollectiveId, Config, ExperienceId, NewExperience, PulseDB, PulseDBSubstrate, SubstrateProvider,
};
use tokio::sync::broadcast;

use pulsehive_core::agent::AgentDefinition;
use pulsehive_core::approval::{ApprovalHandler, AutoApprove};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::{EventBus, HiveEvent};
use pulsehive_core::llm::LlmProvider;

use crate::intelligence::insight::InsightSynthesizer;
use crate::intelligence::relationship::RelationshipDetector;
use crate::workflow::{self, WorkflowContext};

/// A task to be executed by deployed agents.
#[derive(Debug, Clone)]
pub struct Task {
    /// Human-readable description of what to accomplish.
    pub description: String,
    /// Collective (namespace) this task operates within.
    pub collective_id: CollectiveId,
}

impl Task {
    /// Creates a task with a new collective ID.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            collective_id: CollectiveId::new(),
        }
    }

    /// Creates a task within an existing collective.
    pub fn with_collective(description: impl Into<String>, collective_id: CollectiveId) -> Self {
        Self {
            description: description.into(),
            collective_id,
        }
    }
}

/// The central orchestrator of PulseHive.
///
/// Owns the substrate, LLM providers, approval handler, and event bus.
/// Constructed exclusively via [`HiveMind::builder()`].
pub struct HiveMind {
    pub(crate) substrate: Arc<dyn SubstrateProvider>,
    pub(crate) llm_providers: HashMap<String, Arc<dyn LlmProvider>>,
    pub(crate) approval_handler: Arc<dyn ApprovalHandler>,
    pub(crate) event_bus: EventBus,
    pub(crate) relationship_detector: Option<RelationshipDetector>,
    pub(crate) insight_synthesizer: Option<InsightSynthesizer>,
}

impl std::fmt::Debug for HiveMind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HiveMind")
            .field(
                "llm_providers",
                &self.llm_providers.keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl HiveMind {
    /// Creates a new builder for constructing a HiveMind.
    pub fn builder() -> HiveMindBuilder {
        HiveMindBuilder::new()
    }

    /// Access the substrate provider for direct operations.
    pub fn substrate(&self) -> &dyn SubstrateProvider {
        self.substrate.as_ref()
    }

    /// Deploy agents to execute tasks. Returns a stream of events.
    ///
    /// Each agent is spawned as a Tokio task and dispatched via
    /// [`workflow::dispatch_agent()`] which handles all agent kinds
    /// (LLM, Sequential, Parallel, Loop).
    ///
    /// Automatically subscribes to the PulseDB Watch system for the collective,
    /// forwarding substrate change events as [`HiveEvent::WatchNotification`].
    /// If Watch subscription fails, agents still execute normally (graceful degradation).
    pub async fn deploy(
        &self,
        agents: Vec<AgentDefinition>,
        tasks: Vec<Task>,
    ) -> Result<Pin<Box<dyn Stream<Item = HiveEvent> + Send>>> {
        if agents.is_empty() {
            return Ok(Box::pin(stream::empty()));
        }

        // Get the first task (or create a default one)
        let mut task = tasks.into_iter().next().unwrap_or_else(|| Task::new(""));

        // Ensure the collective exists in the substrate
        let collective_name = format!("collective-{}", task.collective_id);
        let collective_id = self
            .substrate
            .get_or_create_collective(&collective_name)
            .await?;
        task.collective_id = collective_id;

        // Subscribe to Watch system for real-time substrate change notifications.
        // Runs as a background task — failure to subscribe doesn't block deployment.
        let watch_substrate = Arc::clone(&self.substrate);
        let watch_emitter = self.event_bus.clone();
        tokio::spawn(async move {
            match watch_substrate.watch(collective_id).await {
                Ok(mut watch_stream) => {
                    while let Some(event) = watch_stream.next().await {
                        watch_emitter.emit(HiveEvent::WatchNotification {
                            experience_id: event.experience_id,
                            collective_id: event.collective_id,
                            event_type: format!("{:?}", event.event_type),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to subscribe to Watch system");
                }
            }
        });

        let rx = self.event_bus.subscribe();

        for agent in agents {
            self.spawn_agent(agent, task.clone());
        }

        // Convert broadcast::Receiver into a Stream
        Ok(Box::pin(BroadcastStream::new(rx)))
    }

    /// Record an experience in the substrate.
    ///
    /// Stores the experience via PulseDB, emits an `ExperienceRecorded` event,
    /// runs the RelationshipDetector to infer relations, and triggers the
    /// InsightSynthesizer if a cluster exceeds the density threshold.
    pub async fn record_experience(&self, experience: NewExperience) -> Result<ExperienceId> {
        let agent_id = experience.source_agent.0.clone();
        let collective_id = experience.collective_id;
        let id = self.substrate.store_experience(experience).await?;
        self.event_bus.emit(HiveEvent::ExperienceRecorded {
            experience_id: id,
            agent_id,
        });

        // Run relationship inference if detector is configured
        if let Some(detector) = &self.relationship_detector {
            if let Ok(Some(stored)) = self.substrate.get_experience(id).await {
                let relations = detector
                    .infer_relations(&stored, self.substrate.as_ref())
                    .await;

                for rel in relations {
                    match self.substrate.store_relation(rel).await {
                        Ok(relation_id) => {
                            self.event_bus.emit(HiveEvent::RelationshipInferred {
                                relation_id,
                            });
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to store inferred relation");
                        }
                    }
                }
            }
        }

        // Run insight synthesis if synthesizer is configured
        if let Some(synthesizer) = &self.insight_synthesizer {
            if !synthesizer.is_debounced(collective_id) {
                let cluster = synthesizer
                    .find_cluster(id, self.substrate.as_ref())
                    .await;

                if synthesizer.should_synthesize(cluster.len()) {
                    // Use the first available LLM provider for synthesis
                    if let Some((provider_name, provider)) = self.llm_providers.iter().next() {
                        let llm_config = pulsehive_core::llm::LlmConfig::new(
                            provider_name,
                            "default",
                        );
                        if let Some(insight) = synthesizer
                            .synthesize_cluster(&cluster, collective_id, provider.as_ref(), &llm_config)
                            .await
                        {
                            let source_count = insight.source_experience_ids.len();
                            match self.substrate.store_insight(insight).await {
                                Ok(insight_id) => {
                                    synthesizer.mark_synthesized(collective_id);
                                    self.event_bus.emit(HiveEvent::InsightGenerated {
                                        insight_id,
                                        source_count,
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "Failed to store synthesized insight");
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(id)
    }

    /// Spawn a single agent as a Tokio task.
    ///
    /// Builds a [`WorkflowContext`] from HiveMind's fields and delegates
    /// to [`workflow::dispatch_agent()`] which handles all agent kinds.
    fn spawn_agent(&self, agent: AgentDefinition, task: Task) {
        let ctx = WorkflowContext {
            task,
            llm_providers: self.llm_providers.clone(),
            substrate: Arc::clone(&self.substrate),
            approval_handler: Arc::clone(&self.approval_handler),
            event_emitter: self.event_bus.clone(),
        };

        tokio::spawn(async move {
            workflow::dispatch_agent(agent, &ctx).await;
        });
    }
}

/// Adapter that converts a `broadcast::Receiver<HiveEvent>` into a `Stream`.
struct BroadcastStream {
    rx: broadcast::Receiver<HiveEvent>,
}

impl BroadcastStream {
    fn new(rx: broadcast::Receiver<HiveEvent>) -> Self {
        Self { rx }
    }
}

impl Stream for BroadcastStream {
    type Item = HiveEvent;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.rx.try_recv() {
            Ok(event) => std::task::Poll::Ready(Some(event)),
            Err(broadcast::error::TryRecvError::Empty) => {
                // No events yet — register waker and return Pending
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                tracing::warn!(lagged = n, "Event stream lagged, some events dropped");
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            Err(broadcast::error::TryRecvError::Closed) => std::task::Poll::Ready(None),
        }
    }
}

/// Builder for constructing a [`HiveMind`] with validated configuration.
pub struct HiveMindBuilder {
    substrate: Option<Box<dyn SubstrateProvider>>,
    substrate_path: Option<String>,
    llm_providers: HashMap<String, Arc<dyn LlmProvider>>,
    approval_handler: Option<Box<dyn ApprovalHandler>>,
    relationship_detector: Option<Option<RelationshipDetector>>,
    insight_synthesizer: Option<Option<InsightSynthesizer>>,
}

impl HiveMindBuilder {
    fn new() -> Self {
        Self {
            substrate: None,
            substrate_path: None,
            llm_providers: HashMap::new(),
            approval_handler: None,
            relationship_detector: None,
            insight_synthesizer: None,
        }
    }

    /// Set substrate via file path.
    pub fn substrate_path(mut self, path: impl AsRef<Path>) -> Self {
        self.substrate_path = Some(path.as_ref().to_string_lossy().into_owned());
        self
    }

    /// Set a custom substrate provider (e.g., for testing with mocks).
    pub fn substrate(mut self, provider: Box<dyn SubstrateProvider>) -> Self {
        self.substrate = Some(provider);
        self
    }

    /// Register a named LLM provider.
    pub fn llm_provider(
        mut self,
        name: impl Into<String>,
        provider: impl LlmProvider + 'static,
    ) -> Self {
        self.llm_providers.insert(name.into(), Arc::new(provider));
        self
    }

    /// Set a custom approval handler. Defaults to [`AutoApprove`] if not set.
    pub fn approval_handler(mut self, handler: impl ApprovalHandler + 'static) -> Self {
        self.approval_handler = Some(Box::new(handler));
        self
    }

    /// Set a custom relationship detector. Default: enabled with default thresholds.
    pub fn relationship_detector(mut self, detector: RelationshipDetector) -> Self {
        self.relationship_detector = Some(Some(detector));
        self
    }

    /// Disable automatic relationship detection.
    pub fn no_relationship_detector(mut self) -> Self {
        self.relationship_detector = Some(None);
        self
    }

    /// Set a custom insight synthesizer. Default: enabled with default thresholds.
    pub fn insight_synthesizer(mut self, synthesizer: InsightSynthesizer) -> Self {
        self.insight_synthesizer = Some(Some(synthesizer));
        self
    }

    /// Disable automatic insight synthesis.
    pub fn no_insight_synthesizer(mut self) -> Self {
        self.insight_synthesizer = Some(None);
        self
    }

    /// Build the HiveMind. Validates that a substrate is configured.
    pub fn build(self) -> Result<HiveMind> {
        let substrate: Arc<dyn SubstrateProvider> = if let Some(s) = self.substrate {
            Arc::from(s)
        } else if let Some(path) = self.substrate_path {
            let db = PulseDB::open(&path, Config::with_builtin_embeddings())?;
            Arc::new(PulseDBSubstrate::from_db(db))
        } else {
            return Err(PulseHiveError::config(
                "Substrate not configured. Call substrate_path() or substrate() on the builder.",
            ));
        };

        let approval: Arc<dyn ApprovalHandler> = match self.approval_handler {
            Some(h) => Arc::from(h),
            None => Arc::new(AutoApprove),
        };

        // Default: relationship detector enabled with default thresholds
        let relationship_detector = match self.relationship_detector {
            Some(explicit) => explicit,
            None => Some(RelationshipDetector::with_defaults()),
        };

        // Default: insight synthesizer enabled with default thresholds
        let insight_synthesizer = match self.insight_synthesizer {
            Some(explicit) => explicit,
            None => Some(InsightSynthesizer::with_defaults()),
        };

        Ok(HiveMind {
            substrate,
            llm_providers: self.llm_providers,
            approval_handler: approval,
            event_bus: EventBus::default(),
            relationship_detector,
            insight_synthesizer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[test]
    fn test_build_fails_without_substrate() {
        let result = HiveMind::builder().build();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Substrate not configured"));
    }

    #[test]
    fn test_build_with_substrate_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        assert!(HiveMind::builder().substrate_path(&path).build().is_ok());
    }

    #[tokio::test]
    async fn test_deploy_empty_agents_returns_empty_stream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let hive = HiveMind::builder().substrate_path(&path).build().unwrap();

        let mut stream = hive.deploy(vec![], vec![]).await.unwrap();
        assert!(stream.next().await.is_none());
    }

    #[test]
    fn test_task_new() {
        let task = Task::new("Analyze the codebase");
        assert_eq!(task.description, "Analyze the codebase");
    }

    #[test]
    fn test_task_with_collective() {
        let cid = CollectiveId::new();
        let task = Task::with_collective("Search for bugs", cid);
        assert_eq!(task.collective_id, cid);
    }

    /// Helper: create a HiveMind with Builtin embeddings and a collective for testing.
    async fn build_hive_with_collective() -> (HiveMind, CollectiveId) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Leak tempdir so it lives long enough
        let dir = Box::leak(Box::new(dir));
        let _ = dir;
        let hive = HiveMind::builder().substrate_path(&path).build().unwrap();
        // Create collective via SubstrateProvider trait (no raw PulseDB needed!)
        let cid = hive
            .substrate
            .get_or_create_collective("test")
            .await
            .unwrap();
        (hive, cid)
    }

    #[tokio::test]
    async fn test_record_experience_stores_and_emits_event() {
        let (hive, cid) = build_hive_with_collective().await;
        let mut rx = hive.event_bus.subscribe();

        let exp = pulsedb::NewExperience {
            collective_id: cid,
            content: "Learned that Rust's ownership model prevents data races.".into(),
            experience_type: pulsedb::ExperienceType::Generic {
                category: Some("rust".into()),
            },
            embedding: None, // Builtin embeddings auto-compute
            importance: 0.8,
            confidence: 0.9,
            domain: vec!["rust".into(), "concurrency".into()],
            source_agent: pulsedb::AgentId("test-agent".into()),
            source_task: None,
            related_files: vec![],
        };

        let id = hive.record_experience(exp).await.unwrap();

        // Verify event emitted
        let event = rx.try_recv().unwrap();
        match event {
            HiveEvent::ExperienceRecorded {
                experience_id,
                agent_id,
            } => {
                assert_eq!(experience_id, id);
                assert_eq!(agent_id, "test-agent");
            }
            other => panic!("Expected ExperienceRecorded, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_record_experience_retrievable() {
        let (hive, cid) = build_hive_with_collective().await;

        let exp = pulsedb::NewExperience {
            collective_id: cid,
            content: "Test experience for retrieval.".into(),
            experience_type: pulsedb::ExperienceType::Generic { category: None },
            embedding: None, // Builtin embeddings auto-compute
            importance: 0.5,
            confidence: 0.5,
            domain: vec![],
            source_agent: pulsedb::AgentId("agent-1".into()),
            source_task: None,
            related_files: vec![],
        };

        let id = hive.record_experience(exp).await.unwrap();

        // Verify retrievable
        let retrieved = hive.substrate.get_experience(id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.content, "Test experience for retrieval.");
    }
}
