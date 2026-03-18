//! Core traits and types for PulseHive multi-agent SDK.
//!
//! This crate defines the public API surface: Agent, Tool, Lens, LlmProvider,
//! HiveEvent, ApprovalHandler, and shared error types. No runtime logic lives here.
//!
//! Most consumers should use the `pulsehive` meta-crate instead of depending
//! on this crate directly.

pub mod agent;
pub mod approval;
pub mod error;
pub mod event;
pub mod lens;
pub mod llm;
pub mod tool;

/// Re-exports of the most commonly used types.
///
/// ```rust
/// use pulsehive_core::prelude::*;
/// ```
pub mod prelude {
    // ── Core traits ──────────────────────────────────────────────────
    pub use crate::approval::ApprovalHandler;
    pub use crate::llm::LlmProvider;
    pub use crate::tool::Tool;

    // ── Agent types ──────────────────────────────────────────────────
    pub use crate::agent::{
        AgentDefinition, AgentKind, AgentKindTag, AgentOutcome, ExperienceExtractor,
        LlmAgentConfig,
    };

    // ── Approval types ───────────────────────────────────────────────
    pub use crate::approval::{ApprovalResult, AutoApprove, PendingAction};

    // ── Error types ──────────────────────────────────────────────────
    pub use crate::error::{PulseHiveError, Result};

    // ── Event types ──────────────────────────────────────────────────
    pub use crate::event::{EventBus, EventEmitter, HiveEvent};

    // ── Lens types ───────────────────────────────────────────────────
    pub use crate::lens::{ExperienceTypeTag, Lens, RecencyCurve};

    // ── LLM types ────────────────────────────────────────────────────
    pub use crate::llm::{
        LlmChunk, LlmConfig, LlmResponse, Message, TokenUsage, ToolCall, ToolDefinition,
    };

    // ── Tool types ───────────────────────────────────────────────────
    pub use crate::tool::{ToolContext, ToolResult};

    // ── PulseDB re-exports ───────────────────────────────────────────
    pub use pulsedb::{
        CollectiveId, Experience, ExperienceId, ExperienceType, InsightId, NewExperience,
        PulseDBSubstrate, RelationId, SubstrateProvider,
    };
}
