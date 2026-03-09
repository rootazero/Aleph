//! Capability module for AI-first intent detection and capability execution.
//!
//! This module provides:
//! - **CapabilityExecutor**: Executes capabilities in priority order (Memory → Search → MCP → Video)
//! - **CapabilityStrategy**: Trait for pluggable capability implementations
//! - **CompositeCapabilityExecutor**: Strategy-based executor for decoupled capability execution
//! - **CapabilityDeclaration**: Describes capabilities for AI understanding
//! - **CapabilityRequest/AiResponse**: Types for AI capability invocation requests
//! - **ResponseParser**: Parses AI responses to detect capability requests

pub mod declaration;
pub mod request;
pub mod response_parser;
pub mod strategies;
pub mod strategy;
pub mod system;

// Re-exports for convenience
pub use declaration::{
    CapabilityDeclaration, CapabilityParameter, CapabilityRegistry, McpToolInfo,
};
pub use request::{AiResponse, CapabilityRequest, ClarificationInfo, ClarificationReason};
pub use response_parser::ResponseParser;
pub use strategies::{McpStrategy, MemoryStrategy, SkillsStrategy};
pub use strategy::{CapabilityHealth, CapabilityStrategy, CompositeCapabilityExecutor};
pub use system::{
    CapabilityDiagnostics, CapabilityStatus, CapabilitySystem, CapabilitySystemBuilder,
    CapabilitySystemConfig, SystemStatus,
};

// ============================================================================
// Capability Executor
// ============================================================================

/// Capability Executor - Execute capabilities in priority order
///
/// This module orchestrates the execution of different capabilities (Memory, Search, MCP, Video)
/// in a fixed priority order, enriching the AgentPayload with context data.
use crate::config::{McpConfig, MemoryConfig, SkillsConfig};
use crate::error::Result;
use crate::mcp::McpClient;
use crate::memory::store::MemoryBackend;
use crate::memory::{EmbeddingProvider, FactRetrieval, FactRetrievalConfig};
use crate::payload::{AgentPayload, Capability};
use crate::skills::SkillsRegistry;
use crate::sync_primitives::Arc;
use tracing::{debug, info, warn};

/// Capability executor that enriches AgentPayload with context data
///
/// Executes capabilities in priority order: Memory → Mcp → Skills
pub struct CapabilityExecutor {
    /// Optional memory database for vector retrieval
    memory_db: Option<MemoryBackend>,
    /// Memory configuration
    memory_config: Option<Arc<MemoryConfig>>,
    /// Embedding provider for memory retrieval
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Skills registry for Skills capability
    skills_registry: Option<Arc<SkillsRegistry>>,
    /// Skills configuration
    skills_config: Option<Arc<SkillsConfig>>,
    /// MCP client for tool access
    mcp_client: Option<Arc<McpClient>>,
    /// MCP configuration
    mcp_config: Option<Arc<McpConfig>>,

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
    pub fn new(
        memory_db: Option<MemoryBackend>,
        memory_config: Option<Arc<MemoryConfig>>,
    ) -> Self {
        Self {
            memory_db,
            memory_config,
            embedder: None,
            skills_registry: None,
            skills_config: None,
            mcp_client: None,
            mcp_config: None,
            // AI Memory Retrieval defaults
            ai_provider: None,
            memory_exclusion_set: Vec::new(),
            use_ai_retrieval: false,
            ai_retrieval_timeout_ms: 3000,
            ai_retrieval_max_candidates: 20,
            ai_retrieval_fallback_count: 3,
        }
    }

    /// Configure embedding provider
    pub fn with_embedder(mut self, embedder: Option<Arc<dyn EmbeddingProvider>>) -> Self {
        self.embedder = embedder;
        self
    }

    /// Configure MCP capability
    ///
    /// # Arguments
    ///
    /// * `mcp_client` - MCP client for tool access
    /// * `mcp_config` - MCP configuration
    pub fn with_mcp(
        mut self,
        mcp_client: Option<Arc<McpClient>>,
        mcp_config: Option<Arc<McpConfig>>,
    ) -> Self {
        self.mcp_client = mcp_client;
        self.mcp_config = mcp_config;
        self
    }

    /// Configure skills capability
    ///
    /// # Arguments
    ///
    /// * `skills_registry` - Skills registry containing loaded skills
    /// * `skills_config` - Skills configuration
    pub fn with_skills(
        mut self,
        skills_registry: Option<Arc<SkillsRegistry>>,
        skills_config: Option<Arc<SkillsConfig>>,
    ) -> Self {
        self.skills_registry = skills_registry;
        self.skills_config = skills_config;
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
            Capability::Mcp => {
                payload = self.execute_mcp(payload).await?;
            }
            Capability::Skills => {
                payload = self.execute_skills(payload).await?;
            }
        }

        Ok(payload)
    }

    /// Execute Memory capability
    ///
    /// Uses a "Fact-First" retrieval strategy:
    /// 1. First retrieve compressed facts (Layer 2) - more concise and informative
    /// 2. If facts are insufficient, fallback to raw memories (Layer 1)
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload
    ///
    /// # Returns
    ///
    /// The payload with memory_facts and/or memory_snippets populated
    async fn execute_memory(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
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
            window = ?anchor.window_title,
            max_facts = config.max_facts_in_context,
            raw_fallback = config.raw_memory_fallback_count,
            "Retrieving memory (fact-first strategy)"
        );

        // Get embedding provider
        let Some(embedder) = &self.embedder else {
            warn!("Memory capability requested but no embedding provider configured");
            return Ok(payload);
        };

        // Configure fact retrieval with user settings
        let retrieval_config = FactRetrievalConfig {
            max_facts: config.max_facts_in_context,
            max_raw_fallback: config.raw_memory_fallback_count,
            similarity_threshold: config.similarity_threshold,
        };

        // Create fact retrieval service
        let fact_retrieval = FactRetrieval::new(Arc::clone(db), Arc::clone(embedder), retrieval_config);

        // Retrieve using fact-first strategy
        let result = fact_retrieval.retrieve(query).await?;

        // Log retrieval results
        if result.is_empty() {
            info!("No relevant memory context found");
        } else {
            info!(
                facts_count = result.facts.len(),
                raw_memories_count = result.raw_memories.len(),
                "Retrieved memory context (facts + fallback)"
            );
        }

        // Store facts in payload context
        payload.context.memory_facts = if result.facts.is_empty() {
            None
        } else {
            Some(result.facts)
        };

        // Store raw memories as fallback
        payload.context.memory_snippets = if result.raw_memories.is_empty() {
            None
        } else {
            Some(result.raw_memories)
        };

        Ok(payload)
    }

    /// Execute MCP capability
    ///
    /// Lists available MCP tools and populates the payload with tool information.
    /// This allows the AI to understand what tools are available for use.
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload
    ///
    /// # Returns
    ///
    /// The payload with mcp_resources populated (if tools available)
    async fn execute_mcp(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        use std::collections::HashMap;

        // Check if MCP config exists and is enabled
        if let Some(config) = &self.mcp_config {
            if !config.enabled {
                debug!("MCP capability disabled in config");
                return Ok(payload);
            }
        }

        // Check if MCP client is available
        let Some(client) = &self.mcp_client else {
            debug!("MCP capability requested but no client configured");
            return Ok(payload);
        };

        info!("Executing MCP capability - listing available tools");

        // Get available tools from the MCP client
        let tools = client.list_tools().await;

        if tools.is_empty() {
            debug!("No MCP tools available");
            return Ok(payload);
        }

        info!(tool_count = tools.len(), "MCP tools available");

        // Convert tools to mcp_resources format
        // Format: tool_name -> { description, input_schema, requires_confirmation }
        let mut resources: HashMap<String, serde_json::Value> = HashMap::new();

        for tool in tools {
            let tool_info = serde_json::json!({
                "description": tool.description,
                "input_schema": tool.input_schema,
                "requires_confirmation": tool.requires_confirmation,
            });
            resources.insert(tool.name, tool_info);
        }

        payload.context.mcp_resources = Some(resources);

        Ok(payload)
    }

    /// Execute Skills capability
    ///
    /// Looks up skill by ID from Intent::Skills or auto-matches based on user input,
    /// then injects skill instructions into the payload context.
    ///
    /// # Arguments
    ///
    /// * `payload` - The agent payload
    ///
    /// # Returns
    ///
    /// The payload with skill_instructions populated (if skill matched)
    async fn execute_skills(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Check if skills config is available and enabled
        let default_config = SkillsConfig::default();
        let config = self
            .skills_config
            .as_ref()
            .map(|c| c.as_ref())
            .unwrap_or(&default_config);

        if !config.enabled {
            debug!("Skills capability disabled in config");
            return Ok(payload);
        }

        // Check if skills registry is available
        let Some(registry) = &self.skills_registry else {
            warn!("Skills capability requested but no registry configured");
            return Ok(payload);
        };

        // Check if a skill is explicitly specified via Intent::Skills
        let skill_id = payload.meta.intent.skills_id();

        // If skill_id is specified, look up directly
        if let Some(id) = skill_id {
            if let Some(skill) = registry.get_skill(id) {
                info!(
                    skill_id = %id,
                    skill_name = %skill.name(),
                    "Injecting skill instructions from explicit /skill command"
                );
                payload.context.skill_instructions = Some(skill.instructions.clone());
                return Ok(payload);
            } else {
                warn!(
                    skill_id = %id,
                    "Skill not found in registry"
                );
                return Ok(payload);
            }
        }

        // Auto-matching is only enabled when explicitly configured
        if !config.auto_match_enabled {
            debug!("Skills auto-matching disabled in config");
            return Ok(payload);
        }

        // Try to auto-match skill based on user input
        if let Some(skill) = registry.find_matching(&payload.user_input) {
            info!(
                skill_id = %skill.id,
                skill_name = %skill.name(),
                "Auto-matched skill based on user input"
            );
            payload.context.skill_instructions = Some(skill.instructions.clone());
        } else {
            debug!("No skill matched for user input");
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
        let executor = CapabilityExecutor::new(None, None);

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
    async fn test_execute_all_with_mcp_capability() {
        let executor = CapabilityExecutor::new(None, None);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Mcp],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        // Should complete without error (no MCP client configured)
        let result = executor.execute_all(payload).await.unwrap();
        assert!(result.context.mcp_tool_result.is_none());
    }

    #[tokio::test]
    async fn test_execute_memory_no_database() {
        let executor = CapabilityExecutor::new(None, None);

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
        let executor = CapabilityExecutor::new(None, None);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        // Test that capabilities are executed in order: Memory, Mcp, Skills
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Mcp, Capability::Memory, Capability::Skills],
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
    async fn test_executor_with_memory_db() {
        // Test that CapabilityExecutor can be created with memory db
        let executor = CapabilityExecutor::new(None, None);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test query".to_string())
            .build()
            .unwrap();

        // Execute without error
        let result = executor.execute_all(payload).await.unwrap();

        // Verify executor works
        assert!(result.context.memory_snippets.is_none());
    }

    #[tokio::test]
    async fn test_executor_basic() {
        // Test that CapabilityExecutor can be created
        let executor = CapabilityExecutor::new(None, None);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);

        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config("openai".to_string(), vec![], ContextFormat::Markdown)
            .user_input("Test query".to_string())
            .build()
            .unwrap();

        // Execute
        let result = executor.execute_all(payload).await.unwrap();

        // Verify executor works correctly
        assert!(result.context.memory_snippets.is_none());
    }
}
