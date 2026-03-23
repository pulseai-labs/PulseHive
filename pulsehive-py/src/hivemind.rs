//! Python bindings for HiveMind, HiveMindBuilder, Task, and LLM provider factories.

use std::pin::Pin;
use std::sync::Arc;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use tokio::sync::Mutex;

use pulsehive_core::llm::LlmProvider;
use pulsehive_runtime::hivemind::{HiveMind, Task};

use crate::agents::PyAgentDefinition;
use crate::stream::PyEventStream;

// ── LLM Provider Proxy ───────────────────────────────────────────────

/// Opaque LLM provider — holds an Arc<dyn LlmProvider> internally.
///
/// Created via factory functions: ``openai_provider()``, ``anthropic_provider()``.
/// Passed to ``HiveMind.builder().llm_provider(name, provider)``.
#[pyclass(name = "LlmProviderProxy", frozen)]
pub struct PyLlmProviderProxy {
    pub(crate) inner: Arc<dyn LlmProvider>,
    pub(crate) name: String,
}

#[pymethods]
impl PyLlmProviderProxy {
    fn __repr__(&self) -> String {
        format!("LlmProviderProxy('{}')", self.name)
    }
}

/// Create an OpenAI-compatible LLM provider.
///
/// Works with OpenAI, Azure OpenAI, GLM, vLLM, Ollama, and other
/// OpenAI-compatible APIs.
///
/// Args:
///     api_key: API key for authentication
///     model: Default model name (e.g., "gpt-4")
///     base_url: Override base URL (default: https://api.openai.com/v1)
#[pyfunction]
#[pyo3(signature = (api_key, model="gpt-4".to_string(), base_url=None))]
pub fn openai_provider(
    api_key: String,
    model: String,
    base_url: Option<String>,
) -> PyLlmProviderProxy {
    let mut config = pulsehive_openai::OpenAIConfig::new(&api_key, &model);
    if let Some(url) = base_url {
        config = config.with_base_url(&url);
    }
    let provider = pulsehive_openai::OpenAICompatibleProvider::new(config);
    PyLlmProviderProxy {
        inner: Arc::new(provider),
        name: "openai".into(),
    }
}

/// Create an Anthropic Claude LLM provider.
///
/// Args:
///     api_key: Anthropic API key
#[pyfunction]
pub fn anthropic_provider(api_key: String) -> PyLlmProviderProxy {
    let provider = pulsehive_anthropic::AnthropicProvider::new(&api_key);
    PyLlmProviderProxy {
        inner: Arc::new(provider),
        name: "anthropic".into(),
    }
}

// ── Task ─────────────────────────────────────────────────────────────

/// A task to be executed by deployed agents.
///
/// Args:
///     description: Human-readable description of what to accomplish
#[pyclass(name = "Task", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyTask {
    pub(crate) inner: Task,
}

#[pymethods]
impl PyTask {
    #[new]
    fn new(description: String) -> Self {
        Self {
            inner: Task::new(description),
        }
    }

    #[getter]
    fn description(&self) -> &str {
        &self.inner.description
    }

    fn __repr__(&self) -> String {
        format!("Task('{}')", self.inner.description)
    }
}

// ── HiveMind ─────────────────────────────────────────────────────────

/// The central orchestrator of PulseHive.
///
/// Owns the substrate, LLM providers, and event bus. Create via builder pattern:
///
///     hive = HiveMind.builder() \\
///         .substrate_path("/tmp/my_project.db") \\
///         .llm_provider("openai", openai_provider("sk-...")) \\
///         .build()
#[pyclass(name = "HiveMind")]
pub struct PyHiveMind {
    // Arc-wrapped so we can clone into async futures (PyO3 requires 'static)
    pub(crate) inner: Arc<HiveMind>,
}

#[pymethods]
impl PyHiveMind {
    /// Create a new builder for constructing a HiveMind.
    #[staticmethod]
    fn builder() -> PyHiveMindBuilder {
        PyHiveMindBuilder {
            substrate_path: None,
            providers: Vec::new(),
        }
    }

    /// Deploy agents to execute tasks. Returns an async event stream.
    ///
    /// Usage::
    ///
    ///     stream = await hive.deploy([agent], [Task("Analyze code")])
    ///     async for event in stream:
    ///         print(event.event_type, event.data)
    ///
    /// Args:
    ///     agents: List of AgentDefinition objects
    ///     tasks: List of Task objects
    fn deploy<'py>(
        &self,
        py: Python<'py>,
        agents: Vec<PyAgentDefinition>,
        tasks: Vec<PyTask>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let rust_agents = agents.into_iter().map(|a| a.inner).collect();
        let rust_tasks = tasks.into_iter().map(|t| t.inner).collect();
        let hive = Arc::clone(&self.inner);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let stream = hive
                .deploy(rust_agents, rust_tasks)
                .await
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

            Ok(PyEventStream {
                stream: Arc::new(Mutex::new(stream)),
            })
        })
    }

    /// Signal shutdown to all background tasks.
    fn shutdown(&self) {
        self.inner.shutdown();
    }

    /// Returns true if shutdown has been signaled.
    fn is_shutdown(&self) -> bool {
        self.inner.is_shutdown()
    }

    fn __repr__(&self) -> String {
        "HiveMind(active)".to_string()
    }
}

// ── HiveMindBuilder ──────────────────────────────────────────────────

/// Builder for constructing a HiveMind with validated configuration.
///
/// Methods return self for chaining:
///
///     hive = HiveMind.builder() \\
///         .substrate_path("/tmp/test.db") \\
///         .llm_provider("openai", openai_provider("sk-...")) \\
///         .build()
#[pyclass(name = "HiveMindBuilder")]
pub struct PyHiveMindBuilder {
    substrate_path: Option<String>,
    providers: Vec<(String, Arc<dyn LlmProvider>)>,
}

#[pymethods]
impl PyHiveMindBuilder {
    /// Set the PulseDB substrate file path. Returns self for chaining.
    fn substrate_path(slf: Py<Self>, path: String, py: Python<'_>) -> Py<Self> {
        slf.borrow_mut(py).substrate_path = Some(path);
        slf
    }

    /// Register a named LLM provider. Returns self for chaining.
    ///
    /// Args:
    ///     name: Provider name (e.g., "openai", "anthropic")
    ///     provider: Provider created via openai_provider() or anthropic_provider()
    fn llm_provider(
        slf: Py<Self>,
        name: String,
        provider: &PyLlmProviderProxy,
        py: Python<'_>,
    ) -> Py<Self> {
        slf.borrow_mut(py)
            .providers
            .push((name, Arc::clone(&provider.inner)));
        slf
    }

    /// Build the HiveMind. Validates that a substrate is configured.
    ///
    /// Raises RuntimeError if substrate is not configured.
    fn build(&self) -> PyResult<PyHiveMind> {
        let Some(path) = &self.substrate_path else {
            return Err(PyRuntimeError::new_err(
                "Substrate not configured. Call .substrate_path() before .build()",
            ));
        };

        let mut builder = HiveMind::builder().substrate_path(path);
        for (name, provider) in &self.providers {
            // We need to pass an owned provider. Clone the Arc contents by wrapping in a newtype.
            builder = builder.llm_provider(name, ArcProvider(Arc::clone(provider)));
        }

        match builder.build() {
            Ok(hive) => Ok(PyHiveMind { inner: Arc::new(hive) }),
            Err(e) => Err(PyRuntimeError::new_err(e.to_string())),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "HiveMindBuilder(substrate={:?}, providers={})",
            self.substrate_path,
            self.providers.len()
        )
    }
}

/// Newtype wrapper to pass Arc<dyn LlmProvider> to builder.llm_provider()
/// which expects `impl LlmProvider + 'static`.
struct ArcProvider(Arc<dyn LlmProvider>);

#[async_trait::async_trait]
impl LlmProvider for ArcProvider {
    async fn chat(
        &self,
        messages: Vec<pulsehive_core::llm::Message>,
        tools: Vec<pulsehive_core::llm::ToolDefinition>,
        config: &pulsehive_core::llm::LlmConfig,
    ) -> pulsehive_core::error::Result<pulsehive_core::llm::LlmResponse> {
        self.0.chat(messages, tools, config).await
    }

    async fn chat_stream(
        &self,
        messages: Vec<pulsehive_core::llm::Message>,
        tools: Vec<pulsehive_core::llm::ToolDefinition>,
        config: &pulsehive_core::llm::LlmConfig,
    ) -> pulsehive_core::error::Result<
        std::pin::Pin<
            Box<dyn futures_core::Stream<Item = pulsehive_core::error::Result<pulsehive_core::llm::LlmChunk>> + Send>,
        >,
    > {
        self.0.chat_stream(messages, tools, config).await
    }
}

/// Register HiveMind classes and provider factories with the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyHiveMind>()?;
    m.add_class::<PyHiveMindBuilder>()?;
    m.add_class::<PyTask>()?;
    m.add_class::<PyLlmProviderProxy>()?;
    m.add_function(wrap_pyfunction!(openai_provider, m)?)?;
    m.add_function(wrap_pyfunction!(anthropic_provider, m)?)?;
    Ok(())
}
