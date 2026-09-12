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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::stream;
use futures::{Stream, StreamExt};
use pulsedb::{
    CollectiveId, Config, ExperienceId, NewExperience, PulseDB, PulseDBSubstrate, SubstrateProvider,
};
use tokio::sync::broadcast;

use pulsehive_core::agent::AgentDefinition;
use pulsehive_core::approval::{ApprovalHandler, AutoApprove};
use pulsehive_core::embedding::EmbeddingProvider;
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::{EventBus, HiveEvent};
use pulsehive_core::export::EventExporter;
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
    /// Optional embedding provider for domain-specific models.
    /// When set, embeddings are computed via this provider before PulseDB storage.
    pub(crate) embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    /// Shutdown signal for background tasks (Watch system).
    shutdown: Arc<AtomicBool>,
    /// Handles to the per-deployment Watch background tasks, pruned of
    /// completed entries on each deploy and aborted together on
    /// shutdown/drop.
    watch_handles: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
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

    /// Resolve a task's existing collective or create its synthetic namespace.
    async fn resolve_collective(&self, task: &mut Task) -> Result<()> {
        let exists = self
            .substrate
            .list_collectives()
            .await?
            .iter()
            .any(|collective| collective.id == task.collective_id);
        if exists {
            return Ok(());
        }

        let collective_name = format!("collective-{}", task.collective_id);
        task.collective_id = self
            .substrate
            .get_or_create_collective(&collective_name)
            .await?;
        Ok(())
    }

    /// Deploy agents to execute tasks. Returns a stream of events.
    ///
    /// Every agent runs against every task — the cartesian product of
    /// `agents × tasks` — so each task is executed by the full agent set.
    /// Each task's collective is resolved independently (an existing
    /// collective is reused; an unknown ID gets the synthetic
    /// `collective-{id}` namespace). An empty `tasks` list deploys every
    /// agent against a single default empty task.
    ///
    /// Each agent run is spawned as a Tokio task and dispatched via
    /// the workflow module's `dispatch_agent()` which handles all agent kinds
    /// (LLM, Sequential, Parallel, Loop).
    ///
    /// Automatically subscribes to the PulseDB Watch system — one background
    /// watch task per deployment, fanning in every resolved collective —
    /// forwarding substrate change events as
    /// [`HiveEvent::WatchNotification`]. Watches of earlier deployments keep
    /// forwarding while those deployments run; completed watch tasks are
    /// pruned on the next deploy, and shutdown/drop aborts all of them. If a
    /// Watch subscription fails, that collective is skipped and agents still
    /// execute normally (graceful degradation).
    pub async fn deploy(
        &self,
        agents: Vec<AgentDefinition>,
        tasks: Vec<Task>,
    ) -> Result<Pin<Box<dyn Stream<Item = HiveEvent> + Send>>> {
        if agents.is_empty() {
            return Ok(Box::pin(stream::empty()));
        }

        // Resolve every task's collective before falling back to the
        // historical single default task, so no task in the list is dropped.
        let mut resolved_tasks = Vec::with_capacity(tasks.len());
        for mut task in tasks {
            self.resolve_collective(&mut task).await?;
            resolved_tasks.push(task);
        }
        if resolved_tasks.is_empty() {
            resolved_tasks.push(Task::new(""));
        }

        // One watch collective per distinct resolved collective, first-seen
        // order, so duplicated collectives do not duplicate notifications.
        let mut watch_collectives: Vec<CollectiveId> = Vec::new();
        for task in &resolved_tasks {
            if !watch_collectives.contains(&task.collective_id) {
                watch_collectives.push(task.collective_id);
            }
        }

        // Subscribe to Watch system for real-time substrate change notifications.
        // Runs as a single background task fanning in every collective —
        // failure to subscribe doesn't block deployment, and the shutdown
        // flag is respected for graceful termination.
        let watch_substrate = Arc::clone(&self.substrate);
        let watch_emitter = self.event_bus.clone();
        let watch_shutdown = Arc::clone(&self.shutdown);
        let watch_handle = tokio::spawn(async move {
            let mut watch_streams = stream::SelectAll::new();
            for collective_id in watch_collectives {
                match watch_substrate.watch(collective_id).await {
                    Ok(watch_stream) => watch_streams.push(watch_stream),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            collective_id = %collective_id,
                            "Failed to subscribe to Watch system"
                        );
                    }
                }
            }
            while !watch_shutdown.load(Ordering::Relaxed) {
                match watch_streams.next().await {
                    Some(event) => {
                        watch_emitter.emit(HiveEvent::WatchNotification {
                            timestamp_ms: pulsehive_core::event::now_ms(),
                            experience_id: event.experience_id,
                            collective_id: event.collective_id,
                            event_type: format!("{:?}", event.event_type),
                        });
                    }
                    None => break,
                }
            }
        });
        // Retain the watch tasks of still-running deployments — each keeps
        // forwarding its collectives' WatchNotifications — while pruning
        // handles whose task already finished, so the set cannot grow
        // without bound across repeated deploys. A poisoned lock (a panic
        // elsewhere held it) must not block deployment: recover the guard.
        {
            let mut watch_set = self
                .watch_handles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            watch_set.retain(|handle| !handle.is_finished());
            watch_set.push(watch_handle);
        }

        let rx = self.event_bus.subscribe();

        for agent in agents {
            for task in &resolved_tasks {
                self.spawn_agent(agent.clone(), task.clone());
            }
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

        // Compute embedding via provider if available and not already set
        let mut experience = experience;
        if let Some(provider) = &self.embedding_provider {
            if experience.embedding.is_none() {
                match provider.embed(&experience.content).await {
                    Ok(embedding) => {
                        experience.embedding = Some(embedding);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to compute embedding in record_experience, storing without");
                    }
                }
            }
        }

        // Capture metadata before move
        let content_preview: String = experience.content.chars().take(200).collect();
        let experience_type_str = format!("{:?}", experience.experience_type);
        let importance = experience.importance;

        let id = self.substrate.store_experience(experience).await?;
        self.event_bus.emit(HiveEvent::ExperienceRecorded {
            timestamp_ms: pulsehive_core::event::now_ms(),
            experience_id: id,
            agent_id: agent_id.clone(),
            content_preview,
            experience_type: experience_type_str,
            importance,
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
                                timestamp_ms: pulsehive_core::event::now_ms(),
                                relation_id,
                                agent_id: agent_id.clone(),
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
                let cluster = synthesizer.find_cluster(id, self.substrate.as_ref()).await;

                if synthesizer.should_synthesize(cluster.len()) {
                    // Use the first available LLM provider for synthesis
                    if let Some((provider_name, provider)) = self.llm_providers.iter().next() {
                        let llm_config =
                            pulsehive_core::llm::LlmConfig::new(provider_name, "default");
                        if let Some(insight) = synthesizer
                            .synthesize_cluster(
                                &cluster,
                                collective_id,
                                provider.as_ref(),
                                &llm_config,
                            )
                            .await
                        {
                            let source_count = insight.source_experience_ids.len();
                            match self.substrate.store_insight(insight).await {
                                Ok(insight_id) => {
                                    synthesizer.mark_synthesized(collective_id);
                                    self.event_bus.emit(HiveEvent::InsightGenerated {
                                        timestamp_ms: pulsehive_core::event::now_ms(),
                                        insight_id,
                                        source_count,
                                        agent_id: agent_id.clone(),
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

    /// Signal shutdown to all background tasks (Watch system).
    ///
    /// Sets the shutdown flag, causing the Watch background tasks to stop
    /// after processing their current event. This is non-blocking.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Abort the Watch background tasks so they drop their EventBus sender
        // clones, allowing the broadcast channel to close and BroadcastStream
        // to terminate. Recover from a poisoned lock: shutdown must run even
        // while another thread is unwinding a panic.
        for handle in self
            .watch_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            handle.abort();
        }
        tracing::info!("HiveMind shutdown signaled");
    }

    /// Returns true if shutdown has been signaled.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    /// Redeploy agents on the existing substrate and event bus.
    ///
    /// Use this to restart failed agents. Products typically call this when
    /// they observe `AgentCompleted { outcome: Error { .. } }` on the event stream.
    ///
    /// The collective is created/resolved from the task, same as in [`HiveMind::deploy()`].
    pub async fn redeploy(&self, agents: Vec<AgentDefinition>, task: Task) -> Result<()> {
        if agents.is_empty() {
            return Ok(());
        }

        let mut task = task;
        self.resolve_collective(&mut task).await?;

        for agent in agents {
            self.spawn_agent(agent, task.clone());
        }

        Ok(())
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
            embedding_provider: self.embedding_provider.clone(),
        };

        tokio::spawn(async move {
            workflow::dispatch_agent(agent, &ctx).await;
        });
    }
}

impl Drop for HiveMind {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        for handle in self.watch_handles.get_mut().unwrap().drain(..) {
            handle.abort();
        }
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
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    event_exporter: Option<Arc<dyn EventExporter>>,
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
            embedding_provider: None,
            event_exporter: None,
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

    /// Set a custom embedding provider for domain-specific models.
    ///
    /// When set, PulseHive computes embeddings via this provider before storing
    /// experiences in PulseDB (External mode). When not set, PulseDB uses its
    /// built-in all-MiniLM-L6-v2 model (384d).
    pub fn embedding_provider(mut self, provider: impl EmbeddingProvider + 'static) -> Self {
        self.embedding_provider = Some(Arc::new(provider));
        self
    }

    /// Set an event exporter for streaming events to external observability systems.
    ///
    /// When set, every `HiveEvent` emission is also forwarded to the exporter
    /// via a fire-and-forget `tokio::spawn` call — zero latency on the emit path.
    ///
    /// Use this to connect PulseHive to PulseVision or custom dashboards.
    pub fn event_exporter(mut self, exporter: impl EventExporter + 'static) -> Self {
        self.event_exporter = Some(Arc::new(exporter));
        self
    }

    /// Build the HiveMind. Validates that a substrate is configured.
    pub fn build(self) -> Result<HiveMind> {
        let substrate: Arc<dyn SubstrateProvider> = if let Some(s) = self.substrate {
            Arc::from(s)
        } else if let Some(path) = self.substrate_path {
            let config = if self.embedding_provider.is_some() {
                // External mode: PulseHive computes embeddings via the provider
                Config::new()
            } else {
                // Builtin mode: PulseDB computes embeddings internally
                Config::with_builtin_embeddings()
            };
            let db = PulseDB::open(&path, config)?;
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

        let event_bus = match self.event_exporter {
            Some(exporter) => EventBus::with_exporter(256, exporter),
            None => EventBus::default(),
        };

        Ok(HiveMind {
            substrate,
            llm_providers: self.llm_providers,
            approval_handler: approval,
            event_bus,
            relationship_detector,
            insight_synthesizer,
            embedding_provider: self.embedding_provider,
            shutdown: Arc::new(AtomicBool::new(false)),
            watch_handles: std::sync::Mutex::new(Vec::new()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use pulsehive_core::agent::AgentKind;
    use std::sync::atomic::AtomicUsize;

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

    #[tokio::test]
    async fn task_with_collective_reuses_existing_collective() {
        let dir = tempfile::tempdir().unwrap();
        let hive = HiveMind::builder()
            .substrate_path(dir.path().join("existing.db"))
            .build()
            .unwrap();
        let existing_id = hive
            .substrate()
            .get_or_create_collective("differently-named-existing")
            .await
            .unwrap();
        let mut task = Task::with_collective("reuse it", existing_id);

        hive.resolve_collective(&mut task).await.unwrap();

        let collectives = hive.substrate().list_collectives().await.unwrap();
        assert_eq!(task.collective_id, existing_id);
        assert_eq!(collectives.len(), 1, "must not create a duplicate");
        assert_eq!(collectives[0].name, "differently-named-existing");
    }

    #[tokio::test]
    async fn task_new_creates_collective() {
        let dir = tempfile::tempdir().unwrap();
        let hive = HiveMind::builder()
            .substrate_path(dir.path().join("new.db"))
            .build()
            .unwrap();
        let mut task = Task::new("create it");
        let synthetic_name = format!("collective-{}", task.collective_id);

        hive.resolve_collective(&mut task).await.unwrap();

        let collectives = hive.substrate().list_collectives().await.unwrap();
        assert_eq!(collectives.len(), 1);
        assert_eq!(collectives[0].name, synthetic_name);
        assert_eq!(task.collective_id, collectives[0].id);
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
            embedding: None,
            importance: 0.8,
            confidence: 0.9,
            domain: vec!["rust".into(), "concurrency".into()],
            source_agent: pulsedb::AgentId("test-agent".into()),
            source_task: None,
            tags: Default::default(),
            related_files: vec![],
        };

        let id = hive.record_experience(exp).await.unwrap();

        // Verify event emitted
        let event = rx.try_recv().unwrap();
        match event {
            HiveEvent::ExperienceRecorded {
                experience_id,
                agent_id,
                ..
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
            embedding: None,
            importance: 0.5,
            confidence: 0.5,
            domain: vec![],
            source_agent: pulsedb::AgentId("agent-1".into()),
            source_task: None,
            tags: Default::default(),
            related_files: vec![],
        };

        let id = hive.record_experience(exp).await.unwrap();

        // Verify retrievable
        let retrieved = hive.substrate.get_experience(id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.content, "Test experience for retrieval.");
    }

    // ── Shutdown & Restart tests ─────────────────────────────────────

    #[test]
    fn test_shutdown_sets_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let hive = HiveMind::builder().substrate_path(&path).build().unwrap();

        assert!(!hive.is_shutdown());
        hive.shutdown();
        assert!(hive.is_shutdown());
    }

    #[test]
    fn test_drop_sets_shutdown_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let hive = HiveMind::builder().substrate_path(&path).build().unwrap();
        let shutdown = Arc::clone(&hive.shutdown);

        assert!(!shutdown.load(Ordering::Relaxed));
        drop(hive);
        assert!(shutdown.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_redeploy_empty_is_noop() {
        let (hive, _cid) = build_hive_with_collective().await;
        let task = Task::new("test");
        assert!(hive.redeploy(vec![], task).await.is_ok());
    }

    // ── Watch task lifecycle tests ───────────────────────────────────

    /// Substrate double that counts how many watch streams are alive.
    ///
    /// `watch()` increments the count and the returned stream decrements it
    /// on drop, so the count tracks exactly how many deploy watch tasks hold
    /// a live subscription. The first subscription ends immediately (its
    /// stream yields `None` right away) so pruning of finished watch tasks
    /// is observable; later subscriptions stay pending forever.
    struct CountingWatchSubstrate {
        live_watches: Arc<AtomicUsize>,
        subscriptions: AtomicUsize,
        end_first_subscription: bool,
    }

    struct LiveWatchGuard(Arc<AtomicUsize>);

    impl Drop for LiveWatchGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct CountedWatchStream {
        pending: Pin<Box<dyn Stream<Item = pulsedb::WatchEvent> + Send>>,
        _guard: LiveWatchGuard,
    }

    impl Stream for CountedWatchStream {
        type Item = pulsedb::WatchEvent;

        fn poll_next(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            self.pending.as_mut().poll_next(cx)
        }
    }

    type DbResult<T> = std::result::Result<T, pulsedb::PulseDBError>;

    #[async_trait::async_trait]
    impl pulsedb::SubstrateProvider for CountingWatchSubstrate {
        async fn store_experience(
            &self,
            _exp: pulsedb::NewExperience,
        ) -> DbResult<pulsedb::ExperienceId> {
            Ok(pulsedb::ExperienceId::new())
        }

        async fn get_experience(
            &self,
            _id: pulsedb::ExperienceId,
        ) -> DbResult<Option<pulsedb::Experience>> {
            Ok(None)
        }

        async fn search_similar(
            &self,
            _collective: CollectiveId,
            _embedding: &[f32],
            _k: usize,
        ) -> DbResult<Vec<(pulsedb::Experience, f32)>> {
            Ok(vec![])
        }

        async fn get_recent(
            &self,
            _collective: CollectiveId,
            _limit: usize,
        ) -> DbResult<Vec<pulsedb::Experience>> {
            Ok(vec![])
        }

        async fn store_relation(
            &self,
            _rel: pulsedb::NewExperienceRelation,
        ) -> DbResult<pulsedb::RelationId> {
            Ok(pulsedb::RelationId::new())
        }

        async fn get_related(
            &self,
            _exp_id: pulsedb::ExperienceId,
        ) -> DbResult<Vec<(pulsedb::Experience, pulsedb::ExperienceRelation)>> {
            Ok(vec![])
        }

        async fn store_insight(
            &self,
            _insight: pulsedb::NewDerivedInsight,
        ) -> DbResult<pulsedb::InsightId> {
            Ok(pulsedb::InsightId::new())
        }

        async fn get_insights(
            &self,
            _collective: CollectiveId,
            _embedding: &[f32],
            _k: usize,
        ) -> DbResult<Vec<(pulsedb::DerivedInsight, f32)>> {
            Ok(vec![])
        }

        async fn get_activities(
            &self,
            _collective: CollectiveId,
        ) -> DbResult<Vec<pulsedb::Activity>> {
            Ok(vec![])
        }

        async fn get_context_candidates(
            &self,
            _request: pulsedb::ContextRequest,
        ) -> DbResult<pulsedb::ContextCandidates> {
            Ok(pulsedb::ContextCandidates {
                similar_experiences: vec![],
                recent_experiences: vec![],
                insights: vec![],
                relations: vec![],
                active_agents: vec![],
            })
        }

        async fn watch(
            &self,
            _collective: CollectiveId,
        ) -> DbResult<Pin<Box<dyn Stream<Item = pulsedb::WatchEvent> + Send>>> {
            self.live_watches.fetch_add(1, Ordering::SeqCst);
            let ends_immediately = self.end_first_subscription
                && self.subscriptions.fetch_add(1, Ordering::SeqCst) == 0;
            let pending: Pin<Box<dyn Stream<Item = pulsedb::WatchEvent> + Send>> =
                if ends_immediately {
                    Box::pin(futures::stream::empty())
                } else {
                    Box::pin(futures::stream::pending())
                };
            let stream = CountedWatchStream {
                pending,
                _guard: LiveWatchGuard(Arc::clone(&self.live_watches)),
            };
            Ok(Box::pin(stream))
        }

        async fn create_collective(&self, _name: &str) -> DbResult<CollectiveId> {
            Ok(CollectiveId::new())
        }

        async fn get_or_create_collective(&self, _name: &str) -> DbResult<CollectiveId> {
            Ok(CollectiveId::new())
        }

        async fn list_collectives(&self) -> DbResult<Vec<pulsedb::Collective>> {
            Ok(vec![])
        }
    }

    /// Polls until the live watch count settles at `expected`, or fails with
    /// the observed count after a generous timeout.
    async fn wait_for_live_watches(live: &Arc<AtomicUsize>, expected: usize) {
        for _ in 0..500 {
            if live.load(Ordering::SeqCst) == expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!(
            "live watch count did not settle at {expected}: {}",
            live.load(Ordering::SeqCst)
        );
    }

    /// Polls until `expected` tracked watch handles are marked finished.
    async fn wait_for_finished_handles(hive: &HiveMind, expected: usize) {
        for _ in 0..500 {
            let finished = hive
                .watch_handles
                .lock()
                .unwrap()
                .iter()
                .filter(|handle| handle.is_finished())
                .count();
            if finished == expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("no tracked watch handle was marked finished");
    }

    fn counting_hive(live_watches: Arc<AtomicUsize>, end_first_subscription: bool) -> HiveMind {
        HiveMind::builder()
            .substrate(Box::new(CountingWatchSubstrate {
                live_watches,
                subscriptions: AtomicUsize::new(0),
                end_first_subscription,
            }))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn repeated_deploy_retains_watch_per_deployment() {
        let live_watches = Arc::new(AtomicUsize::new(0));
        let hive = counting_hive(Arc::clone(&live_watches), false);
        let agent = |name: &str| AgentDefinition {
            name: name.into(),
            kind: AgentKind::Sequential(vec![]),
        };

        let first = hive
            .deploy(vec![agent("first")], vec![Task::new("first")])
            .await
            .unwrap();
        drop(first);
        wait_for_live_watches(&live_watches, 1).await;

        let second = hive
            .deploy(vec![agent("second")], vec![Task::new("second")])
            .await
            .unwrap();
        drop(second);
        // The still-running first deployment keeps its watch: a second
        // deploy must not abort it, and no watch may be leaked beyond the
        // two deployments' own streams.
        wait_for_live_watches(&live_watches, 2).await;
        assert_eq!(live_watches.load(Ordering::SeqCst), 2);

        // Shutdown tears down every retained watch task.
        hive.shutdown();
        wait_for_live_watches(&live_watches, 0).await;
    }

    #[tokio::test]
    async fn finished_watch_handles_are_pruned_on_next_deploy() {
        let live_watches = Arc::new(AtomicUsize::new(0));
        let hive = counting_hive(Arc::clone(&live_watches), true);
        let agent = AgentDefinition {
            name: "noop".into(),
            kind: AgentKind::Sequential(vec![]),
        };

        // First deploy's watch stream ends immediately, so its task finishes.
        let first = hive
            .deploy(vec![agent.clone()], vec![Task::new("first")])
            .await
            .unwrap();
        drop(first);
        wait_for_live_watches(&live_watches, 0).await;
        // The finished task is marked done asynchronously — wait for the
        // runtime to record it before expecting the next deploy to prune it.
        wait_for_finished_handles(&hive, 1).await;
        {
            let tracked = hive.watch_handles.lock().unwrap().len();
            assert_eq!(
                tracked, 1,
                "finished handle is still tracked until the next deploy prunes it"
            );
        }

        // The next deploy prunes the finished handle instead of accumulating.
        let second = hive
            .deploy(vec![agent], vec![Task::new("second")])
            .await
            .unwrap();
        drop(second);
        wait_for_live_watches(&live_watches, 1).await;
        {
            let tracked = hive.watch_handles.lock().unwrap().len();
            assert_eq!(tracked, 1, "the finished handle was pruned on deploy");
        }

        hive.shutdown();
        wait_for_live_watches(&live_watches, 0).await;
    }

    #[tokio::test]
    async fn multi_task_deploy_fans_collectives_into_one_watch() {
        let live_watches = Arc::new(AtomicUsize::new(0));
        let hive = counting_hive(Arc::clone(&live_watches), false);
        let agent = AgentDefinition {
            name: "noop".into(),
            kind: AgentKind::Sequential(vec![]),
        };

        // Three tasks resolve to three distinct collectives, fanned into ONE
        // watch task that holds three live streams.
        let multi = hive
            .deploy(
                vec![agent.clone()],
                vec![Task::new("a"), Task::new("b"), Task::new("c")],
            )
            .await
            .unwrap();
        drop(multi);
        wait_for_live_watches(&live_watches, 3).await;

        // A new deploy adds its own watch task (one more stream), never
        // cancels the fan-in, and shutdown clears everything.
        let next = hive
            .deploy(vec![agent], vec![Task::new("d")])
            .await
            .unwrap();
        drop(next);
        wait_for_live_watches(&live_watches, 4).await;
        hive.shutdown();
        wait_for_live_watches(&live_watches, 0).await;
    }

    /// Scripted-LLM helper for deploy contract tests: an agent whose single
    /// turn replies `text` through the shared `scripted` provider.
    fn scripted_agent(name: &str) -> AgentDefinition {
        use pulsehive_core::agent::LlmAgentConfig;
        use pulsehive_core::lens::Lens;
        use pulsehive_core::llm::LlmConfig;

        AgentDefinition {
            name: name.into(),
            kind: AgentKind::Llm(Box::new(LlmAgentConfig {
                system_prompt: "Reply with one word.".into(),
                tools: vec![],
                lens: Lens::default(),
                llm_config: LlmConfig::new("scripted", "deploy-contract-test"),
                experience_extractor: None,
                refresh_every_n_tool_calls: None,
            })),
        }
    }

    /// Deploys and drains the event stream until `expected` AgentCompleted
    /// events have been observed.
    async fn drain_n_completions(
        hive: &HiveMind,
        agents: Vec<AgentDefinition>,
        tasks: Vec<Task>,
        expected: usize,
    ) {
        let mut stream = hive.deploy(agents, tasks).await.unwrap();
        let mut completed = 0;
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            while let Some(event) = stream.next().await {
                if matches!(event, HiveEvent::AgentCompleted { .. }) {
                    completed += 1;
                    if completed == expected {
                        return;
                    }
                }
            }
        })
        .await
        .expect("drain timed out before all agents completed");
    }

    #[tokio::test]
    async fn deploy_runs_every_agent_against_every_task() {
        use pulsehive_core::llm::Message;
        use pulsehive_core::testing::ScriptedProvider;

        let mut provider = ScriptedProvider::new();
        for _ in 0..6 {
            provider = provider.then_text("done");
        }
        let dir = tempfile::tempdir().unwrap();
        let hive = HiveMind::builder()
            .substrate_path(dir.path().join("cartesian.db"))
            .llm_provider("scripted", provider.clone())
            .no_relationship_detector()
            .no_insight_synthesizer()
            .build()
            .unwrap();

        let tasks = vec![
            Task::new("task-a"),
            Task::new("task-b"),
            Task::new("task-c"),
        ];
        drain_n_completions(
            &hive,
            vec![scripted_agent("agent-one"), scripted_agent("agent-two")],
            tasks,
            6,
        )
        .await;

        let requests = provider.requests();
        assert_eq!(requests.len(), 6, "2 agents x 3 tasks = 6 agent runs");
        let mut seen: Vec<String> = requests
            .iter()
            .flat_map(|request| {
                request.messages.iter().filter_map(|message| match message {
                    Message::User { content } => Some(content.clone()),
                    _ => None,
                })
            })
            .collect();
        for description in ["task-a", "task-b", "task-c"] {
            assert_eq!(
                seen.iter().filter(|d| d.as_str() == description).count(),
                2,
                "each task is executed by both agents"
            );
        }
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen,
            vec!["task-a", "task-b", "task-c"],
            "no task outside the list was invented"
        );
    }

    #[tokio::test]
    async fn deploy_without_tasks_runs_every_agent_on_the_default_task() {
        use pulsehive_core::llm::Message;
        use pulsehive_core::testing::ScriptedProvider;

        let mut provider = ScriptedProvider::new();
        for _ in 0..2 {
            provider = provider.then_text("done");
        }
        let dir = tempfile::tempdir().unwrap();
        let hive = HiveMind::builder()
            .substrate_path(dir.path().join("default-task.db"))
            .llm_provider("scripted", provider.clone())
            .no_relationship_detector()
            .no_insight_synthesizer()
            .build()
            .unwrap();

        drain_n_completions(
            &hive,
            vec![scripted_agent("agent-one"), scripted_agent("agent-two")],
            vec![],
            2,
        )
        .await;

        let requests = provider.requests();
        assert_eq!(requests.len(), 2, "both agents ran once");
        for request in requests {
            let user_messages: Vec<&String> = request
                .messages
                .iter()
                .filter_map(|message| match message {
                    Message::User { content } => Some(content),
                    _ => None,
                })
                .collect();
            assert_eq!(
                user_messages,
                vec![&String::new()],
                "the default task carries an empty description"
            );
        }
    }
}
