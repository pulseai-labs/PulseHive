//! Embedding provider abstraction for domain-specific embedding models.
//!
//! [`EmbeddingProvider`] enables products to use custom embedding models
//! (medical, code, multilingual) instead of PulseDB's built-in all-MiniLM-L6-v2.
//!
//! When set on HiveMind, PulseHive computes embeddings via the provider and
//! passes vectors to PulseDB in External mode. Products that don't set a provider
//! get PulseDB's default embeddings automatically.
//!
//! # Example
//! ```rust,ignore
//! struct OpenAIEmbeddings { client: reqwest::Client, api_key: String }
//!
//! #[async_trait]
//! impl EmbeddingProvider for OpenAIEmbeddings {
//!     async fn embed(&self, text: &str) -> Result<Vec<f32>> {
//!         // Call OpenAI embeddings API
//!         todo!()
//!     }
//!     fn dimensions(&self) -> usize { 1536 } // text-embedding-3-small
//! }
//! ```

use async_trait::async_trait;

use crate::error::Result;

/// Trait for domain-specific embedding model implementations.
///
/// Provides text-to-vector embeddings for semantic search and similarity
/// computation. When registered with HiveMind, all experiences are embedded
/// via this provider before storage in PulseDB (External mode).
///
/// Must be `Send + Sync` for concurrent use across Tokio tasks.
///
/// # Default batch implementation
///
/// `embed_batch` has a default implementation that calls `embed` sequentially.
/// Override it for providers that support native batching (e.g., OpenAI, Cohere).
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text string into a vector.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed a batch of text strings.
    ///
    /// Default implementation calls `embed` sequentially. Override for providers
    /// that support native batch embedding (significantly faster for large batches).
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// Return the dimensionality of embeddings produced by this provider.
    ///
    /// Must be constant for a given provider instance. Used to configure
    /// PulseDB's HNSW index when opening in External mode.
    fn dimensions(&self) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_embedding_provider_is_object_safe() {
        fn _assert_object_safe(_: &dyn EmbeddingProvider) {}
        fn _assert_boxable(_: Box<dyn EmbeddingProvider>) {}
        fn _assert_arcable(_: Arc<dyn EmbeddingProvider>) {}
    }

    /// Mock embedding provider for testing.
    struct MockEmbeddingProvider {
        dims: usize,
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![0.1; self.dims])
        }

        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    #[test]
    fn test_dimensions_returns_configured_value() {
        let provider = MockEmbeddingProvider { dims: 384 };
        assert_eq!(provider.dimensions(), 384);

        let provider = MockEmbeddingProvider { dims: 1536 };
        assert_eq!(provider.dimensions(), 1536);
    }

    #[tokio::test]
    async fn test_embed_returns_correct_length() {
        let provider = MockEmbeddingProvider { dims: 384 };
        let result = provider.embed("test text").await.unwrap();
        assert_eq!(result.len(), 384);
    }

    #[tokio::test]
    async fn test_embed_batch_default_impl() {
        let provider = MockEmbeddingProvider { dims: 384 };
        let texts = &["hello", "world", "test"];
        let results = provider.embed_batch(texts).await.unwrap();
        assert_eq!(results.len(), 3);
        for result in &results {
            assert_eq!(result.len(), 384);
        }
    }

    #[tokio::test]
    async fn test_embed_batch_empty_input() {
        let provider = MockEmbeddingProvider { dims: 384 };
        let results = provider.embed_batch(&[]).await.unwrap();
        assert!(results.is_empty());
    }
}
