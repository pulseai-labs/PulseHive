//! Event export trait for streaming HiveEvents to external observability systems.
//!
//! [`EventExporter`] enables PulseHive to forward events to tools like PulseVision
//! for real-time visualization. Implementations handle the wire protocol (WebSocket,
//! HTTP, file, etc.); the SDK calls `export()` on each event emission.
//!
//! # Example
//! ```rust,ignore
//! use pulsehive_core::export::EventExporter;
//! use pulsehive_core::event::HiveEvent;
//!
//! struct FileExporter { path: PathBuf }
//!
//! #[async_trait]
//! impl EventExporter for FileExporter {
//!     async fn export(&self, event: &HiveEvent) {
//!         let json = serde_json::to_string(event).unwrap();
//!         tokio::fs::write(&self.path, json).await.ok();
//!     }
//!     async fn flush(&self) {}
//! }
//! ```

use async_trait::async_trait;

use crate::event::HiveEvent;

/// Trait for exporting HiveEvents to external observability systems.
///
/// When registered with `HiveMindBuilder::event_exporter()`, the exporter
/// receives every event emitted by the agent runtime. Export is fire-and-forget —
/// errors are logged but don't block agent execution.
///
/// Must be `Send + Sync` for use across Tokio tasks.
#[async_trait]
pub trait EventExporter: Send + Sync {
    /// Export a single event. Called for every HiveEvent emission.
    ///
    /// Implementations should be non-blocking. For network transports,
    /// consider buffering and batch-sending.
    async fn export(&self, event: &HiveEvent);

    /// Flush any buffered events. Called on HiveMind shutdown.
    async fn flush(&self);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_event_exporter_is_object_safe() {
        fn _assert_object_safe(_: &dyn EventExporter) {}
        fn _assert_arcable(_: Arc<dyn EventExporter>) {}
    }
}
