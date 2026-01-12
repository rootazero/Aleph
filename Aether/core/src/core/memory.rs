//! Memory operations for AetherCore
//!
//! This module contains all memory-related methods:
//! - Memory storage and retrieval
//! - Memory cleanup and compression
//! - Memory statistics and management

use super::types::{AppMemoryInfo, CapturedContext, CompressionStats, MemoryEntryFFI};
use super::AetherCore;
use crate::config::MemoryConfig;
use crate::error::{AetherError, Result};
use crate::memory::context::CompressionResult;
use crate::memory::database::MemoryStats;
use std::sync::Arc;
use tracing::{debug, warn};

impl AetherCore {
    // ========================================================================
    // MEMORY MANAGEMENT METHODS (Phase 4)
    // ========================================================================

    /// Get memory database statistics
    pub fn get_memory_stats(&self) -> Result<MemoryStats> {
        let db = self.require_memory_db()?;
        self.runtime.block_on(db.get_stats())
    }

    /// Search memories by context
    pub fn search_memories(
        &self,
        app_bundle_id: String,
        window_title: Option<String>,
        limit: u32,
    ) -> Result<Vec<MemoryEntryFFI>> {
        let db = self.require_memory_db()?;

        // Use empty window title if not provided
        let window = window_title.as_deref().unwrap_or("");

        // For search without embedding, we'll return recent memories only
        // TODO: In Phase 4B, implement actual embedding-based search
        let memories =
            self.runtime
                .block_on(db.search_memories(&app_bundle_id, window, &[], limit))?;

        // Convert to FFI type
        Ok(memories
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

    /// Get list of unique app bundle IDs from memories
    pub fn get_memory_app_list(&self) -> Result<Vec<AppMemoryInfo>> {
        let db = self.require_memory_db()?;

        let apps = self.runtime.block_on(db.get_app_list())?;

        // Convert to FFI type
        Ok(apps
            .into_iter()
            .map(|(app_bundle_id, memory_count)| AppMemoryInfo {
                app_bundle_id,
                memory_count,
            })
            .collect())
    }

    /// Delete specific memory by ID
    pub fn delete_memory(&self, id: String) -> Result<()> {
        let db = self.require_memory_db()?;
        self.runtime.block_on(db.delete_memory(&id))
    }

    /// Clear memories (with optional filters)
    pub fn clear_memories(
        &self,
        app_bundle_id: Option<String>,
        window_title: Option<String>,
    ) -> Result<u64> {
        let db = self.require_memory_db()?;

        self.runtime
            .block_on(db.clear_memories(app_bundle_id.as_deref(), window_title.as_deref()))
    }

    /// Clear all compressed facts (Layer 2 data)
    ///
    /// This clears the memory_facts table which stores compressed/extracted
    /// facts from raw memories. The raw memories (Layer 1) are preserved.
    ///
    /// # Returns
    /// * `Result<u64>` - Number of deleted facts
    pub fn clear_facts(&self) -> Result<u64> {
        let db = self.require_memory_db()?;
        self.runtime.block_on(db.clear_facts())
    }

    /// Delete all memories associated with a specific topic ID
    ///
    /// This method is called when a multi-turn conversation topic is deleted
    /// to ensure all related memories are also removed.
    ///
    /// # Arguments
    /// * `topic_id` - The unique identifier of the topic
    ///
    /// # Returns
    /// * `Result<u64>` - Number of deleted memories
    pub fn delete_memories_by_topic_id(&self, topic_id: String) -> Result<u64> {
        let db = self.require_memory_db()?;
        self.runtime.block_on(db.delete_by_topic_id(&topic_id))
    }

    /// Get memory configuration
    pub fn get_memory_config(&self) -> MemoryConfig {
        let config = self.lock_config();
        config.memory.clone()
    }

    /// Update memory configuration
    pub fn update_memory_config(&self, new_config: MemoryConfig) -> Result<()> {
        let mut config = self.lock_config();
        let old_retention_days = config.memory.retention_days;
        config.memory = new_config.clone();

        // If retention policy changed and cleanup service exists, log the change
        // Note: The cleanup service will pick up the new config on next cleanup cycle
        if old_retention_days != new_config.retention_days {
            if let Some(_cleanup) = &self.cleanup_service {
                println!(
                    "[Memory] Retention policy updated: {} -> {} days",
                    old_retention_days, new_config.retention_days
                );
                // Note: We cannot update the cleanup service directly due to Arc
                // The service will be recreated when AetherCore is reinitialized
            }
        }

        // TODO: Persist config to file in Phase 4
        Ok(())
    }

    /// Manually trigger memory cleanup (for testing or immediate cleanup)
    ///
    /// This runs the cleanup operation immediately in the current thread,
    /// deleting memories older than the configured retention period.
    ///
    /// # Returns
    /// * `Result<u64>` - Number of deleted memories, or error
    #[deprecated(note = "Not used by Swift layer, may be removed in future")]
    pub fn cleanup_old_memories(&self) -> Result<u64> {
        let cleanup = self
            .cleanup_service
            .as_ref()
            .ok_or_else(|| AetherError::config("Cleanup service not initialized"))?;

        cleanup
            .cleanup_old_memories()
            .map_err(|e| AetherError::config(format!("Cleanup failed: {}", e)))
    }

    /// Manually trigger memory compression
    ///
    /// This executes the compression pipeline immediately:
    /// 1. Fetches uncompressed memories
    /// 2. Extracts facts using LLM
    /// 3. Detects and resolves conflicts
    /// 4. Stores facts in memory_facts table
    ///
    /// # Returns
    /// * `Result<CompressionResult>` - Compression statistics
    pub fn trigger_compression(&self) -> Result<CompressionResult> {
        let compression = self
            .compression_service
            .as_ref()
            .ok_or_else(|| AetherError::config("Compression service not initialized"))?;

        self.runtime
            .block_on(compression.compress())
            .map_err(|e| AetherError::other(format!("Compression failed: {}", e)))
    }

    /// Get compression statistics
    ///
    /// Returns statistics about the memory compression state:
    /// - Total raw memories count
    /// - Total facts count (valid and invalid)
    /// - Facts breakdown by type
    ///
    /// # Returns
    /// * `Result<CompressionStats>` - Compression statistics
    pub fn get_compression_stats(&self) -> Result<CompressionStats> {
        let db = self.require_memory_db()?;

        // Get memory stats
        let memory_stats = self.runtime.block_on(db.get_stats())?;

        // Get fact stats
        let fact_stats = self.runtime.block_on(db.get_fact_stats())?;

        Ok(CompressionStats {
            total_raw_memories: memory_stats.total_memories,
            total_facts: fact_stats.total_facts,
            valid_facts: fact_stats.valid_facts,
            facts_by_type: fact_stats.facts_by_type,
        })
    }

    /// Store interaction memory with current context
    ///
    /// This method is called after a successful AI interaction to store the
    /// user input and AI output along with the captured context.
    ///
    /// # Arguments
    /// * `user_input` - User's original input
    /// * `ai_output` - AI's response
    ///
    /// # Returns
    /// * `Result<String>` - Memory ID if stored successfully
    pub fn store_interaction_memory(
        &self,
        user_input: String,
        ai_output: String,
    ) -> Result<String> {
        use crate::memory::context::ContextAnchor;
        use crate::memory::embedding::EmbeddingModel;
        use crate::memory::ingestion::MemoryIngestion;

        // Check if memory is enabled
        let config = self.lock_config();
        if !config.memory.enabled {
            return Err(AetherError::config("Memory is disabled"));
        }

        // Get current context
        let current_context = self.current_context.lock().unwrap_or_else(|e| {
            warn!(
                "Mutex poisoned in current_context (AetherCore::store_interaction_memory), recovering"
            );
            e.into_inner()
        });
        let captured_context = current_context
            .as_ref()
            .ok_or_else(|| AetherError::config("No context captured"))?;

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
        let db = self.require_memory_db()?;

        // Get embedding model directory
        let model_dir = Self::get_embedding_model_dir().map_err(|e| {
            AetherError::config(format!("Failed to get embedding model directory: {}", e))
        })?;

        // Create embedding model (lazy load)
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

        // Create ingestion service
        let ingestion = MemoryIngestion::new(
            Arc::clone(db),
            embedding_model,
            Arc::new(config.memory.clone()),
        );

        // Store memory asynchronously
        let result =
            self.runtime
                .block_on(ingestion.store_memory(context_anchor, &user_input, &ai_output));

        result
    }

    /// Retrieve memories and augment prompt with context
    ///
    /// This is the main entry point for integrating memory into the AI request pipeline.
    /// It performs the following steps:
    /// 1. Check if memory is enabled
    /// 2. Get current context (app + window)
    /// 3. Retrieve relevant memories based on user query
    /// 4. Augment base prompt with retrieved memories
    /// 5. Return augmented prompt ready for LLM
    ///
    /// # Arguments
    /// * `base_prompt` - Base system prompt (e.g., "You are a helpful assistant")
    /// * `user_input` - Current user input/query
    ///
    /// # Returns
    /// * `Result<String>` - Augmented prompt with memory context, or base prompt if memory disabled
    ///
    /// # Performance
    /// - Includes timing logs for monitoring memory operation overhead
    /// - Target: <150ms total (embedding + search + formatting)
    #[deprecated(note = "Not used by Swift layer, may be removed in future")]
    pub fn retrieve_and_augment_prompt(
        &self,
        base_prompt: String,
        user_input: String,
    ) -> Result<String> {
        use crate::memory::augmentation::PromptAugmenter;
        use crate::memory::context::ContextAnchor;
        use crate::memory::embedding::EmbeddingModel;
        use crate::memory::retrieval::MemoryRetrieval;
        use std::time::Instant;

        let start_time = Instant::now();

        // Check if memory is enabled
        let config = self.lock_config();
        if !config.memory.enabled {
            println!("[Memory] Disabled - using base prompt");
            return Ok(format!("{}\n\nUser: {}", base_prompt, user_input));
        }

        // Get current context
        let current_context = self.current_context.lock().unwrap_or_else(|e| {
            warn!("Mutex poisoned in current_context (retrieve_and_augment_prompt), recovering");
            e.into_inner()
        });
        let captured_context = match current_context.as_ref() {
            Some(ctx) => ctx,
            None => {
                println!("[Memory] Warning: No context captured, skipping memory retrieval");
                return Ok(format!("{}\n\nUser: {}", base_prompt, user_input));
            }
        };

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
        let db = match self.memory_db.as_ref() {
            Some(db) => db,
            None => {
                println!("[Memory] Warning: Database not initialized");
                return Ok(format!("{}\n\nUser: {}", base_prompt, user_input));
            }
        };

        // Get embedding model
        let model_dir = Self::get_embedding_model_dir()?;
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

        let init_time = start_time.elapsed();
        println!("[Memory] Initialization time: {:?}", init_time);

        // Create retrieval service
        let retrieval = MemoryRetrieval::new(
            Arc::clone(db),
            Arc::clone(&embedding_model),
            Arc::new(config.memory.clone()),
        );

        // Retrieve memories
        let retrieval_start = Instant::now();
        let memories = self
            .runtime
            .block_on(retrieval.retrieve_memories(&context_anchor, &user_input))?;
        let retrieval_time = retrieval_start.elapsed();

        println!(
            "[Memory] Retrieved {} memories in {:?} (app: {}, window: {})",
            memories.len(),
            retrieval_time,
            context_anchor.app_bundle_id,
            context_anchor.window_title
        );

        // Augment prompt
        let augmentation_start = Instant::now();
        let augmenter = PromptAugmenter::with_config(
            config.memory.max_context_items as usize,
            false, // Don't show similarity scores in production
        );
        let augmented_prompt = augmenter.augment_prompt(&base_prompt, &memories, &user_input);
        let augmentation_time = augmentation_start.elapsed();

        let total_time = start_time.elapsed();
        println!(
            "[Memory] Augmentation time: {:?}, Total time: {:?}",
            augmentation_time, total_time
        );

        Ok(augmented_prompt)
    }

    /// Retrieve memories and augment ONLY the user input (no system prompt)
    ///
    /// # DEPRECATED
    /// This method is deprecated in favor of the CapabilityExecutor system.
    /// Memory retrieval is now handled by `CapabilityExecutor::execute_memory()` which:
    /// - Supports AI-based retrieval via `AiMemoryRetriever`
    /// - Uses exclusion sets to avoid duplicate context
    /// - Is properly integrated into the build_enriched_payload pipeline
    ///
    /// Use `build_enriched_payload()` with Memory capability instead.
    ///
    /// # Arguments
    /// * `user_input` - Current user input/query
    ///
    /// # Returns
    /// * `Result<String>` - User input with optional memory context
    #[allow(dead_code)]
    #[deprecated(
        since = "0.2.0",
        note = "Use CapabilityExecutor with Memory capability via build_enriched_payload()"
    )]
    pub fn retrieve_and_augment_user_input(&self, user_input: String) -> Result<String> {
        use crate::memory::augmentation::PromptAugmenter;
        use crate::memory::context::ContextAnchor;
        use crate::memory::embedding::EmbeddingModel;
        use crate::memory::retrieval::MemoryRetrieval;
        use std::time::Instant;

        let start_time = Instant::now();

        // Check if memory is enabled
        let config = self.lock_config();
        if !config.memory.enabled {
            debug!("[Memory] Disabled - returning original user input");
            return Ok(user_input);
        }

        // Check if AI retrieval is enabled
        let use_ai_retrieval = config.memory.ai_retrieval_enabled;

        // Get current context
        let current_context = self.current_context.lock().unwrap_or_else(|e| {
            warn!(
                "Mutex poisoned in current_context (retrieve_and_augment_user_input), recovering"
            );
            e.into_inner()
        });
        let captured_context = match current_context.as_ref() {
            Some(ctx) => ctx.clone(),
            None => {
                debug!("[Memory] No context captured, returning original user input");
                return Ok(user_input);
            }
        };
        drop(current_context); // Release lock before async operations

        // Get memory database
        let db = match self.memory_db.as_ref() {
            Some(db) => db,
            None => {
                debug!("[Memory] Database not initialized, returning original user input");
                return Ok(user_input);
            }
        };

        // Retrieve memories based on configured method
        let memories = if use_ai_retrieval {
            // AI-based retrieval: use AI to select relevant memories
            debug!("[Memory] Using AI-based retrieval");
            let exclusion_set = self.build_memory_exclusion_set();
            self.runtime.block_on(self.retrieve_memories_with_ai(
                &user_input,
                &captured_context,
                &exclusion_set,
            ))?
        } else {
            // Embedding-based retrieval: use vector similarity search
            debug!("[Memory] Using embedding-based retrieval");

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

            // Get embedding model
            let model_dir = Self::get_embedding_model_dir()?;
            let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
                AetherError::config(format!("Failed to initialize embedding model: {}", e))
            })?);

            // Create retrieval service
            let retrieval = MemoryRetrieval::new(
                Arc::clone(db),
                Arc::clone(&embedding_model),
                Arc::new(config.memory.clone()),
            );

            // Retrieve memories using embedding search
            self.runtime
                .block_on(retrieval.retrieve_memories(&context_anchor, &user_input))?
        };

        debug!(
            "[Memory] Retrieved {} memories for user input augmentation",
            memories.len()
        );

        // Augment user input (without system prompt)
        let augmenter =
            PromptAugmenter::with_config(config.memory.max_context_items as usize, false);
        let augmented_input = augmenter.augment_user_input(&memories, &user_input);

        let total_time = start_time.elapsed();
        debug!(
            "[Memory] User input augmentation completed in {:?}",
            total_time
        );

        Ok(augmented_input)
    }

    // ========================================================================
    // AI-based Memory Retrieval + Parallel Execution
    // ========================================================================

    /// Build exclusion set from current conversation session.
    ///
    /// Returns user inputs from all turns in the active session to prevent
    /// memory retrieval from returning content that's already in conversation cache.
    pub(crate) fn build_memory_exclusion_set(&self) -> Vec<String> {
        let manager = self
            .conversation_manager
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if let Some(session) = manager.active_session() {
            session
                .turns
                .iter()
                .map(|t| t.user_input.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Retrieve memories using AI-based selection.
    ///
    /// This replaces embedding-based vector similarity with AI evaluation.
    /// The AI is given recent memories and selects which are relevant.
    pub(crate) async fn retrieve_memories_with_ai(
        &self,
        user_input: &str,
        context: &CapturedContext,
        exclusion_set: &[String],
    ) -> Result<Vec<crate::memory::MemoryEntry>> {
        use crate::memory::{AiMemoryRetriever, MemoryEntry};
        use std::time::Duration;

        // Check if memory and AI retrieval are enabled
        // Use a block scope to ensure the MutexGuard is dropped before any await
        let (timeout_ms, max_candidates, fallback_count) = {
            let config = self.lock_config();
            if !config.memory.enabled || !config.memory.ai_retrieval_enabled {
                debug!("[Memory] AI retrieval disabled");
                return Ok(Vec::new());
            }
            (
                config.memory.ai_retrieval_timeout_ms,
                config.memory.ai_retrieval_max_candidates,
                config.memory.ai_retrieval_fallback_count,
            )
        };

        // Get memory database
        let db = match self.memory_db.as_ref() {
            Some(db) => db,
            None => {
                debug!("[Memory] Database not initialized");
                return Ok(Vec::new());
            }
        };

        // Get recent memories from database (without embedding search)
        let candidates: Vec<MemoryEntry> = db
            .get_recent_memories(
                &context.app_bundle_id,
                context.window_title.as_deref().unwrap_or(""),
                max_candidates,
                exclusion_set,
            )
            .await?;

        if candidates.is_empty() {
            debug!("[Memory] No candidate memories found");
            return Ok(Vec::new());
        }

        debug!(
            "[Memory] Found {} candidate memories for AI selection",
            candidates.len()
        );

        // Get default provider for AI memory selection
        let provider = match self.get_default_provider_instance() {
            Some(p) => p,
            None => {
                warn!("[Memory] No AI provider available for memory selection");
                // Fallback to most recent memories
                return Ok(candidates.into_iter().take(fallback_count as usize).collect());
            }
        };

        // Create AI memory retriever
        let retriever = AiMemoryRetriever::new(provider)
            .with_timeout(Duration::from_millis(timeout_ms))
            .with_max_candidates(max_candidates)
            .with_fallback_count(fallback_count);

        // Retrieve using AI selection
        retriever
            .retrieve(user_input, candidates, exclusion_set)
            .await
    }

    /// Record a conversation turn for compression scheduling
    ///
    /// This increments the pending turns counter in the compression scheduler.
    /// When the counter reaches the threshold (default: 20), automatic compression
    /// will be triggered immediately instead of waiting for the next hourly check.
    pub(crate) fn record_conversation_turn(&self) {
        if let Some(ref compression) = self.compression_service {
            compression.record_activity();
            // Use record_turn_and_check to trigger immediate compression when threshold reached
            compression.record_turn_and_check();
            tracing::trace!("Recorded conversation turn for compression scheduling");
        }
    }
}
