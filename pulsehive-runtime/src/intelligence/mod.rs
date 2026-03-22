//! Intelligence layer — automatic relationship inference and insight synthesis.
//!
//! When experiences are recorded, the intelligence layer:
//! 1. **RelationshipDetector** finds semantically similar experiences and creates
//!    typed relations (Supports, Contradicts, Supersedes, Implies, etc.)
//! 2. **InsightSynthesizer** detects clusters of related experiences and uses an
//!    LLM to synthesize consolidated insights

pub mod context;
pub mod insight;
pub mod relationship;
