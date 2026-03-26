//! PulseHive — Shared Consciousness SDK for Multi-Agent AI Systems.
//!
//! This is the meta-crate that re-exports `pulsehive-core` and `pulsehive-runtime`.
//! Use feature flags `openai` and `anthropic` to include LLM providers.
//!
//! # Quick Start
//! ```toml
//! [dependencies]
//! pulsehive = { version = "2.0", features = ["openai"] }
//! ```
//!
//! ```rust,ignore
//! use pulsehive::prelude::*;
//! use pulsehive::HiveMind;
//!
//! let hive = HiveMind::builder()
//!     .substrate_path("my_project.db")
//!     .llm_provider("openai", my_provider)
//!     .build()?;
//! ```

// Re-export all core modules and prelude
pub use pulsehive_core::*;

// Re-export runtime types at top level for convenience
pub use pulsehive_runtime::experience::DefaultExperienceExtractor;
pub use pulsehive_runtime::hivemind::{HiveMind, HiveMindBuilder, Task};

// Feature-gated provider re-exports
#[cfg(feature = "openai")]
pub use pulsehive_openai;

#[cfg(feature = "anthropic")]
pub use pulsehive_anthropic;
