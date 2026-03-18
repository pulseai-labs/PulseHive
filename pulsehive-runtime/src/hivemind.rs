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
use futures_core::Stream;
use pulsedb::{CollectiveId, Config, PulseDB, PulseDBSubstrate, SubstrateProvider};
use tokio::sync::broadcast;

use pulsehive_core::agent::{AgentDefinition, AgentKind, AgentKindTag};
use pulsehive_core::approval::{ApprovalHandler, AutoApprove};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::{EventBus, HiveEvent};
use pulsehive_core::llm::LlmProvider;

use crate::agentic_loop::{self, LoopContext, DEFAULT_MAX_ITERATIONS};

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

    /// Deploy agents to execute tasks. Returns a stream of events.
    ///
    /// Each LLM agent is spawned as a Tokio task running the agentic loop.
    /// The returned stream receives all events from all agents via broadcast.
    ///
    /// Currently only supports `AgentKind::Llm`. Workflow agents
    /// (Sequential/Parallel/Loop) are supported in Sprint 5.
    pub async fn deploy(
        &self,
        agents: Vec<AgentDefinition>,
        tasks: Vec<Task>,
    ) -> Result<Pin<Box<dyn Stream<Item = HiveEvent> + Send>>> {
        if agents.is_empty() {
            return Ok(Box::pin(stream::empty()));
        }

        // Get the first task (or create a default one)
        let task = tasks.into_iter().next().unwrap_or_else(|| Task::new(""));

        let rx = self.event_bus.subscribe();

        for agent in agents {
            self.spawn_agent(agent, task.clone()).await?;
        }

        // Convert broadcast::Receiver into a Stream
        Ok(Box::pin(BroadcastStream::new(rx)))
    }

    /// Spawn a single agent as a Tokio task.
    async fn spawn_agent(&self, agent: AgentDefinition, task: Task) -> Result<()> {
        let agent_id = uuid::Uuid::now_v7().to_string();
        let name = agent.name.clone();

        match agent.kind {
            AgentKind::Llm(config) => {
                // Look up the provider
                let provider_name = &config.llm_config.provider;
                let provider = self
                    .llm_providers
                    .get(provider_name)
                    .ok_or_else(|| {
                        PulseHiveError::config(format!(
                            "LLM provider '{}' not registered. Available: {:?}",
                            provider_name,
                            self.llm_providers.keys().collect::<Vec<_>>()
                        ))
                    })?
                    .clone();

                let substrate = Arc::clone(&self.substrate);
                let approval = Arc::clone(&self.approval_handler);
                let emitter = self.event_bus.clone();

                // Emit AgentStarted
                emitter.emit(HiveEvent::AgentStarted {
                    agent_id: agent_id.clone(),
                    name: name.clone(),
                    kind: AgentKindTag::Llm,
                });

                // Spawn agent task
                let agent_id_clone = agent_id.clone();
                tokio::spawn(async move {
                    let outcome = agentic_loop::run_agentic_loop(
                        *config,
                        LoopContext {
                            agent_id: agent_id_clone.clone(),
                            task: &task,
                            provider,
                            substrate,
                            approval_handler: approval.as_ref(),
                            event_emitter: emitter.clone(),
                            max_iterations: DEFAULT_MAX_ITERATIONS,
                        },
                    )
                    .await;

                    emitter.emit(HiveEvent::AgentCompleted {
                        agent_id: agent_id_clone,
                        outcome,
                    });
                });

                Ok(())
            }
            AgentKind::Sequential(_) | AgentKind::Parallel(_) | AgentKind::Loop { .. } => {
                Err(PulseHiveError::config(
                    "Workflow agents (Sequential/Parallel/Loop) not yet supported. Coming in Sprint 5.",
                ))
            }
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
}

impl HiveMindBuilder {
    fn new() -> Self {
        Self {
            substrate: None,
            substrate_path: None,
            llm_providers: HashMap::new(),
            approval_handler: None,
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

    /// Build the HiveMind. Validates that a substrate is configured.
    pub fn build(self) -> Result<HiveMind> {
        let substrate: Arc<dyn SubstrateProvider> = if let Some(s) = self.substrate {
            Arc::from(s)
        } else if let Some(path) = self.substrate_path {
            let db = PulseDB::open(&path, Config::default())?;
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

        Ok(HiveMind {
            substrate,
            llm_providers: self.llm_providers,
            approval_handler: approval,
            event_bus: EventBus::default(),
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
}
