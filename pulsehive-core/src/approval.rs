//! Human-in-the-loop approval primitives.
//!
//! Products implement [`ApprovalHandler`] to define their approval UX.
//! The framework calls it before executing tools where
//! [`Tool::requires_approval()`](crate::tool::Tool::requires_approval) returns `true`.
//!
//! [`AutoApprove`] is the default handler that approves everything — suitable for
//! autonomous operation and MVP development.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;

/// Describes a tool invocation awaiting human approval.
#[derive(Debug, Clone)]
pub struct PendingAction {
    /// ID of the agent requesting the action.
    pub agent_id: String,
    /// Name of the tool to be executed.
    pub tool_name: String,
    /// Parameters the tool would be called with.
    pub params: Value,
    /// Human-readable description of what the tool will do.
    pub description: String,
}

/// Result of a human approval decision.
#[derive(Debug, Clone)]
pub enum ApprovalResult {
    /// Action is approved as-is.
    Approved,
    /// Action is denied with a reason (communicated back to the LLM).
    Denied { reason: String },
    /// Action is approved with modified parameters.
    Modified { new_params: Value },
}

/// Trait for handling human-in-the-loop approval requests.
///
/// Products implement this to define how approval is presented to users.
/// The framework calls [`request_approval`](ApprovalHandler::request_approval)
/// before executing any tool where `requires_approval()` returns `true`.
///
/// # Approval Flow
///
/// 1. Agent's agentic loop encounters a tool with `requires_approval() == true`
/// 2. Framework emits `HiveEvent::ToolApprovalRequested` and calls your handler
/// 3. Handler returns one of:
///    - [`ApprovalResult::Approved`] — tool executes with original parameters
///    - [`ApprovalResult::Denied`] — tool is blocked, LLM is informed of the reason
///    - [`ApprovalResult::Modified`] — tool executes with modified parameters
///
/// # CLI Example
///
/// Interactive terminal approval with all three outcome paths:
///
/// ```rust,ignore
/// use std::io::{self, Write};
/// use async_trait::async_trait;
/// use pulsehive_core::approval::*;
/// use pulsehive_core::error::Result;
///
/// struct CLIApproval;
///
/// #[async_trait]
/// impl ApprovalHandler for CLIApproval {
///     async fn request_approval(&self, action: &PendingAction) -> Result<ApprovalResult> {
///         println!("\n--- Approval Required ---");
///         println!("Agent:  {}", action.agent_id);
///         println!("Tool:   {}", action.tool_name);
///         println!("Params: {}", action.params);
///         println!("Desc:   {}", action.description);
///         print!("[a]pprove / [d]eny / [m]odify: ");
///         io::stdout().flush().unwrap();
///
///         let mut input = String::new();
///         io::stdin().read_line(&mut input).unwrap();
///
///         match input.trim() {
///             "a" | "approve" => Ok(ApprovalResult::Approved),
///             "d" | "deny" => Ok(ApprovalResult::Denied {
///                 reason: "Operator denied the action".into(),
///             }),
///             "m" | "modify" => {
///                 // Example: force safe_mode on all approved actions
///                 let mut params = action.params.clone();
///                 if let Some(obj) = params.as_object_mut() {
///                     obj.insert("safe_mode".into(), serde_json::Value::Bool(true));
///                 }
///                 Ok(ApprovalResult::Modified { new_params: params })
///             }
///             _ => Ok(ApprovalResult::Denied {
///                 reason: "Unrecognized input — defaulting to deny".into(),
///             }),
///         }
///     }
/// }
/// ```
///
/// # Slack / Webhook Example
///
/// ```rust,ignore
/// struct SlackApproval { channel: String }
///
/// #[async_trait]
/// impl ApprovalHandler for SlackApproval {
///     async fn request_approval(&self, action: &PendingAction) -> Result<ApprovalResult> {
///         // Post to Slack, wait for reaction, return result
///         todo!()
///     }
/// }
/// ```
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// Request approval for a pending action.
    ///
    /// Called by the agentic loop before executing a tool with `requires_approval() == true`.
    /// Implementations should present the action to a human and return their decision.
    async fn request_approval(&self, action: &PendingAction) -> Result<ApprovalResult>;
}

/// Default approval handler that approves all actions automatically.
///
/// Used when no custom handler is provided to [`HiveMind`](crate) builder.
/// Suitable for autonomous operation, testing, and MVP development.
#[derive(Debug, Clone, Default)]
pub struct AutoApprove;

#[async_trait]
impl ApprovalHandler for AutoApprove {
    async fn request_approval(&self, _action: &PendingAction) -> Result<ApprovalResult> {
        Ok(ApprovalResult::Approved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approval_handler_is_object_safe() {
        let _: Box<dyn ApprovalHandler> = Box::new(AutoApprove);
    }

    #[tokio::test]
    async fn test_auto_approve_returns_approved() {
        let handler = AutoApprove;
        let action = PendingAction {
            agent_id: "agent-1".into(),
            tool_name: "delete_file".into(),
            params: serde_json::json!({"path": "/tmp/test"}),
            description: "Delete temporary file".into(),
        };

        let result = handler.request_approval(&action).await.unwrap();
        assert!(matches!(result, ApprovalResult::Approved));
    }

    #[test]
    fn test_approval_result_variants() {
        let approved = ApprovalResult::Approved;
        assert!(matches!(approved, ApprovalResult::Approved));

        let denied = ApprovalResult::Denied {
            reason: "Too dangerous".into(),
        };
        assert!(matches!(denied, ApprovalResult::Denied { .. }));

        let modified = ApprovalResult::Modified {
            new_params: serde_json::json!({"safe_mode": true}),
        };
        assert!(matches!(modified, ApprovalResult::Modified { .. }));
    }

    #[test]
    fn test_pending_action_fields() {
        let action = PendingAction {
            agent_id: "test-agent".into(),
            tool_name: "run_query".into(),
            params: serde_json::json!({"sql": "SELECT 1"}),
            description: "Execute a read-only query".into(),
        };
        assert_eq!(action.agent_id, "test-agent");
        assert_eq!(action.tool_name, "run_query");
        assert_eq!(action.description, "Execute a read-only query");
    }
}
