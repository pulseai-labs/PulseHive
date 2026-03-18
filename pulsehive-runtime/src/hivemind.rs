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

use pulsehive_core::agent::AgentDefinition;
use pulsehive_core::approval::{ApprovalHandler, AutoApprove};
use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::event::{EventBus, HiveEvent};
use pulsehive_core::llm::LlmProvider;

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
    #[allow(dead_code)]
    pub(crate) substrate: Box<dyn SubstrateProvider>,
    #[allow(dead_code)]
    pub(crate) llm_providers: HashMap<String, Arc<dyn LlmProvider>>,
    #[allow(dead_code)]
    pub(crate) approval_handler: Box<dyn ApprovalHandler>,
    #[allow(dead_code)]
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
    /// **Sprint 1 stub**: Returns an empty stream. The agentic loop
    /// (perceive-think-act-record) is wired in Sprint 2.
    pub async fn deploy(
        &self,
        _agents: Vec<AgentDefinition>,
        _tasks: Vec<Task>,
    ) -> Result<Pin<Box<dyn Stream<Item = HiveEvent> + Send>>> {
        Ok(Box::pin(stream::empty()))
    }
}

/// Builder for constructing a [`HiveMind`] with validated configuration.
///
/// At minimum, a substrate must be configured (via `substrate_path` or `substrate`).
/// LLM providers and approval handler are optional.
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

    /// Set substrate via file path. Creates a PulseDB database at the given path.
    ///
    /// Uses PulseDB's default configuration with builtin embeddings (all-MiniLM-L6-v2, 384d).
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
    ///
    /// The name is used to route agent requests — `LlmConfig.provider` must match
    /// one of the registered names.
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
    ///
    /// Returns `Err(PulseHiveError::Config(...))` if no substrate is set.
    pub fn build(self) -> Result<HiveMind> {
        let substrate = if let Some(s) = self.substrate {
            s
        } else if let Some(path) = self.substrate_path {
            let db = PulseDB::open(&path, Config::default())?;
            Box::new(PulseDBSubstrate::from_db(db))
        } else {
            return Err(PulseHiveError::config(
                "Substrate not configured. Call substrate_path() or substrate() on the builder.",
            ));
        };

        Ok(HiveMind {
            substrate,
            llm_providers: self.llm_providers,
            approval_handler: self
                .approval_handler
                .unwrap_or_else(|| Box::new(AutoApprove)),
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
        let err = result.unwrap_err();
        assert!(matches!(err, PulseHiveError::Config(_)));
        assert!(err.to_string().contains("Substrate not configured"));
    }

    #[test]
    fn test_build_with_substrate_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");

        let result = HiveMind::builder().substrate_path(&path).build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_with_llm_provider() {
        use async_trait::async_trait;
        use futures_core::Stream;
        use pulsehive_core::error::Result as HiveResult;
        use pulsehive_core::llm::*;
        use std::pin::Pin;

        // Minimal mock provider
        struct MockLlm;

        #[async_trait]
        impl LlmProvider for MockLlm {
            async fn chat(
                &self,
                _msgs: Vec<Message>,
                _tools: Vec<ToolDefinition>,
                _config: &LlmConfig,
            ) -> HiveResult<LlmResponse> {
                Ok(LlmResponse {
                    content: Some("mock".into()),
                    tool_calls: vec![],
                    usage: TokenUsage::default(),
                })
            }
            async fn chat_stream(
                &self,
                _msgs: Vec<Message>,
                _tools: Vec<ToolDefinition>,
                _config: &LlmConfig,
            ) -> HiveResult<Pin<Box<dyn Stream<Item = HiveResult<LlmChunk>> + Send>>> {
                Ok(Box::pin(futures::stream::empty()))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");

        let hive = HiveMind::builder()
            .substrate_path(&path)
            .llm_provider("mock", MockLlm)
            .build()
            .unwrap();

        assert!(hive.llm_providers.contains_key("mock"));
    }

    #[tokio::test]
    async fn test_deploy_returns_empty_stream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");

        let hive = HiveMind::builder().substrate_path(&path).build().unwrap();

        let mut stream = hive.deploy(vec![], vec![]).await.unwrap();
        // Stream should be immediately done (empty)
        assert!(stream.next().await.is_none());
    }

    #[test]
    fn test_task_new() {
        let task = Task::new("Analyze the codebase");
        assert_eq!(task.description, "Analyze the codebase");
        // collective_id is auto-generated
    }

    #[test]
    fn test_task_with_collective() {
        let cid = CollectiveId::new();
        let task = Task::with_collective("Search for bugs", cid);
        assert_eq!(task.description, "Search for bugs");
        assert_eq!(task.collective_id, cid);
    }

    #[test]
    fn test_default_approval_handler_is_auto_approve() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");

        // Build without setting approval handler — should use AutoApprove
        let _hive = HiveMind::builder().substrate_path(&path).build().unwrap();
        // If it builds, AutoApprove was used (no way to inspect from outside,
        // but the fact it compiled confirms the default works)
    }
}
