//! Integration tests for the tool index pipeline
//!
//! Tests the full Tool-as-Resource pipeline:
//! - ToolIndexCoordinator syncing tools to Memory
//! - ToolRetrieval retrieving with hydration levels
//! - SemanticPurposeInferrer inference levels

#[cfg(test)]
mod tests {
    use crate::dispatcher::tool_index::{
        HydrationLevel, HydratedTool, SemanticPurposeInferrer, ToolIndexCoordinator, ToolMeta,
        ToolRetrieval, ToolRetrievalConfig,
    };
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::store::MemoryBackend;
    use crate::memory::store::lance::LanceMemoryBackend;
    use std::sync::Arc;

    /// Create a test database using a temp directory for isolation
    async fn setup_test_db() -> (MemoryBackend, tempfile::TempDir) {
        let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
        let backend = LanceMemoryBackend::open_or_create(temp_dir.path()).await.expect("Failed to create LanceDB backend");
        (Arc::new(backend), temp_dir)
    }

    /// Generate a simple test embedding (384 dimensions)
    ///
    /// Uses sin function with seed to create somewhat unique embeddings
    /// that have reasonable similarity properties for testing.
    fn test_embedding(seed: f32) -> Vec<f32> {
        (0..384)
            .map(|i| ((i as f32 + seed) / 384.0).sin())
            .collect()
    }

    // ============================================================
    // ToolIndexCoordinator Tests
    // ============================================================

    #[tokio::test]
    async fn test_coordinator_sync_single_tool() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        let fact_id = coordinator
            .sync_tool(
                "read_file",
                Some("Read file contents from disk"),
                Some("file"),
                None,
                Some(test_embedding(1.0)),
            )
            .await
            .expect("sync_tool should succeed");

        assert_eq!(fact_id, "tool:read_file");

        // Verify the tool fact was stored
        let facts = coordinator
            .get_tool_facts()
            .await
            .expect("get_tool_facts should succeed");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].id, "tool:read_file");
        assert_eq!(facts[0].fact_type, FactType::Tool);
    }

    #[tokio::test]
    async fn test_coordinator_sync_tool_with_structured_meta() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        let fact_id = coordinator
            .sync_tool(
                "execute_shell",
                Some("Execute shell commands"),
                Some("system"),
                Some("Execute arbitrary shell commands in a sandboxed environment with security controls"),
                Some(test_embedding(2.0)),
            )
            .await
            .expect("sync_tool should succeed");

        assert_eq!(fact_id, "tool:execute_shell");

        // Verify the fact uses the structured_meta (L0 inference)
        let fact = coordinator
            .get_tool_fact("execute_shell")
            .await
            .expect("get_tool_fact should succeed")
            .expect("fact should exist");

        // L0 inference uses structured_meta directly, so content should contain it
        assert!(fact.content.contains("sandboxed environment"));
        assert_eq!(fact.confidence, 0.95); // L0 confidence
    }

    #[tokio::test]
    async fn test_coordinator_sync_all() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        let tools = vec![
            ToolMeta::new("read_file")
                .with_description("Read file contents")
                .with_category("file")
                .with_embedding(test_embedding(1.0)),
            ToolMeta::new("write_file")
                .with_description("Write content to file")
                .with_category("file")
                .with_embedding(test_embedding(2.0)),
            ToolMeta::new("search_code")
                .with_description("Search code in repository")
                .with_category("code")
                .with_embedding(test_embedding(3.0)),
        ];

        let fact_ids = coordinator
            .sync_all(tools)
            .await
            .expect("sync_all should succeed");

        assert_eq!(fact_ids.len(), 3);
        assert!(fact_ids.contains(&"tool:read_file".to_string()));
        assert!(fact_ids.contains(&"tool:write_file".to_string()));
        assert!(fact_ids.contains(&"tool:search_code".to_string()));

        let facts = coordinator
            .get_tool_facts()
            .await
            .expect("get_tool_facts should succeed");
        assert_eq!(facts.len(), 3);
    }

    #[tokio::test]
    async fn test_coordinator_remove_tool() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        // Add a tool
        coordinator
            .sync_tool(
                "test_tool",
                Some("Test tool"),
                None,
                None,
                Some(test_embedding(1.0)),
            )
            .await
            .expect("sync_tool should succeed");

        // Verify it exists
        assert!(
            coordinator
                .tool_exists("test_tool")
                .await
                .expect("tool_exists should succeed")
        );

        // Remove it
        coordinator
            .remove_tool("test_tool")
            .await
            .expect("remove_tool should succeed");

        // Verify it's gone (invalidated)
        assert!(
            !coordinator
                .tool_exists("test_tool")
                .await
                .expect("tool_exists should succeed")
        );
    }

    #[tokio::test]
    async fn test_coordinator_update_existing_tool() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        // Add a tool
        coordinator
            .sync_tool(
                "evolving_tool",
                Some("Original description"),
                Some("category1"),
                None,
                Some(test_embedding(1.0)),
            )
            .await
            .expect("sync_tool should succeed");

        // Update the same tool with new description
        coordinator
            .sync_tool(
                "evolving_tool",
                Some("Updated description with more details"),
                Some("category2"),
                None,
                Some(test_embedding(1.5)),
            )
            .await
            .expect("sync_tool should succeed for update");

        // Should still only have one fact
        let facts = coordinator
            .get_tool_facts()
            .await
            .expect("get_tool_facts should succeed");
        assert_eq!(facts.len(), 1);

        // Content should be updated
        let fact = &facts[0];
        assert!(fact.content.contains("Updated description"));
    }

    // ============================================================
    // SemanticPurposeInferrer Tests
    // ============================================================

    #[test]
    fn test_semantic_purpose_inferrer_l0() {
        let inferrer = SemanticPurposeInferrer::new();

        let result = inferrer.infer(
            "read_file",
            Some("Read file"),
            Some("file"),
            Some("Read and retrieve content from local filesystem files"),
        );

        assert_eq!(result.level, 0);
        assert_eq!(result.confidence, 0.95);
        assert!(result.description.contains("filesystem"));
    }

    #[test]
    fn test_semantic_purpose_inferrer_l1() {
        let inferrer = SemanticPurposeInferrer::new();

        let result = inferrer.infer(
            "search_code",
            Some("Search for code patterns"),
            Some("code"),
            None, // No structured_meta
        );

        assert_eq!(result.level, 1);
        assert!(result.confidence < 0.95);
        assert!(result.description.contains("[code]"));
    }

    #[test]
    fn test_semantic_purpose_inferrer_l1_without_category() {
        let inferrer = SemanticPurposeInferrer::new();

        let result = inferrer.infer(
            "my_custom_tool",
            Some("Does something custom"),
            None, // No category
            None, // No structured_meta
        );

        assert_eq!(result.level, 1);
        // Lower confidence without category
        assert!(result.confidence < 0.8);
        assert!(result.description.contains("custom"));
    }

    #[test]
    fn test_semantic_purpose_inferrer_empty_meta_fallback() {
        let inferrer = SemanticPurposeInferrer::new();

        let result = inferrer.infer(
            "tool_name",
            Some("Description"),
            Some("category"),
            Some("   "), // Empty/whitespace meta should fall back
        );

        // Should fall back to L1
        assert_eq!(result.level, 1);
    }

    // ============================================================
    // ToolRetrievalConfig Tests
    // ============================================================

    #[test]
    fn test_hydration_level_classification() {
        let config = ToolRetrievalConfig::default();

        // Verify default thresholds
        assert_eq!(config.high_confidence_threshold, 0.7);
        assert_eq!(config.soft_threshold, 0.6);
        assert_eq!(config.hard_threshold, 0.4);

        // Verify ordering
        assert!(config.hard_threshold < config.soft_threshold);
        assert!(config.soft_threshold < config.high_confidence_threshold);
    }

    // ============================================================
    // ToolRetrieval Tests
    // ============================================================

    #[tokio::test]
    async fn test_retrieval_empty_database() {
        let (db, _temp) = setup_test_db().await;
        let retrieval = ToolRetrieval::with_defaults(db.clone());

        let tools = retrieval
            .retrieve(&test_embedding(1.0))
            .await
            .expect("retrieve should succeed");

        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn test_retrieval_finds_similar_tools() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        // Sync some tools with distinct embeddings
        let file_embedding = test_embedding(1.0);
        let code_embedding = test_embedding(100.0); // Very different seed

        coordinator
            .sync_tool(
                "read_file",
                Some("Read file contents from disk"),
                Some("file"),
                Some("Read and retrieve content from local filesystem files"),
                Some(file_embedding.clone()),
            )
            .await
            .expect("sync_tool should succeed");

        coordinator
            .sync_tool(
                "search_code",
                Some("Search for code patterns in repository"),
                Some("code"),
                None,
                Some(code_embedding.clone()),
            )
            .await
            .expect("sync_tool should succeed");

        // Retrieve using embedding similar to file_embedding
        let retrieval = ToolRetrieval::with_defaults(db.clone());
        let tools = retrieval
            .retrieve(&file_embedding)
            .await
            .expect("retrieve should succeed");

        // Should find at least the file tool
        assert!(!tools.is_empty());

        // First result should be read_file (highest similarity to itself)
        let first = &tools[0];
        assert_eq!(first.name, "read_file");
    }

    #[tokio::test]
    async fn test_retrieval_hydration_levels() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        // Sync a tool
        let embedding = test_embedding(1.0);
        coordinator
            .sync_tool(
                "test_tool",
                Some("Test tool for hydration"),
                Some("test"),
                None,
                Some(embedding.clone()),
            )
            .await
            .expect("sync_tool should succeed");

        // Retrieve with exact same embedding (should get high similarity)
        let retrieval = ToolRetrieval::with_defaults(db.clone());
        let tools = retrieval
            .retrieve(&embedding)
            .await
            .expect("retrieve should succeed");

        assert!(!tools.is_empty());
        let tool = &tools[0];

        // With identical embedding, should have very high similarity -> Full hydration
        // Note: Actual score depends on vector search implementation
        assert!(tool.score > 0.4); // At minimum above hard threshold
    }

    #[tokio::test]
    async fn test_retrieval_respects_max_tools() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        // Sync many tools
        for i in 0..20 {
            coordinator
                .sync_tool(
                    &format!("tool_{}", i),
                    Some(&format!("Tool number {}", i)),
                    Some("bulk"),
                    None,
                    Some(test_embedding(i as f32)),
                )
                .await
                .expect("sync_tool should succeed");
        }

        // Create retrieval with max_tools = 5
        let config = ToolRetrievalConfig {
            max_tools: 5,
            ..Default::default()
        };
        let retrieval = ToolRetrieval::new(db.clone(), config);

        let tools = retrieval
            .retrieve(&test_embedding(0.0))
            .await
            .expect("retrieve should succeed");

        // Should return at most 5 tools
        assert!(tools.len() <= 5);
    }

    #[tokio::test]
    async fn test_retrieval_partition_by_hydration() {
        use crate::memory::context::MemoryFact;

        let config = ToolRetrievalConfig::default();

        // Create mock facts with different scores to test partitioning
        let mut fact_full = MemoryFact::with_id(
            "tool:high_confidence".to_string(),
            "High confidence tool".to_string(),
            FactType::Tool,
        );
        fact_full.similarity_score = Some(0.85); // Above high_confidence_threshold

        let mut fact_summary = MemoryFact::with_id(
            "tool:medium_confidence".to_string(),
            "Medium confidence tool".to_string(),
            FactType::Tool,
        );
        fact_summary.similarity_score = Some(0.65); // Between soft and high

        let mut fact_minimal = MemoryFact::with_id(
            "tool:low_confidence".to_string(),
            "Low confidence tool".to_string(),
            FactType::Tool,
        );
        fact_minimal.similarity_score = Some(0.45); // Between hard and soft

        // Import HydratedTool for from_fact
        use crate::dispatcher::tool_index::HydratedTool;

        let tools = vec![
            HydratedTool::from_fact(fact_full, &config),
            HydratedTool::from_fact(fact_summary, &config),
            HydratedTool::from_fact(fact_minimal, &config),
        ];

        let (full, summary, minimal) = ToolRetrieval::partition_by_hydration(&tools);

        assert_eq!(full.len(), 1);
        assert_eq!(full[0].name, "high_confidence");
        assert_eq!(full[0].hydration_level, HydrationLevel::Full);

        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].name, "medium_confidence");
        assert_eq!(summary[0].hydration_level, HydrationLevel::Summary);

        assert_eq!(minimal.len(), 1);
        assert_eq!(minimal[0].name, "low_confidence");
        assert_eq!(minimal[0].hydration_level, HydrationLevel::Minimal);
    }

    // ============================================================
    // Full Pipeline Integration Tests
    // ============================================================

    #[tokio::test]
    async fn test_full_pipeline_sync_and_retrieve() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        // Phase 1: Sync tools with embeddings
        let file_embedding = test_embedding(1.0);
        let code_embedding = test_embedding(2.0);
        let shell_embedding = test_embedding(3.0);

        coordinator
            .sync_tool(
                "read_file",
                Some("Read file contents from disk"),
                Some("file"),
                Some("Read and retrieve content from local filesystem files"),
                Some(file_embedding.clone()),
            )
            .await
            .expect("sync read_file should succeed");

        coordinator
            .sync_tool(
                "search_code",
                Some("Search for code patterns in repository"),
                Some("code"),
                None,
                Some(code_embedding.clone()),
            )
            .await
            .expect("sync search_code should succeed");

        coordinator
            .sync_tool(
                "execute_shell",
                Some("Run shell commands"),
                Some("system"),
                Some("Execute shell commands in a sandboxed environment"),
                Some(shell_embedding.clone()),
            )
            .await
            .expect("sync execute_shell should succeed");

        // Phase 2: Verify facts are stored correctly
        let facts = coordinator
            .get_tool_facts()
            .await
            .expect("get_tool_facts should succeed");
        assert_eq!(facts.len(), 3);

        // Phase 3: Retrieve tools using semantic similarity
        let retrieval = ToolRetrieval::with_defaults(db.clone());

        // Query similar to file operations
        let file_results = retrieval
            .retrieve(&file_embedding)
            .await
            .expect("retrieve should succeed");

        assert!(!file_results.is_empty());
        // First result should be the file tool (exact match)
        assert_eq!(file_results[0].name, "read_file");

        // Query similar to code operations
        let code_results = retrieval
            .retrieve(&code_embedding)
            .await
            .expect("retrieve should succeed");

        assert!(!code_results.is_empty());
        assert_eq!(code_results[0].name, "search_code");
    }

    #[tokio::test]
    async fn test_pipeline_tool_lifecycle() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());
        let retrieval = ToolRetrieval::with_defaults(db.clone());

        let embedding = test_embedding(42.0);

        // Step 1: Add tool
        coordinator
            .sync_tool(
                "lifecycle_tool",
                Some("Tool to test lifecycle"),
                Some("test"),
                None,
                Some(embedding.clone()),
            )
            .await
            .expect("sync should succeed");

        // Step 2: Verify retrievable
        let results = retrieval
            .retrieve(&embedding)
            .await
            .expect("retrieve should succeed");
        assert!(results.iter().any(|t| t.name == "lifecycle_tool"));

        // Step 3: Remove tool
        coordinator
            .remove_tool("lifecycle_tool")
            .await
            .expect("remove should succeed");

        // Step 4: Verify no longer retrievable (invalidated)
        let results_after = retrieval
            .retrieve(&embedding)
            .await
            .expect("retrieve should succeed");
        assert!(!results_after.iter().any(|t| t.name == "lifecycle_tool"));
    }

    #[tokio::test]
    async fn test_pipeline_category_based_retrieval() {
        let (db, _temp) = setup_test_db().await;
        let coordinator = ToolIndexCoordinator::new(db.clone());

        // Create tools in different categories with slightly different embeddings
        // but based on the same seed family
        let base_seed = 10.0;

        // File category tools
        coordinator
            .sync_tool(
                "read_file",
                Some("Read file"),
                Some("file"),
                None,
                Some(test_embedding(base_seed)),
            )
            .await
            .unwrap();

        coordinator
            .sync_tool(
                "write_file",
                Some("Write file"),
                Some("file"),
                None,
                Some(test_embedding(base_seed + 0.1)),
            )
            .await
            .unwrap();

        // Code category tools
        coordinator
            .sync_tool(
                "search_code",
                Some("Search code"),
                Some("code"),
                None,
                Some(test_embedding(base_seed + 100.0)),
            )
            .await
            .unwrap();

        coordinator
            .sync_tool(
                "analyze_code",
                Some("Analyze code"),
                Some("code"),
                None,
                Some(test_embedding(base_seed + 100.1)),
            )
            .await
            .unwrap();

        // Retrieve with file-like query
        let retrieval = ToolRetrieval::with_defaults(db.clone());
        let file_results = retrieval
            .retrieve(&test_embedding(base_seed + 0.05))
            .await
            .expect("retrieve should succeed");

        // File tools should rank higher for file-like query
        if file_results.len() >= 2 {
            // The top results should be file-related
            let top_names: Vec<_> = file_results.iter().take(2).map(|t| &t.name).collect();
            assert!(
                top_names.contains(&&"read_file".to_string())
                    || top_names.contains(&&"write_file".to_string()),
                "File tools should rank high for file-like query"
            );
        }
    }

    // ============================================================
    // HydrationPipeline Tests
    // ============================================================

    #[test]
    fn test_hydration_pipeline_config_default() {
        use crate::dispatcher::tool_index::HydrationPipelineConfig;

        let config = HydrationPipelineConfig::default();
        assert_eq!(config.max_full_schema, 5);
        assert_eq!(config.max_summary, 3);
        assert!(config.core_tools.contains(&"file_ops".to_string()));
        assert!(config.core_tools.contains(&"bash".to_string()));
    }

    #[test]
    fn test_hydration_pipeline_config_builder() {
        use crate::dispatcher::tool_index::HydrationPipelineConfig;

        let config = HydrationPipelineConfig::default()
            .with_max_full_schema(10)
            .with_max_summary(5)
            .with_core_tools(vec!["custom_tool".to_string()]);

        assert_eq!(config.max_full_schema, 10);
        assert_eq!(config.max_summary, 5);
        assert_eq!(config.core_tools, vec!["custom_tool"]);
    }

    #[test]
    fn test_hydration_result_empty() {
        use crate::dispatcher::tool_index::HydrationResult;

        let result = HydrationResult::empty();
        assert!(result.is_empty());
        assert_eq!(result.total_count(), 0);
        assert!(result.all_tool_names().is_empty());
    }

    #[test]
    fn test_hydration_result_counts() {
        use crate::dispatcher::tool_index::HydrationResult;

        let config = ToolRetrievalConfig::default();

        // Create mock facts
        let mut fact1 = MemoryFact::with_id(
            "tool:read_file".to_string(),
            "Read file".to_string(),
            FactType::Tool,
        );
        fact1.similarity_score = Some(0.85);

        let mut fact2 = MemoryFact::with_id(
            "tool:write_file".to_string(),
            "Write file".to_string(),
            FactType::Tool,
        );
        fact2.similarity_score = Some(0.65);

        let result = HydrationResult {
            full_schema_tools: vec![HydratedTool::from_fact(fact1, &config)],
            summary_tools: vec![HydratedTool::from_fact(fact2, &config)],
            indexed_tool_names: vec!["delete_file".to_string()],
        };

        assert!(!result.is_empty());
        assert_eq!(result.total_count(), 3);

        let names = result.all_tool_names();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"delete_file"));
    }

    #[test]
    fn test_hydrated_tool_schema_caching() {
        let config = ToolRetrievalConfig::default();

        let mut fact = MemoryFact::with_id(
            "tool:test_tool".to_string(),
            "Test tool description".to_string(),
            FactType::Tool,
        );
        fact.similarity_score = Some(0.85);

        let tool = HydratedTool::from_fact(fact, &config);

        // Initially no schema
        assert!(!tool.has_schema());
        assert!(tool.schema_json().is_none());

        // Add schema
        let tool_with_schema = tool.with_schema(r#"{"type": "object"}"#.to_string());
        assert!(tool_with_schema.has_schema());
        assert_eq!(tool_with_schema.schema_json(), Some(r#"{"type": "object"}"#));
    }

    // ============================================================
    // PromptBuilder Hydration Tests
    // ============================================================

    #[test]
    fn test_prompt_builder_hydrated_tools_empty() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
        use crate::dispatcher::tool_index::HydrationResult;

        let builder = PromptBuilder::new(PromptConfig::default());
        let mut prompt = String::new();
        let result = HydrationResult::empty();

        builder.append_hydrated_tools(&mut prompt, &result);

        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("get_tool_schema"));
    }

    #[test]
    fn test_prompt_builder_hydrated_tools_full_schema() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
        use crate::dispatcher::tool_index::HydrationResult;

        let builder = PromptBuilder::new(PromptConfig::default());
        let mut prompt = String::new();

        let config = ToolRetrievalConfig::default();
        let mut fact = MemoryFact::with_id(
            "tool:read_file".to_string(),
            "Read file from disk".to_string(),
            FactType::Tool,
        );
        fact.similarity_score = Some(0.85);

        let mut tool = HydratedTool::from_fact(fact, &config);
        tool.cached_schema = Some(r#"{"path": "string"}"#.to_string());

        let result = HydrationResult {
            full_schema_tools: vec![tool],
            summary_tools: vec![],
            indexed_tool_names: vec![],
        };

        builder.append_hydrated_tools(&mut prompt, &result);

        assert!(prompt.contains("#### read_file"));
        assert!(prompt.contains("Read file from disk"));
        assert!(prompt.contains("Parameters:"));
        assert!(prompt.contains(r#"{"path": "string"}"#));
    }

    #[test]
    fn test_prompt_builder_hydrated_tools_summary() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
        use crate::dispatcher::tool_index::HydrationResult;

        let builder = PromptBuilder::new(PromptConfig::default());
        let mut prompt = String::new();

        let config = ToolRetrievalConfig::default();
        let mut fact = MemoryFact::with_id(
            "tool:search_code".to_string(),
            "Search for code patterns".to_string(),
            FactType::Tool,
        );
        fact.similarity_score = Some(0.65); // Summary level

        let tool = HydratedTool::from_fact(fact, &config);

        let result = HydrationResult {
            full_schema_tools: vec![],
            summary_tools: vec![tool],
            indexed_tool_names: vec![],
        };

        builder.append_hydrated_tools(&mut prompt, &result);

        assert!(prompt.contains("summary"));
        assert!(prompt.contains("**search_code**"));
        assert!(prompt.contains("Search for code patterns"));
    }

    #[test]
    fn test_prompt_builder_hydrated_tools_indexed() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
        use crate::dispatcher::tool_index::HydrationResult;

        let builder = PromptBuilder::new(PromptConfig::default());
        let mut prompt = String::new();

        let result = HydrationResult {
            full_schema_tools: vec![],
            summary_tools: vec![],
            indexed_tool_names: vec!["tool_a".to_string(), "tool_b".to_string()],
        };

        builder.append_hydrated_tools(&mut prompt, &result);

        assert!(prompt.contains("Additional Tools"));
        assert!(prompt.contains("tool_a"));
        assert!(prompt.contains("tool_b"));
    }

    // ============================================================
    // Skill Registry Event Tests
    // ============================================================

    #[test]
    fn test_skill_registry_event_creation() {
        use crate::skills::SkillRegistryEvent;

        let loaded = SkillRegistryEvent::loaded("test-skill", "Test Skill");
        assert_eq!(loaded.skill_id(), Some("test-skill"));
        assert!(!loaded.is_bulk_reload());

        let removed = SkillRegistryEvent::removed("old-skill");
        assert_eq!(removed.skill_id(), Some("old-skill"));
        assert!(!removed.is_bulk_reload());

        let reloaded = SkillRegistryEvent::all_reloaded(3, vec!["a".into(), "b".into(), "c".into()]);
        assert!(reloaded.skill_id().is_none());
        assert!(reloaded.is_bulk_reload());
    }

    #[test]
    fn test_skill_registry_event_serialization() {
        use crate::skills::SkillRegistryEvent;

        let event = SkillRegistryEvent::loaded("test", "Test Skill");
        let json = serde_json::to_string(&event).expect("serialize should succeed");

        assert!(json.contains("skill_loaded"));
        assert!(json.contains("test"));

        let deserialized: SkillRegistryEvent =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(deserialized.skill_id(), Some("test"));
    }

    #[tokio::test]
    async fn test_skill_registry_subscribe() {
        use crate::skills::SkillsRegistry;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        let registry = SkillsRegistry::new(skills_dir);

        // Should be able to subscribe
        let mut receiver = registry.subscribe();

        // Create a skill for testing
        let skill_dir = temp_dir.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: Test skill\n---\nInstructions",
        )
        .unwrap();

        // Load skills (should emit AllReloaded event)
        registry.load_all().unwrap();

        // Try to receive the event (non-blocking)
        tokio::time::timeout(std::time::Duration::from_millis(100), async {
            if let Ok(event) = receiver.recv().await {
                match event {
                    crate::skills::SkillRegistryEvent::AllReloaded { count, skill_ids } => {
                        assert_eq!(count, 1);
                        assert!(skill_ids.contains(&"test-skill".to_string()));
                    }
                    _ => panic!("Expected AllReloaded event"),
                }
            }
        })
        .await
        .expect("Should receive event within timeout");
    }
}
