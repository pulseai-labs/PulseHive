//! Tool bridge: JsToolBridge via ThreadsafeFunction.
//!
//! Architecture:
//! - `JsToolContext`: read-only context passed to JS tool execute()
//! - `JsToolResult`: tagged-class for tool execution results
//! - `JsToolHandle` (exposed as `Tool`): napi class wrapping a JS tool callback
//! - `JsToolBridge`: internal struct implementing Rust `Tool` trait, backed by ThreadsafeFunction
//!
//! The bridge caches metadata at construction time. Only `execute()` crosses the
//! Rust↔JS boundary via ThreadsafeFunction (Send + Sync, scheduled on Node event loop).

#[cfg(feature = "napi")]
use napi::threadsafe_function::ThreadsafeFunction;
#[cfg(feature = "napi")]
use napi_derive::napi;
#[cfg(feature = "napi")]
use serde_json::Value;

use pulsehive_core::tool::{ToolContext, ToolResult};

// ── JsToolContext ────────────────────────────────────────────────────

/// Runtime context available to tools during execution.
///
/// Provides the agent's identity. Substrate access is not yet available
/// from JavaScript (planned for a future sprint).
#[cfg_attr(feature = "napi", napi(js_name = "ToolContext"))]
pub struct JsToolContext {
    agent_id: String,
    collective_id: String,
}

impl JsToolContext {
    /// Create from a Rust ToolContext (called internally by JsToolBridge).
    pub fn from_rust(ctx: &ToolContext) -> Self {
        Self {
            agent_id: ctx.agent_id.clone(),
            collective_id: ctx.collective_id.to_string(),
        }
    }
}

#[cfg_attr(feature = "napi", napi)]
impl JsToolContext {
    /// Agent ID executing this tool.
    #[cfg_attr(feature = "napi", napi(getter, js_name = "agentId"))]
    pub fn agent_id(&self) -> String {
        self.agent_id.clone()
    }

    /// Collective (namespace) UUID.
    #[cfg_attr(feature = "napi", napi(getter, js_name = "collectiveId"))]
    pub fn collective_id(&self) -> String {
        self.collective_id.clone()
    }

    /// String representation for debugging.
    #[cfg_attr(feature = "napi", napi(js_name = "toString"))]
    pub fn to_string_js(&self) -> String {
        format!(
            "ToolContext(agentId='{}', collectiveId='{}')",
            self.agent_id, self.collective_id
        )
    }
}

// ── JsToolResult ─────────────────────────────────────────────────────

/// Result of a tool execution.
///
/// Create via static methods:
///   - `ToolResult.text("hello")`
///   - `ToolResult.json('{"key":"value"}')`
///   - `ToolResult.error("something went wrong")`
#[cfg_attr(feature = "napi", napi(js_name = "ToolResult"))]
pub struct JsToolResult {
    kind: String,
    content: String,
}

#[cfg_attr(feature = "napi", napi)]
impl JsToolResult {
    /// Create a text result.
    #[cfg_attr(feature = "napi", napi(factory))]
    pub fn text(content: String) -> Self {
        Self {
            kind: "text".into(),
            content,
        }
    }

    /// Create a JSON result from a stringified object.
    #[cfg_attr(feature = "napi", napi(factory))]
    pub fn json(data: String) -> Self {
        Self {
            kind: "json".into(),
            content: data,
        }
    }

    /// Create an error result.
    #[cfg_attr(feature = "napi", napi(factory))]
    pub fn error(message: String) -> Self {
        Self {
            kind: "error".into(),
            content: message,
        }
    }

    /// Result kind: "text", "json", or "error".
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn kind(&self) -> String {
        self.kind.clone()
    }

    /// Result content (text, JSON string, or error message).
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn content(&self) -> String {
        self.content.clone()
    }

    /// String representation for debugging.
    #[cfg_attr(feature = "napi", napi(js_name = "toString"))]
    pub fn to_string_js(&self) -> String {
        let preview = if self.content.len() > 50 {
            format!("{}...", crate::events::truncate_chars(&self.content, 50))
        } else {
            self.content.clone()
        };
        format!("ToolResult.{}('{}')", self.kind, preview)
    }
}

impl From<ToolResult> for JsToolResult {
    fn from(r: ToolResult) -> Self {
        match r {
            ToolResult::Text(s) => Self {
                kind: "text".into(),
                content: s,
            },
            ToolResult::Json(v) => Self {
                kind: "json".into(),
                content: v.to_string(),
            },
            ToolResult::Error(s) => Self {
                kind: "error".into(),
                content: s,
            },
        }
    }
}

impl From<JsToolResult> for ToolResult {
    fn from(r: JsToolResult) -> Self {
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

// ── JsToolHandle (exposed as "Tool" in JS) ──────────────────────────

/// Tool definition with a JavaScript execute callback.
///
/// The execute callback receives a single JSON string containing both params and context:
/// ```typescript
/// const tool = new Tool(
///     "calculator",
///     "Performs arithmetic",
///     '{"type":"object","properties":{"expr":{"type":"string"}}}',
///     async (payloadJson: string) => {
///         const { params, context } = JSON.parse(payloadJson);
///         return `Result: ${eval(params.expr)}`;
///     }
/// );
/// ```
#[cfg(feature = "napi")]
#[napi(js_name = "Tool")]
pub struct JsToolHandle {
    pub(crate) name_val: String,
    pub(crate) description_val: String,
    pub(crate) parameters_val: Value,
    pub(crate) requires_approval_val: bool,
    /// ThreadsafeFunction: (payloadJson: string) => string | Promise<string>
    /// Payload is JSON: {"params": {...}, "context": {"agentId": "...", "collectiveId": "..."}}
    pub(crate) execute_fn: std::sync::Arc<ThreadsafeFunction<String, String>>,
}

#[cfg(feature = "napi")]
#[napi]
impl JsToolHandle {
    /// Create a new Tool.
    ///
    /// @param name - Tool name shown to the LLM for selection
    /// @param description - Description the LLM uses to decide when to invoke this tool
    /// @param parametersJson - JSON Schema describing the tool's parameters (as JSON string)
    /// @param executeFn - Callback: (payloadJson: string) => string | Promise\<string\>.
    ///   payloadJson contains `{"params": {...}, "context": {"agentId": "...", "collectiveId": "..."}}`.
    ///   Return the tool result as a string (or JSON string for structured results).
    /// @param requiresApproval - Whether this tool requires human approval before execution
    #[napi(constructor)]
    pub fn new(
        name: String,
        description: String,
        parameters_json: String,
        #[napi(ts_arg_type = "(payloadJson: string) => string | Promise<string>")]
        execute_fn: ThreadsafeFunction<String, String>,
        requires_approval: Option<bool>,
    ) -> napi::Result<Self> {
        let parameters_val: Value = serde_json::from_str(&parameters_json).map_err(|e| {
            napi::Error::new(
                napi::Status::InvalidArg,
                format!("Invalid JSON in parameters: {e}"),
            )
        })?;

        Ok(Self {
            name_val: name,
            description_val: description,
            parameters_val,
            requires_approval_val: requires_approval.unwrap_or(false),
            execute_fn: std::sync::Arc::new(execute_fn),
        })
    }

    /// Tool name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.name_val.clone()
    }

    /// Tool description.
    #[napi(getter)]
    pub fn description(&self) -> String {
        self.description_val.clone()
    }

    /// Whether this tool requires human approval.
    #[napi(getter, js_name = "requiresApproval")]
    pub fn requires_approval(&self) -> bool {
        self.requires_approval_val
    }

    /// String representation for debugging.
    #[napi(js_name = "toString")]
    pub fn to_string_js(&self) -> String {
        format!(
            "Tool('{}', requiresApproval={})",
            self.name_val, self.requires_approval_val
        )
    }
}

// ── JsToolBridge (internal, implements Rust Tool trait) ──────────────

/// Internal bridge from a `JsToolHandle` to the Rust `Tool` trait.
///
/// `ThreadsafeFunction` is `Send + Sync` by design — it schedules calls
/// on Node's event loop thread, making it safe to invoke from Tokio tasks.
#[cfg(feature = "napi")]
pub struct JsToolBridge {
    name: String,
    description: String,
    parameters: Value,
    requires_approval: bool,
    execute_fn: std::sync::Arc<ThreadsafeFunction<String, String>>,
}

#[cfg(feature = "napi")]
impl JsToolBridge {
    /// Create a bridge from a JsToolHandle (clones the ThreadsafeFunction reference).
    pub fn from_handle(handle: &JsToolHandle) -> Self {
        Self {
            name: handle.name_val.clone(),
            description: handle.description_val.clone(),
            parameters: handle.parameters_val.clone(),
            requires_approval: handle.requires_approval_val,
            execute_fn: handle.execute_fn.clone(),
        }
    }
}

#[cfg(feature = "napi")]
#[async_trait::async_trait]
impl pulsehive_core::tool::Tool for JsToolBridge {
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

    async fn execute(
        &self,
        params: Value,
        context: &ToolContext,
    ) -> pulsehive_core::error::Result<ToolResult> {
        // Serialize params and context into a single JSON payload
        let payload = serde_json::json!({
            "params": params,
            "context": {
                "agentId": context.agent_id,
                "collectiveId": context.collective_id.to_string(),
            }
        });
        let payload_json = serde_json::to_string(&payload).map_err(|e| {
            pulsehive_core::error::PulseHiveError::tool(format!("Serialize payload: {e}"))
        })?;

        // Call the JS function via ThreadsafeFunction (scheduled on Node event loop).
        // call_async returns a future that resolves when the JS function returns/Promise resolves.
        let result: String = self
            .execute_fn
            .call_async(Ok(payload_json))
            .await
            .map_err(|e| {
                pulsehive_core::error::PulseHiveError::tool(format!(
                    "JS tool '{}' raised: {}",
                    self.name, e
                ))
            })?;

        // Parse the result — if it looks like JSON, treat as Json; otherwise Text
        if result.starts_with('{') || result.starts_with('[') {
            if let Ok(v) = serde_json::from_str::<Value>(&result) {
                return Ok(ToolResult::json(v));
            }
        }
        Ok(ToolResult::text(result))
    }
}
