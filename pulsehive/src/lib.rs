//! PulseHive — Shared Consciousness SDK for Multi-Agent AI Systems.
//!
//! This is the meta-crate that re-exports `pulsehive-core`, with
//! `pulsehive-runtime` and the LLM providers behind feature flags.
//!
//! # Profiles
//!
//! - **bare** — core traits and types only (`Agent`, `Tool`, `Lens`,
//!   `LlmProvider`, `HiveEvent`, …); no runtime, no storage substrate.
//! - **`runtime`** — `HiveMind`, the agentic loop and the PulseDB substrate
//!   (`dep:pulsehive-runtime` + `pulsehive-core/substrate`).
//! - **`openai` / `anthropic`** — transport-only LLM providers; either resolves
//!   without `pulsehive-runtime` or `pulsehive-db` on its own.
//!
//! # Quick Start
//! ```toml
//! [dependencies]
//! pulsehive = { version = "2.0", features = ["openai", "runtime"] }
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
#[cfg(feature = "runtime")]
pub use pulsehive_runtime::experience::DefaultExperienceExtractor;
#[cfg(feature = "runtime")]
pub use pulsehive_runtime::hivemind::{HiveMind, HiveMindBuilder, Task};

// Feature-gated provider re-exports
#[cfg(feature = "openai")]
pub use pulsehive_openai;

#[cfg(feature = "anthropic")]
pub use pulsehive_anthropic;
