//! Python Tool bridge — enables Python-defined tools to implement the Rust Tool trait.
//!
//! Architecture:
//! - `PyToolContext`: read-only context passed to Python tool execute()
//! - `PyToolResult`: tagged-class for tool execution results
//! - `PythonToolBridge`: Rust struct implementing `Tool` trait, backed by a Python object
//!
//! The bridge uses `Py<PyAny>` (GIL-independent, `Send + Sync`) to hold Python tool
//! objects. Metadata is cached at construction time; only `execute()` acquires the GIL.

use async_trait::async_trait;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use serde_json::Value;

use pulsehive_core::error::{PulseHiveError, Result};
use pulsehive_core::tool::{Tool, ToolContext, ToolResult};

// ── PyToolContext ────────────────────────────────────────────────────

/// Runtime context available to Python tools during execution.
///
/// Provides the agent's identity. Substrate access is not yet available
/// from Python (planned for a future sprint).
///
/// Properties:
///     agent_id: ID of the agent executing this tool
///     collective_id: Collective (namespace) the agent belongs to
#[pyclass(name = "ToolContext", frozen)]
pub struct PyToolContext {
    agent_id: String,
    collective_id: String,
}

impl PyToolContext {
    /// Create from a Rust ToolContext (called internally by PythonToolBridge).
    pub fn from_rust(ctx: &ToolContext) -> Self {
        Self {
            agent_id: ctx.agent_id.clone(),
            collective_id: ctx.collective_id.to_string(),
        }
    }
}

#[pymethods]
impl PyToolContext {
    /// Agent ID executing this tool.
    #[getter]
    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Collective (namespace) UUID.
    #[getter]
    fn collective_id(&self) -> &str {
        &self.collective_id
    }

    fn __repr__(&self) -> String {
        format!(
            "ToolContext(agent_id='{}', collective_id='{}')",
            self.agent_id, self.collective_id
        )
    }
}

// ── PyToolResult ─────────────────────────────────────────────────────

/// Result of a tool execution.
///
/// Create via static methods:
///     ToolResult.text("hello")
///     ToolResult.json({"key": "value"})
///     ToolResult.error("something went wrong")
#[pyclass(name = "ToolResult", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyToolResult {
    kind: String,
    content: String,
}

#[pymethods]
impl PyToolResult {
    /// Create a text result.
    #[staticmethod]
    fn text(content: String) -> Self {
        Self {
            kind: "text".into(),
            content,
        }
    }

    /// Create a JSON result from a dict.
    #[staticmethod]
    fn json(data: &Bound<'_, PyDict>) -> PyResult<Self> {
        // Convert Python dict to JSON string
        let json_str = pythonize_dict(data)?;
        Ok(Self {
            kind: "json".into(),
            content: json_str,
        })
    }

    /// Create an error result.
    #[staticmethod]
    fn error(message: String) -> Self {
        Self {
            kind: "error".into(),
            content: message,
        }
    }

    /// Result kind: "text", "json", or "error".
    #[getter]
    fn kind(&self) -> &str {
        &self.kind
    }

    /// Result content (text, JSON string, or error message).
    #[getter]
    fn content(&self) -> &str {
        &self.content
    }

    fn __repr__(&self) -> String {
        let preview = if self.content.len() > 50 {
            format!("{}...", &self.content[..50])
        } else {
            self.content.clone()
        };
        format!("ToolResult.{}('{}')", self.kind, preview)
    }
}

impl From<ToolResult> for PyToolResult {
    fn from(r: ToolResult) -> Self {
        match r {
            ToolResult::Text(s) => PyToolResult {
                kind: "text".into(),
                content: s,
            },
            ToolResult::Json(v) => PyToolResult {
                kind: "json".into(),
                content: v.to_string(),
            },
            ToolResult::Error(s) => PyToolResult {
                kind: "error".into(),
                content: s,
            },
        }
    }
}

impl From<PyToolResult> for ToolResult {
    fn from(r: PyToolResult) -> Self {
        match r.kind.as_str() {
            "json" => {
                if let Ok(v) = serde_json::from_str(&r.content) {
                    ToolResult::Json(v)
                } else {
                    ToolResult::Text(r.content)
                }
            }
            "error" => ToolResult::Error(r.content),
            _ => ToolResult::Text(r.content),
        }
    }
}

// ── PythonToolBridge ─────────────────────────────────────────────────

/// Rust `Tool` implementation backed by a Python object.
///
/// Holds `Py<PyAny>` which is `Send + Sync` (GIL-independent reference).
/// Metadata is cached at construction; only `execute()` acquires the GIL.
pub struct PythonToolBridge {
    py_tool: Py<PyAny>,
    name: String,
    description: String,
    parameters: Value,
    requires_approval: bool,
}

// Py<PyAny> is Send + Sync, and all other fields are owned Rust types.
// This is safe because we only access the Python object inside Python::with_gil.
unsafe impl Send for PythonToolBridge {}
unsafe impl Sync for PythonToolBridge {}

impl PythonToolBridge {
    /// Create a new bridge from a Python tool object.
    ///
    /// Validates that the object has the required methods (name, description,
    /// parameters, execute) and caches metadata. Raises TypeError if methods
    /// are missing.
    pub fn new(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        // Validate required methods exist
        for method in &["name", "description", "parameters", "execute"] {
            if !obj.hasattr(*method)? {
                return Err(PyTypeError::new_err(format!(
                    "Tool object is missing required method '{}'. \
                     Tools must implement: name(), description(), parameters(), execute(params, context)",
                    method
                )));
            }
        }

        // Cache metadata from Python (avoids GIL on hot path)
        let name: String = obj.call_method0("name")?.extract()?;
        let description: String = obj.call_method0("description")?.extract()?;

        // parameters() returns a Python dict — convert to serde_json::Value
        let params_obj = obj.call_method0("parameters")?;
        let parameters: Value = python_obj_to_json(py, &params_obj)?;

        // requires_approval() is optional (default false)
        let requires_approval = if obj.hasattr("requires_approval")? {
            obj.call_method0("requires_approval")?
                .extract::<bool>()
                .unwrap_or(false)
        } else {
            false
        };

        Ok(Self {
            py_tool: obj.clone().unbind(),
            name,
            description,
            parameters,
            requires_approval,
        })
    }
}

#[async_trait]
impl Tool for PythonToolBridge {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn requires_approval(&self) -> bool {
        self.requires_approval
    }

    async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult> {
        let py_context = PyToolContext::from_rust(context);
        let tool_name = self.name.clone();

        // Acquire GIL and call Python execute()
        // Note: py_tool is Py<PyAny> which is safe to access inside with_gil
        let py_tool_ref = &self.py_tool;

        Python::attach(|py| {
            // Convert params Value → Python dict
            let params_py = json_to_python(py, &params)
                .map_err(|e| PulseHiveError::tool(format!("Failed to convert params: {e}")))?;

            let context_py = Py::new(py, py_context)
                .map_err(|e| PulseHiveError::tool(format!("Failed to create context: {e}")))?;

            // Call Python execute(params, context)
            let result = py_tool_ref.call_method1(py, "execute", (params_py, context_py));

            match result {
                Ok(py_result) => {
                    let bound: &Bound<'_, PyAny> = py_result.bind(py);
                    convert_python_result(bound)
                }
                Err(e) => {
                    // Python exception → ToolResult::Error so agentic loop continues
                    Ok(ToolResult::error(format!(
                        "Python tool '{}' raised: {}",
                        tool_name, e
                    )))
                }
            }
        })
    }
}

/// Convert a Python return value to a Rust ToolResult.
fn convert_python_result(obj: &Bound<'_, PyAny>) -> Result<ToolResult> {
    // None → error
    if obj.is_none() {
        return Ok(ToolResult::error("Tool returned None"));
    }

    // str → Text
    if let Ok(s) = obj.extract::<String>() {
        return Ok(ToolResult::text(s));
    }

    // dict → Json
    if let Ok(dict) = obj.cast::<PyDict>() {
        let json_str = pythonize_dict(dict)
            .map_err(|e| PulseHiveError::tool(format!("Failed to serialize dict result: {e}")))?;
        let value: Value = serde_json::from_str(&json_str)
            .map_err(|e| PulseHiveError::tool(format!("Failed to parse JSON: {e}")))?;
        return Ok(ToolResult::json(value));
    }

    // Unsupported type
    let type_name = obj
        .get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Ok(ToolResult::error(format!(
        "Unsupported return type '{}'. Tool execute() should return str or dict.",
        type_name
    )))
}

// ── JSON Conversion Helpers ──────────────────────────────────────────

/// Convert a Python dict to a JSON string.
fn pythonize_dict(dict: &Bound<'_, PyDict>) -> PyResult<String> {
    let json_mod = dict.py().import("json")?;
    let result = json_mod.call_method1("dumps", (dict,))?;
    result.extract()
}

/// Convert a Python object to serde_json::Value via json.dumps.
fn python_obj_to_json(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    let json_mod = py.import("json")?;
    let json_str: String = json_mod.call_method1("dumps", (obj,))?.extract()?;
    serde_json::from_str(&json_str)
        .map_err(|e| PyTypeError::new_err(format!("Failed to parse parameters as JSON: {e}")))
}

/// Convert serde_json::Value to a Python object via json.loads.
fn json_to_python<'py>(py: Python<'py>, value: &Value) -> PyResult<Bound<'py, PyAny>> {
    let json_mod = py.import("json")?;
    let json_str = value.to_string();
    json_mod.call_method1("loads", (json_str,))
}

/// Register tool classes with the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyToolContext>()?;
    m.add_class::<PyToolResult>()?;
    Ok(())
}
