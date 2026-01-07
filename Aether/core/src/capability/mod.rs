//! Capability module for AI-first intent detection and capability execution.
//!
//! This module provides:
//! - **CapabilityExecutor**: Executes capabilities in priority order (Memory → Search → MCP → Video)
//! - **CapabilityDeclaration**: Describes capabilities for AI understanding
//! - **CapabilityRequest/AiResponse**: Types for AI capability invocation requests
//! - **ResponseParser**: Parses AI responses to detect capability requests

pub mod declaration;
pub mod request;
pub mod response_parser;

// Re-exports for convenience
pub use declaration::{CapabilityDeclaration, CapabilityParameter, CapabilityRegistry};
pub use request::{AiResponse, CapabilityRequest, ClarificationInfo, ClarificationReason};
pub use response_parser::ResponseParser;

// ============================================================================
// Capability Executor
// ============================================================================

/// Capability Executor - Execute capabilities in priority order
///
/// This module orchestrates the execution of different capabilities (Memory, Search, MCP, Video)
/// in a fixed priority order, enriching the AgentPayload with context data.
use crate::config::{MemoryConfig, VideoConfig};
use crate::error::{AetherError, Result};
use crate::memory::{EmbeddingModel, MemoryRetrieval, VectorDatabase};
use crate::payload::{AgentPayload, Capability};
use crate::search::{SearchOptions, SearchRegistry};
use crate::utils::pii;
use crate::video::{extract_youtube_url, YouTubeExtractor};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Capability executor that enriches AgentPayload with context data
///
/// Executes capabilities in priority order: Memory → Search → MCP → Video
pub struct CapabilityExecutor {
    /// Optional memory database for vector retrieval
    memory_db: Option<Arc<VectorDatabase>>,
    /// Memory configuration
    memory_config: Option<Arc<MemoryConfig>>,
    /// Optional search registry for search capability
    search_registry: Option<Arc<SearchRegistry>>,
    /// Search options (timeout, max results, etc.)
    search_options: SearchOptions,
    /// Enable PII (Personally Identifiable Information) scrubbing
    pii_scrubbing_enabled: bool,
    /// Video transcript configuration
    video_config: Option<Arc<VideoConfig>>,

    // AI Memory Retrieval Configuration
    /// AI provider for memory selection (required for AI retrieval)
    ai_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    /// Exclusion set for current conversation (to avoid duplicate context)
    memory_exclusion_set: Vec<String>,
    /// Enable AI-based memory retrieval (vs embedding-based)
    use_ai_retrieval: bool,
    /// AI retrieval timeout in milliseconds
    ai_retrieval_timeout_ms: u64,
    /// Maximum candidates to send to AI for selection
    ai_retrieval_max_candidates: u32,
    /// Fallback count when AI fails
    ai_retrieval_fallback_count: u32,
}

impl CapabilityExecutor {
    /// Create a new capability executor
    ///
    /// # Arguments
    ///
    /// * `memory_db` - Optional memory database for Memory capability
    /// * `memory_config` - Optional memory configuration
    /// * `search_registry` - Optional search registry for Search capability
    /// * `search_options` - Search options (timeout, max results, etc.)
    /// * `pii_scrubbing_enabled` - Enable PII scrubbing for search queries
    /// * `video_config` - Optional video transcript configuration
    pub fn new(
        memory_db: Option<Arc<VectorDatabase>>,
        memory_config: Option<Arc<MemoryConfig>>,
        search_registry: Option<Arc<SearchRegistry>>,
        search_options: Option<SearchOptions>,
        pii_scrubbing_enabled: bool,
    ) -> Self {
        Self {
            memory_db,
            memory_config,
            search_registry,
            search_options: search_options.unwrap_or_default(),
            pii_scrubbing_enabled,
            video_config: None,
            // AI Memory Retrieval defaults
            ai_provider: None,
            memory_exclusion_set: Vec::new(),
            use_ai_retrieval: false,
            ai_retrieval_timeout_ms: 3000,
            ai_retrieval_max_candidates: 20,
            ai_retrieval_fallback_count: 3,
        }
    }

    /// Create a new capability executor with video config
    pub fn with_video_config(mut self, video_config: Option<Arc<VideoConfig>>) -> Self {
        self.video_config = video_config;
        self
    }

    /// Configure AI-based memory retrieval
    ///
    /// # Arguments
    ///
    /// * `provider` - AI provider for memory selection
    /// * `enabled` - Whether to use AI retrieval (vs embedding-based)
    /// * `timeout_ms` - Timeout for AI retrieval in milliseconds
    /// * `max_candidates` - Maximum candidates to send to AI
    /// * `fallback_count` - Number of memories to return on fallback
    pub fn with_ai_retrieval(
        mut self,
        provider: Option<Arc<dyn crate::providers::AiProvider>>,
        enabled: bool,
        timeout_ms: u64,
        max_candidates: u32,
        fallback_count: u32,
    ) -> Self {
        self.ai_provider = provider;
        self.use_ai_retrieval = enabled;
        self.ai_retrieval_timeout_ms = timeout_ms;
        self.ai_retrieval_max_candidates = max_candidates;
        self.ai_retrieval_fallback_count = fallback_count;
        self
    }

    /// Set exclusion set for memory retrieval (to avoid duplicate context)
    ///
    /// # Arguments
    ///
    /// * `exclusion_set` - Strings to exclude from memory retrieval
    pub fn with_memory_exclusion_set(mut self, exclusion_set: Vec<String>) -> Self {
        self.memory_exclusion_set = exclusion_set;
        self
    }

    /// Get embedding model directory
    fn get_embedding_model_dir() -> Result<PathBuf> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

        Ok(PathBuf::from(home_dir)
            .join(".config")
            .join("aether")
            .join("models")
            .join("all-MiniLM-L6-v2"))
    }

    /// Execute all capabilities in priority order
    ///
    /// Capabilities are sorted and executed in order: Memory → Search → MCP
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload to enrich
    ///
    /// # Returns
    ///
    /// The enriched payload with context data added
    pub async fn execute_all(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Sort capabilities by priority (Memory=0, Search=1, MCP=2)
        let capabilities = Capability::sort_by_priority(payload.config.capabilities.clone());

        info!(
            capabilities = ?capabilities,
            "Executing capabilities in priority order"
        );

        // Execute each capability in order
        for capability in capabilities {
            payload = self.execute_capability(payload, capability).await?;
        }

        Ok(payload)
    }

    /// Execute a single capability
    ///
    /// Dispatches to the appropriate executor based on capability type
    async fn execute_capability(
        &self,
        mut payload: AgentPayload,
        capability: Capability,
    ) -> Result<AgentPayload> {
        match capability {
            Capability::Memory => {
                payload = self.execute_memory(payload).await?;
            }
            Capability::Search => {
                payload = self.execute_search(payload).await?;
            }
            Capability::Mcp => {
                warn!("MCP capability not implemented yet (reserved for future)");
                // Future: Call MCP client and populate payload.context.mcp_resources
            }
            Capability::Video => {
                payload = self.execute_video(payload).await?;
            }
        }

        Ok(payload)
    }

    /// Execute Memory capability
    ///
    /// Retrieves relevant memory snippets using either:
    /// - AI-based selection (when `use_ai_retrieval` is true and provider available)
    /// - Embedding-based vector similarity (fallback)
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload
    ///
    /// # Returns
    ///
    /// The payload with memory_snippets populated (if any found)
    async fn execute_memory(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        use crate::memory::ai_retrieval::AiMemoryRetriever;
        use crate::memory::ContextAnchor as MemoryContextAnchor;

        // Check if memory database and config are available
        let Some(db) = &self.memory_db else {
            warn!("Memory capability requested but no memory database configured");
            return Ok(payload);
        };

        let Some(config) = &self.memory_config else {
            warn!("Memory capability requested but no memory config available");
            return Ok(payload);
        };

        let query = &payload.user_input;
        let anchor = &payload.meta.context_anchor;

        info!(
            query_length = query.len(),
            app = %anchor.app_bundle_id,
            window = ?anchor.window_title,
            use_ai_retrieval = self.use_ai_retrieval,
            "Retrieving memory snippets"
        );

        // Convert payload::ContextAnchor to memory::ContextAnchor
        let memory_anchor = MemoryContextAnchor::with_timestamp(
            anchor.app_bundle_id.clone(),
            anchor.window_title.clone().unwrap_or_default(),
            payload.meta.timestamp,
        );

        // Initialize embedding model (needed for both paths)
        let model_dir = Self::get_embedding_model_dir()?;
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).map_err(|e| {
            AetherError::config(format!("Failed to initialize embedding model: {}", e))
        })?);

        // Choose retrieval strategy
        let memories = if self.use_ai_retrieval {
            // AI-based memory selection
            if let Some(ai_provider) = &self.ai_provider {
                info!("Using AI-based memory retrieval");

                // First, fetch candidate memories using embedding search
                let retrieval = MemoryRetrieval::new(
                    Arc::clone(db),
                    Arc::clone(&embedding_model),
                    Arc::clone(config),
                );

                // Get more candidates than needed for AI to select from
                let candidates = retrieval
                    .retrieve_memories_with_limit(&memory_anchor, query, self.ai_retrieval_max_candidates as usize)
                    .await
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "Failed to fetch memory candidates, returning empty");
                        Vec::new()
                    });

                if candidates.is_empty() {
                    debug!("No memory candidates found for AI selection");
                    Vec::new()
                } else {
                    // Use AI to select relevant memories
                    let retriever = AiMemoryRetriever::new(Arc::clone(ai_provider))
                        .with_timeout(std::time::Duration::from_millis(self.ai_retrieval_timeout_ms))
                        .with_max_candidates(self.ai_retrieval_max_candidates)
                        .with_fallback_count(self.ai_retrieval_fallback_count);

                    retriever
                        .retrieve(query, candidates, &self.memory_exclusion_set)
                        .await
                        .unwrap_or_else(|e| {
                            warn!(error = %e, "AI memory selection failed, returning empty");
                            Vec::new()
                        })
                }
            } else {
                warn!("AI retrieval enabled but no provider configured, falling back to embedding");
                // Fallback to embedding-based retrieval
                let retrieval = MemoryRetrieval::new(
                    Arc::clone(db),
                    Arc::clone(&embedding_model),
                    Arc::clone(config),
                );
                retrieval.retrieve_memories(&memory_anchor, query).await?
            }
        } else {
            // Traditional embedding-based vector similarity
            debug!("Using embedding-based memory retrieval");
            let retrieval = MemoryRetrieval::new(
                Arc::clone(db),
                Arc::clone(&embedding_model),
                Arc::clone(config),
            );
            retrieval.retrieve_memories(&memory_anchor, query).await?
        };

        if memories.is_empty() {
            info!("No relevant memories found");
        } else {
            info!(count = memories.len(), "Retrieved relevant memory snippets");
        }

        // Store in payload context
        payload.context.memory_snippets = if memories.is_empty() {
            None
        } else {
            Some(memories)
        };

        Ok(payload)
    }

    /// Extract search query from user input
    ///
    /// For MVP, this is a simple pass-through - the entire user input is used as the search query.
    /// In the future, this could implement more sophisticated query extraction logic.
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text
    ///
    /// # Returns
    ///
    /// The extracted search query, or None if the input is empty
    fn extract_search_query(input: &str) -> Option<String> {
        let query = input.trim();
        if query.is_empty() {
            None
        } else {
            Some(query.to_string())
        }
    }

    /// Execute Search capability
    ///
    /// Performs a web search using the configured search registry and populates
    /// the payload with search results.
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload
    ///
    /// # Returns
    ///
    /// The payload with search_results populated (if any found)
    async fn execute_search(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Check if search registry is available
        let Some(registry) = &self.search_registry else {
            warn!("Search capability requested but no search registry configured");
            return Ok(payload);
        };

        // Extract search query from user input
        let Some(mut query) = Self::extract_search_query(&payload.user_input) else {
            warn!("Search capability requested but user input is empty");
            return Ok(payload);
        };

        // Apply PII scrubbing if enabled
        if self.pii_scrubbing_enabled {
            let scrubbed = pii::scrub_pii(&query);
            if scrubbed != query {
                debug!("PII scrubbing applied to search query");
            }
            query = scrubbed;
        }

        info!(
            query_length = query.len(),
            max_results = self.search_options.max_results,
            timeout = self.search_options.timeout_seconds,
            pii_scrubbing = self.pii_scrubbing_enabled,
            "Executing search capability"
        );

        // Perform search with timeout
        let search_future = registry.search(&query, &self.search_options);
        let timeout_duration = std::time::Duration::from_secs(self.search_options.timeout_seconds);

        match tokio::time::timeout(timeout_duration, search_future).await {
            Ok(Ok(results)) => {
                if results.is_empty() {
                    info!("Search completed but no results found");
                    payload.context.search_results = None;
                } else {
                    info!(
                        count = results.len(),
                        provider = results.first().and_then(|r| r.provider.as_deref()),
                        "Search completed successfully"
                    );
                    payload.context.search_results = Some(results);
                }
            }
            Ok(Err(e)) => {
                warn!(
                    error = %e,
                    "Search failed, continuing without results"
                );
                payload.context.search_results = None;
            }
            Err(_) => {
                warn!(
                    timeout = self.search_options.timeout_seconds,
                    "Search timed out, continuing without results"
                );
                payload.context.search_results = None;
            }
        }

        Ok(payload)
    }

    /// Execute Video capability
    ///
    /// Extracts transcript from YouTube video if URL is found in user input.
    /// Falls back gracefully if extraction fails.
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload
    ///
    /// # Returns
    ///
    /// The payload with video_transcript populated (if URL found and extraction succeeds)
    async fn execute_video(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Use provided config or default
        let default_config = crate::config::VideoConfig::default();
        let config = self.video_config.as_ref().map(|c| c.as_ref()).unwrap_or(&default_config);

        if !config.enabled {
            debug!("Video capability disabled in config");
            return Ok(payload);
        }

        if !config.youtube_transcript {
            debug!("YouTube transcript extraction disabled in config");
            return Ok(payload);
        }

        // Extract YouTube URL from user input
        let Some(video_url) = extract_youtube_url(&payload.user_input) else {
            debug!("No YouTube URL found in user input");
            return Ok(payload);
        };

        info!(
            video_url = %video_url,
            "Found YouTube URL in user input, extracting transcript"
        );

        // Create extractor and fetch transcript
        let extractor = YouTubeExtractor::new(config.clone());

        match extractor.extract_transcript(&video_url).await {
            Ok(transcript) => {
                let formatted = transcript.format_for_context();
                info!(
                    video_id = %transcript.video_id,
                    title = %transcript.title,
                    segments = transcript.segments.len(),
                    truncated = transcript.was_truncated,
                    formatted_len = formatted.len(),
                    "Successfully extracted video transcript"
                );
                // Debug: print first 500 chars of formatted transcript
                debug!(
                    preview = %formatted.chars().take(500).collect::<String>(),
                    "Transcript preview"
                );
                payload.context.video_transcript = Some(transcript);
            }
            Err(e) => {
                warn!(
                    error = %e,
                    video_url = %video_url,
                    "Failed to extract video transcript, continuing without it"
                );
                // Don't fail the request - continue without transcript
            }
        }

        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};

    #[tokio::test]
    async fn test_execute_all_no_capabilities() {
        let executor = CapabilityExecutor::new(None, None, None, None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let result = executor.execute_all(payload).await.unwrap();
        assert!(result.context.memory_snippets.is_none());
    }

    #[tokio::test]
    async fn test_execute_all_with_search_warning() {
        let executor = CapabilityExecutor::new(None, None, None, None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        // Should complete without error, just log a warning
        let result = executor.execute_all(payload).await.unwrap();
        assert!(result.context.search_results.is_none());
    }

    #[tokio::test]
    async fn test_execute_memory_no_database() {
        let executor = CapabilityExecutor::new(None, None, None, None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Memory],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let result = executor.execute_all(payload).await.unwrap();
        assert!(result.context.memory_snippets.is_none());
    }

    #[tokio::test]
    async fn test_capability_priority_ordering() {
        let executor = CapabilityExecutor::new(None, None, None, None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        // Test that capabilities are executed in order: Memory, Search, MCP
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Mcp, Capability::Memory, Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        // Should execute without error in priority order
        let result = executor.execute_all(payload).await.unwrap();

        // Verify payload structure is intact
        assert_eq!(result.user_input, "Test");
        assert_eq!(result.config.provider_name, "openai");
    }

    #[tokio::test]
    async fn test_pii_scrubbing_enabled() {
        // Test that CapabilityExecutor can be created with PII scrubbing enabled
        let executor = CapabilityExecutor::new(None, None, None, None, true);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Contact me at test@example.com".to_string())
            .build()
            .unwrap();

        // Execute with PII scrubbing enabled (no search registry, so no actual search)
        let result = executor.execute_all(payload).await.unwrap();

        // Verify executor doesn't crash with PII scrubbing enabled
        assert!(result.context.search_results.is_none());
        assert!(executor.pii_scrubbing_enabled);
    }

    #[tokio::test]
    async fn test_pii_scrubbing_disabled() {
        // Test that CapabilityExecutor can be created with PII scrubbing disabled
        let executor = CapabilityExecutor::new(None, None, None, None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Contact me at test@example.com".to_string())
            .build()
            .unwrap();

        // Execute with PII scrubbing disabled
        let result = executor.execute_all(payload).await.unwrap();

        // Verify executor works correctly with PII scrubbing disabled
        assert!(result.context.search_results.is_none());
        assert!(!executor.pii_scrubbing_enabled);
    }

    // ===== End-to-End Integration Tests =====

    /// Mock search provider for testing
    struct MockSearchProvider {
        name: String,
        results: Vec<crate::search::SearchResult>,
    }

    impl MockSearchProvider {
        fn new(name: &str, result_count: usize) -> Self {
            let mut results = Vec::new();
            for i in 0..result_count {
                results.push(crate::search::SearchResult {
                    title: format!("Test Result {}", i + 1),
                    url: format!("https://test.com/{}", i + 1),
                    snippet: format!("Test snippet {}", i + 1),
                    full_content: None,
                    source_type: None,
                    provider: Some(name.to_string()),
                    published_date: None,
                    relevance_score: Some(0.9 - (i as f32 * 0.1)),
                });
            }
            Self {
                name: name.to_string(),
                results,
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::search::SearchProvider for MockSearchProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn is_available(&self) -> bool {
            true
        }

        async fn search(
            &self,
            _query: &str,
            _options: &crate::search::SearchOptions,
        ) -> Result<Vec<crate::search::SearchResult>> {
            Ok(self.results.clone())
        }
    }

    #[tokio::test]
    async fn test_e2e_search_capability_execution() {
        use crate::search::SearchRegistry;

        // Create search registry with mock provider
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockSearchProvider::new("mock", 3);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        // Create capability executor with search registry
        let executor = CapabilityExecutor::new(None, None, Some(Arc::new(registry)), None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        // Create payload with Search capability
        let payload = PayloadBuilder::new()
            .meta(Intent::BuiltinSearch, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("test query".to_string())
            .build()
            .unwrap();

        // Execute capabilities
        let result = executor.execute_all(payload).await.unwrap();

        // Verify search results were populated
        assert!(result.context.search_results.is_some());
        let search_results = result.context.search_results.unwrap();
        assert_eq!(search_results.len(), 3);
        assert_eq!(search_results[0].title, "Test Result 1");
        assert_eq!(search_results[0].provider, Some("mock".to_string()));
    }

    #[tokio::test]
    async fn test_e2e_multiple_capabilities_execution() {
        use crate::search::SearchRegistry;

        // Create search registry with mock provider
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockSearchProvider::new("mock", 2);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        // Create capability executor with search (no memory for simplicity)
        let executor = CapabilityExecutor::new(None, None, Some(Arc::new(registry)), None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        // Create payload with multiple capabilities
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Memory, Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("test query".to_string())
            .build()
            .unwrap();

        // Execute all capabilities
        let result = executor.execute_all(payload).await.unwrap();

        // Memory should be None (no database configured)
        assert!(result.context.memory_snippets.is_none());

        // Search results should be populated
        assert!(result.context.search_results.is_some());
        let search_results = result.context.search_results.unwrap();
        assert_eq!(search_results.len(), 2);
    }

    #[tokio::test]
    async fn test_e2e_search_with_pii_scrubbing() {
        use crate::search::SearchRegistry;

        // Create search registry with mock provider
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockSearchProvider::new("mock", 1);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        // Create capability executor with PII scrubbing enabled
        let executor = CapabilityExecutor::new(
            None,
            None,
            Some(Arc::new(registry)),
            None,
            true, // PII scrubbing enabled
        );

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        // Create payload with PII in user input
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("Contact me at test@example.com or call 555-1234".to_string())
            .build()
            .unwrap();

        // Execute search capability
        let result = executor.execute_all(payload).await.unwrap();

        // Search should still succeed (PII is scrubbed before searching)
        assert!(result.context.search_results.is_some());
        assert!(executor.pii_scrubbing_enabled);
    }

    #[tokio::test]
    async fn test_e2e_search_with_empty_query() {
        use crate::search::SearchRegistry;

        // Create search registry
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockSearchProvider::new("mock", 1);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        let executor = CapabilityExecutor::new(None, None, Some(Arc::new(registry)), None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        // Create payload with empty user input
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("   ".to_string()) // Empty after trimming
            .build()
            .unwrap();

        // Execute search capability
        let result = executor.execute_all(payload).await.unwrap();

        // Search results should be None for empty query
        assert!(result.context.search_results.is_none());
    }

    #[tokio::test]
    async fn test_e2e_capability_priority_ordering() {
        use crate::search::SearchRegistry;

        // Create search registry
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockSearchProvider::new("mock", 1);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        let executor = CapabilityExecutor::new(None, None, Some(Arc::new(registry)), None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        // Create payload with capabilities in reverse priority order
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Search, Capability::Mcp, Capability::Memory],
                ContextFormat::Markdown,
            )
            .user_input("test".to_string())
            .build()
            .unwrap();

        // Execute all capabilities (should reorder to Memory, Search, MCP)
        let result = executor.execute_all(payload).await.unwrap();

        // All capabilities should execute without error
        assert!(result.context.memory_snippets.is_none()); // No DB
        assert!(result.context.search_results.is_some()); // Has registry
        assert!(result.context.mcp_resources.is_none()); // Not implemented
    }
}
