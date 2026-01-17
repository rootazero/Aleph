//! Memory management methods for AetherCore
//!
//! This module contains memory-related methods: search_memory, clear_memory, get_memory_stats, etc.

use super::{AetherCore, AetherFfiError, MemoryItem};
use std::path::PathBuf;
use tracing::info;

impl AetherCore {
    /// Search memory for relevant entries
    ///
    /// Searches the memory store for entries matching the query using vector similarity.
    pub fn search_memory(
        &self,
        query: String,
        limit: u32,
    ) -> Result<Vec<MemoryItem>, AetherFfiError> {
        let memory_path = self
            .memory_path
            .as_ref()
            .ok_or_else(|| AetherFfiError::Memory("Memory store not initialized".to_string()))?;

        use crate::memory::{EmbeddingModel, VectorDatabase};

        let db_path = PathBuf::from(memory_path);

        // Create embedding model and database
        let model_path = EmbeddingModel::get_default_model_path()
            .map_err(|e| AetherFfiError::Memory(e.to_string()))?;
        let embedding_model =
            EmbeddingModel::new(model_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;
        let db = VectorDatabase::new(db_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        // Generate query embedding and search
        let result = self.runtime.block_on(async {
            let query_embedding = embedding_model.embed_text(&query).await?;
            db.search_memories("", "", &query_embedding, limit).await
        });

        match result {
            Ok(entries) => Ok(entries.into_iter().map(|e| e.into()).collect()),
            Err(e) => Err(AetherFfiError::Memory(e.to_string())),
        }
    }

    /// Clear all memory entries
    pub fn clear_memory(&self) -> Result<(), AetherFfiError> {
        let memory_path = self
            .memory_path
            .as_ref()
            .ok_or_else(|| AetherFfiError::Memory("Memory store not initialized".to_string()))?;

        use crate::memory::VectorDatabase;
        let db_path = PathBuf::from(memory_path);
        let db = VectorDatabase::new(db_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        // Clear all memories (no filter)
        self.runtime
            .block_on(db.clear_memories(None, None))
            .map(|_| ())
            .map_err(|e| AetherFfiError::Memory(e.to_string()))
    }

    /// Get memory configuration
    pub fn get_memory_config(&self) -> crate::config::MemoryConfig {
        let config = self.lock_config();
        config.memory.clone()
    }

    /// Update memory configuration
    pub fn update_memory_config(
        &self,
        new_config: crate::config::MemoryConfig,
    ) -> Result<(), AetherFfiError> {
        let mut config = self.lock_config();
        config.memory = new_config;
        config
            .save()
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!("Memory configuration updated");
        Ok(())
    }

    /// Delete specific memory by ID
    pub fn delete_memory(&self, id: String) -> Result<(), AetherFfiError> {
        let memory_path = self
            .memory_path
            .as_ref()
            .ok_or_else(|| AetherFfiError::Memory("Memory store not initialized".to_string()))?;

        use crate::memory::database::VectorDatabase;
        let db_path = PathBuf::from(&memory_path);
        let db = VectorDatabase::new(db_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        self.runtime
            .block_on(db.delete_memory(&id))
            .map_err(|e| AetherFfiError::Memory(e.to_string()))
    }

    /// Get memory database statistics
    pub fn get_memory_stats(&self) -> Result<crate::memory::database::MemoryStats, AetherFfiError> {
        let memory_path = self
            .memory_path
            .as_ref()
            .ok_or_else(|| AetherFfiError::Memory("Memory store not initialized".to_string()))?;

        use crate::memory::database::VectorDatabase;
        let db_path = PathBuf::from(&memory_path);
        let db = VectorDatabase::new(db_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        self.runtime
            .block_on(db.get_stats())
            .map_err(|e| AetherFfiError::Memory(e.to_string()))
    }

    /// Get list of unique app bundle IDs from memories
    pub fn get_memory_app_list(
        &self,
    ) -> Result<Vec<crate::core::types::AppMemoryInfo>, AetherFfiError> {
        let memory_path = self
            .memory_path
            .as_ref()
            .ok_or_else(|| AetherFfiError::Memory("Memory store not initialized".to_string()))?;

        use crate::memory::database::VectorDatabase;
        let db_path = PathBuf::from(&memory_path);
        let db = VectorDatabase::new(db_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        let apps = self
            .runtime
            .block_on(db.get_app_list())
            .map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        Ok(apps
            .into_iter()
            .map(
                |(app_bundle_id, memory_count)| crate::core::types::AppMemoryInfo {
                    app_bundle_id,
                    memory_count,
                },
            )
            .collect())
    }

    /// Clear memories (with optional filters)
    pub fn clear_memories(
        &self,
        app_bundle_id: Option<String>,
        window_title: Option<String>,
    ) -> Result<u64, AetherFfiError> {
        let memory_path = self
            .memory_path
            .as_ref()
            .ok_or_else(|| AetherFfiError::Memory("Memory store not initialized".to_string()))?;

        use crate::memory::database::VectorDatabase;
        let db_path = PathBuf::from(&memory_path);
        let db = VectorDatabase::new(db_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        self.runtime
            .block_on(db.clear_memories(app_bundle_id.as_deref(), window_title.as_deref()))
            .map_err(|e| AetherFfiError::Memory(e.to_string()))
    }

    /// Clear all compressed facts (Layer 2 data)
    pub fn clear_facts(&self) -> Result<u64, AetherFfiError> {
        let memory_path = self
            .memory_path
            .as_ref()
            .ok_or_else(|| AetherFfiError::Memory("Memory store not initialized".to_string()))?;

        use crate::memory::database::VectorDatabase;
        let db_path = PathBuf::from(&memory_path);
        let db = VectorDatabase::new(db_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        self.runtime
            .block_on(db.clear_facts())
            .map_err(|e| AetherFfiError::Memory(e.to_string()))
    }

    /// Delete all memories associated with a specific topic ID
    pub fn delete_memories_by_topic_id(&self, topic_id: String) -> Result<u64, AetherFfiError> {
        let memory_path = self
            .memory_path
            .as_ref()
            .ok_or_else(|| AetherFfiError::Memory("Memory store not initialized".to_string()))?;

        use crate::memory::database::VectorDatabase;
        let db_path = PathBuf::from(&memory_path);
        let db = VectorDatabase::new(db_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        self.runtime
            .block_on(db.delete_by_topic_id(&topic_id))
            .map_err(|e| AetherFfiError::Memory(e.to_string()))
    }

    /// Get compression statistics
    pub fn get_compression_stats(
        &self,
    ) -> Result<crate::core::types::CompressionStats, AetherFfiError> {
        let memory_path = self
            .memory_path
            .as_ref()
            .ok_or_else(|| AetherFfiError::Memory("Memory store not initialized".to_string()))?;

        use crate::memory::database::VectorDatabase;
        let db_path = PathBuf::from(&memory_path);
        let db = VectorDatabase::new(db_path).map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        let stats = self
            .runtime
            .block_on(db.get_stats())
            .map_err(|e| AetherFfiError::Memory(e.to_string()))?;
        let fact_stats = self
            .runtime
            .block_on(db.get_fact_stats())
            .map_err(|e| AetherFfiError::Memory(e.to_string()))?;

        Ok(crate::core::types::CompressionStats {
            total_raw_memories: stats.total_memories,
            total_facts: fact_stats.total_facts,
            valid_facts: fact_stats.valid_facts,
            facts_by_type: fact_stats.facts_by_type,
        })
    }

    /// Manually trigger memory compression
    ///
    /// Note: In V2, compression is simplified. This is a placeholder
    /// that returns a default result.
    pub fn trigger_compression(
        &self,
    ) -> Result<crate::memory::context::CompressionResult, AetherFfiError> {
        // V2 compression is not yet fully implemented
        // Return a default result indicating no compression occurred
        Ok(crate::memory::context::CompressionResult {
            memories_processed: 0,
            facts_extracted: 0,
            facts_invalidated: 0,
            duration_ms: 0,
        })
    }

    /// Search memories with optional app/window filter
    ///
    /// This method provides the same interface as V1's search_memories for
    /// backward compatibility with Settings UI.
    ///
    /// Returns recent memories filtered by app_bundle_id and window_title.
    pub fn search_memories(
        &self,
        app_bundle_id: Option<String>,
        window_title: Option<String>,
        limit: u32,
    ) -> Result<Vec<crate::core::types::MemoryEntryFFI>, AetherFfiError> {
        use crate::core::types::MemoryEntryFFI;
        use crate::memory::VectorDatabase;

        // Get memory config from full_config
        let config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());
        if !config.memory.enabled {
            return Err(AetherFfiError::Memory("Memory is disabled".to_string()));
        }

        // Get memory database path
        let db_path = crate::utils::paths::get_memory_db_path()
            .map_err(|e| AetherFfiError::Memory(format!("Failed to get memory path: {}", e)))?;
        drop(config); // Release lock before async

        // Use default values for empty filters
        let app_filter = app_bundle_id.unwrap_or_default();
        let window_filter = window_title.unwrap_or_default();

        // Open VectorDatabase (sync operation)
        let db = VectorDatabase::new(db_path)
            .map_err(|e| AetherFfiError::Memory(format!("Failed to open database: {}", e)))?;

        // Query memories using VectorDatabase (async operation)
        // Search with empty embedding returns recent memories filtered by context
        let result = self.runtime.block_on(async {
            db.search_memories(&app_filter, &window_filter, &[], limit)
                .await
                .map_err(|e| AetherFfiError::Memory(e.to_string()))
        })?;

        // Convert to FFI type
        Ok(result
            .into_iter()
            .map(|m| MemoryEntryFFI {
                id: m.id,
                app_bundle_id: m.context.app_bundle_id,
                window_title: m.context.window_title,
                user_input: m.user_input,
                ai_output: m.ai_output,
                timestamp: m.context.timestamp,
                similarity_score: m.similarity_score,
            })
            .collect())
    }
}
