//! Shared type definitions for the core module
//!
//! This module contains all shared types used across the core submodules:
//! - MediaAttachment: Multimodal content support
//! - CapturedContext: Context from active application
//! - CompressionStats: Memory compression statistics
//! - RequestContext: Last request context for retry
//! - StorageHelper: Async memory storage helper
//! - MemoryEntryFFI: Memory entry for FFI
//! - AppMemoryInfo: App memory info for UI

use crate::config::Config;
use crate::memory::database::VectorDatabase;
use std::sync::{Arc, Mutex};

/// Context for last request (used for retry)
#[derive(Debug, Clone)]
pub(crate) struct RequestContext {
    pub clipboard_content: String,
    pub provider: String,
    pub retry_count: u32,
}

/// Media attachment for multimodal content (add-multimodal-content-support)
/// Supports images, videos, and files from clipboard
#[derive(Debug, Clone)]
pub struct MediaAttachment {
    pub media_type: String,   // "image", "video", "file"
    pub mime_type: String,    // "image/png", "image/jpeg", "video/mp4", etc.
    pub data: String,         // Base64-encoded content
    pub filename: Option<String>, // Optional original filename
    pub size_bytes: u64,      // Original size in bytes for logging/validation
}

/// Captured context from active application (Swift → Rust)
#[derive(Debug, Clone)]
pub struct CapturedContext {
    pub app_bundle_id: String,
    pub window_title: Option<String>,
    pub attachments: Option<Vec<MediaAttachment>>, // Multimodal content support
    pub topic_id: Option<String>, // Topic ID for multi-turn conversations
}

/// Statistics about memory compression state
///
/// Used for displaying compression status in Settings UI
#[derive(Debug, Clone)]
pub struct CompressionStats {
    /// Total number of raw memories (Layer 1)
    pub total_raw_memories: u64,
    /// Total number of compressed facts (Layer 2)
    pub total_facts: u64,
    /// Number of valid (non-invalidated) facts
    pub valid_facts: u64,
    /// Breakdown by fact type (preference, plan, learning, etc.)
    pub facts_by_type: std::collections::HashMap<String, u64>,
}

/// Memory entry type for FFI (UniFFI-compatible)
#[derive(Debug, Clone)]
pub struct MemoryEntryFFI {
    pub id: String,
    pub app_bundle_id: String,
    pub window_title: String,
    pub user_input: String,
    pub ai_output: String,
    pub timestamp: i64,
    pub similarity_score: Option<f32>,
}

/// App memory info for UI filtering (UniFFI-compatible)
#[derive(Debug, Clone)]
pub struct AppMemoryInfo {
    pub app_bundle_id: String,
    pub memory_count: u64,
}

/// Helper struct for async memory storage
///
/// This creates a lightweight clone that can be moved into async tasks
/// for non-blocking memory storage operations.
#[derive(Clone)]
pub(crate) struct StorageHelper {
    pub config: Arc<Mutex<Config>>,
    pub memory_db: Option<Arc<VectorDatabase>>,
    pub current_context: Arc<Mutex<Option<CapturedContext>>>,
}

impl StorageHelper {
    /// Acquires the config mutex lock with poison recovery.
    #[inline(always)]
    fn lock_config(&self) -> std::sync::MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ensures the memory database is initialized and returns a reference to it.
    #[inline(always)]
    fn require_memory_db(&self) -> crate::error::Result<&Arc<VectorDatabase>> {
        self.memory_db
            .as_ref()
            .ok_or_else(|| crate::error::AetherError::config("Memory database not initialized"))
    }

    /// Store interaction memory (used in async context)
    ///
    /// IMPORTANT: This is an async function because it's called from within
    /// a tokio::spawn() task. Using block_on() inside an async context would
    /// cause a panic: "Cannot start a runtime from within a runtime".
    pub async fn store_interaction_memory(
        &self,
        user_input: String,
        ai_output: String,
    ) -> crate::error::Result<String> {
        use crate::memory::context::ContextAnchor;
        use crate::memory::embedding::EmbeddingModel;
        use crate::memory::ingestion::MemoryIngestion;

        // Extract all needed data from locks before any await point
        // MutexGuard is not Send, so we must drop it before await
        let (memory_config, context_anchor, db) = {
            // Check if memory is enabled
            let config = self.lock_config();
            if !config.memory.enabled {
                return Err(crate::error::AetherError::config("Memory is disabled"));
            }

            // Get current context
            let current_context = self.current_context.lock().unwrap_or_else(|e| {
                tracing::warn!("Mutex poisoned in current_context (StorageHelper::store_interaction_memory), recovering");
                e.into_inner()
            });
            let captured_context = current_context
                .as_ref()
                .ok_or_else(|| crate::error::AetherError::config("No context captured"))?;

            // Create context anchor with topic_id from captured context
            let context_anchor = ContextAnchor {
                app_bundle_id: captured_context.app_bundle_id.clone(),
                window_title: captured_context.window_title.clone().unwrap_or_default(),
                timestamp: chrono::Utc::now().timestamp(),
                topic_id: captured_context
                    .topic_id
                    .clone()
                    .unwrap_or_else(|| crate::memory::context::SINGLE_TURN_TOPIC_ID.to_string()),
            };

            // Get memory database
            let db = self.require_memory_db()?.clone();

            // Clone memory config for use after lock is dropped
            let memory_config = config.memory.clone();

            (memory_config, context_anchor, db)
        }; // All locks are dropped here

        // Get embedding model directory
        let model_dir = super::AetherCore::get_embedding_model_dir().map_err(|e| {
            crate::error::AetherError::config(format!(
                "Failed to get embedding model directory: {}",
                e
            ))
        })?;

        // Create embedding model (lazy load)
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            crate::error::AetherError::config(format!(
                "Failed to initialize embedding model: {}",
                e
            ))
        })?);

        // Create ingestion service
        let ingestion = MemoryIngestion::new(db, embedding_model, Arc::new(memory_config));

        // Store memory - use await instead of block_on since we're in async context
        ingestion
            .store_memory(context_anchor, &user_input, &ai_output)
            .await
    }
}
