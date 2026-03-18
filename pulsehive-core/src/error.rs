//! Error types for PulseHive SDK.
//!
//! [`PulseHiveError`] is the top-level error returned by all PulseHive public APIs.
//! It wraps [`pulsedb::PulseDBError`] for seamless substrate error propagation.

use thiserror::Error;

/// Top-level error type for all PulseHive operations.
#[derive(Debug, Error)]
pub enum PulseHiveError {
    /// Error from the PulseDB storage substrate.
    ///
    /// Automatically converted from [`pulsedb::PulseDBError`] via the `?` operator.
    #[error("Substrate error: {0}")]
    Substrate(#[from] pulsedb::PulseDBError),

    /// Error from an LLM provider (API failure, rate limit, parse error).
    #[error("LLM error: {0}")]
    Llm(String),

    /// Error during tool execution.
    #[error("Tool error: {0}")]
    Tool(String),

    /// Error in agent lifecycle (deploy, loop, completion).
    #[error("Agent error: {0}")]
    Agent(String),

    /// Invalid configuration (missing substrate, no providers, etc.).
    #[error("Configuration error: {0}")]
    Config(String),

    /// Input validation failure.
    #[error("Validation error: {0}")]
    Validation(String),
}

impl PulseHiveError {
    /// Creates an LLM error.
    pub fn llm(msg: impl Into<String>) -> Self {
        Self::Llm(msg.into())
    }

    /// Creates a tool error.
    pub fn tool(msg: impl Into<String>) -> Self {
        Self::Tool(msg.into())
    }

    /// Creates an agent error.
    pub fn agent(msg: impl Into<String>) -> Self {
        Self::Agent(msg.into())
    }

    /// Creates a configuration error.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Creates a validation error.
    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }
}

/// Result type alias for PulseHive operations.
pub type Result<T> = std::result::Result<T, PulseHiveError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pulsedb_error_converts_via_from() {
        let db_err = pulsedb::PulseDBError::config("test failure");
        let hive_err: PulseHiveError = db_err.into();
        assert!(matches!(hive_err, PulseHiveError::Substrate(_)));
        assert!(hive_err.to_string().contains("test failure"));
    }

    #[test]
    fn test_question_mark_propagation() {
        fn inner() -> Result<()> {
            // Simulate a PulseDB error propagating with ?
            Err(pulsedb::PulseDBError::config("missing collective"))?
        }

        let result = inner();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PulseHiveError::Substrate(_)));
    }

    #[test]
    fn test_convenience_constructors() {
        assert_eq!(
            PulseHiveError::llm("timeout").to_string(),
            "LLM error: timeout"
        );
        assert_eq!(
            PulseHiveError::tool("not found").to_string(),
            "Tool error: not found"
        );
        assert_eq!(
            PulseHiveError::agent("max iterations").to_string(),
            "Agent error: max iterations"
        );
        assert_eq!(
            PulseHiveError::config("no substrate").to_string(),
            "Configuration error: no substrate"
        );
        assert_eq!(
            PulseHiveError::validation("empty content").to_string(),
            "Validation error: empty content"
        );
    }

    #[test]
    fn test_implements_std_error() {
        let err = PulseHiveError::llm("test");
        let _: &dyn std::error::Error = &err;
    }
}
