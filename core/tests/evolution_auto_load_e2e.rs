//! End-to-end integration test for Evolution auto-load
//!
//! Tests the complete workflow:
//! 1. Evolution Pipeline detects solidification pattern
//! 2. Generates SolidificationSuggestion
//! 3. EvolutionAutoLoader generates SKILL.md
//! 4. Auto-loads skill into ToolServer
//! 5. Skill is immediately available for use

use alephcore::skill_evolution::types::{ExecutionStatus, SkillExecution, SolidificationConfig};
use alephcore::skill_evolution::{EvolutionTracker, SolidificationPipeline};
use alephcore::tools::markdown_skill::{EvolutionAutoLoader, MarkdownSkillGeneratorConfig};
use alephcore::tools::AlephToolServer;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn test_e2e_pattern_detection_to_auto_load() {
    // Setup
    let temp_dir = TempDir::new().unwrap();
    let tracker = Arc::new(EvolutionTracker::in_memory().unwrap());
    let tool_server = Arc::new(AlephToolServer::new());

    // Configure auto-loader with temp output directory
    let config = MarkdownSkillGeneratorConfig {
        output_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let auto_loader = Arc::new(EvolutionAutoLoader::with_config(tool_server.clone(), config));

    // Phase 1: Simulate usage pattern (10 successful executions)
    for i in 0..10 {
        let execution = SkillExecution {
            id: format!("exec-{}", i),
            skill_id: "git-quick-commit".to_string(),
            session_id: format!("session-{}", i % 3), // 3 distinct sessions
            invoked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            duration_ms: 100,
            status: ExecutionStatus::Success,
            satisfaction: Some(0.95),
            context: "git add . && git commit -m 'update'".to_string(),
            input_summary: "quick commit".to_string(),
            output_length: 50,
        };
        tracker.log_execution(&execution).unwrap();
    }

    // Phase 2: Run Evolution Pipeline with low thresholds
    let solidification_config = SolidificationConfig {
        min_success_count: 5,
        min_success_rate: 0.8,
        min_age_days: 0,
        max_idle_days: 100,
    };

    let pipeline = SolidificationPipeline::new(tracker)
        .with_config(solidification_config)
        .with_min_confidence(0.5);

    let result = pipeline.run().await.unwrap();

    // Verify suggestions were generated
    assert!(
        !result.suggestions.is_empty(),
        "Pipeline should generate suggestions"
    );
    assert_eq!(result.candidates_detected, 1);

    // Phase 3: Auto-load the first suggestion
    let suggestion = &result.suggestions[0];
    let loaded_count = auto_loader
        .load_from_suggestion(suggestion)
        .await
        .unwrap();

    assert_eq!(loaded_count, 1, "Should load exactly one tool");

    // Phase 4: Verify skill is available in ToolServer
    // The skill name is generated from suggested_name (not pattern_id)
    let expected_name = suggestion
        .suggested_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    assert!(
        tool_server.has_tool(&expected_name).await,
        "Tool '{}' should be registered in ToolServer",
        expected_name
    );

    // Phase 5: Verify skill definition
    let definition = tool_server.get_definition(&expected_name).await.unwrap();
    assert!(
        !definition.description.is_empty(),
        "Tool should have description"
    );
    // Verify parameters schema exists (should be an object)
    assert!(
        definition.parameters.is_object(),
        "Tool should have parameters schema"
    );

    // Phase 6: Verify skill has evolution metadata
    // The llm_context should contain the skill's markdown content
    assert!(
        definition.llm_context.is_some(),
        "Tool should have LLM context from SKILL.md"
    );

    // Phase 7: Verify tracking
    let generated_skills = auto_loader.get_generated_skills();
    assert_eq!(
        generated_skills.len(),
        1,
        "Should track one generated skill"
    );
}

#[tokio::test]
async fn test_batch_auto_load() {
    let temp_dir = TempDir::new().unwrap();
    let tracker = Arc::new(EvolutionTracker::in_memory().unwrap());
    let tool_server = Arc::new(AlephToolServer::new());

    let config = MarkdownSkillGeneratorConfig {
        output_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let auto_loader = Arc::new(EvolutionAutoLoader::with_config(tool_server.clone(), config));

    // Create multiple patterns
    for pattern_num in 0..3 {
        for i in 0..8 {
            let execution = SkillExecution {
                id: format!("exec-{}-{}", pattern_num, i),
                skill_id: format!("pattern-{}", pattern_num),
                session_id: format!("session-{}", i % 2),
                invoked_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
                duration_ms: 100,
                status: ExecutionStatus::Success,
                satisfaction: Some(0.85),
                context: format!("test context {}", pattern_num),
                input_summary: "test".to_string(),
                output_length: 50,
            };
            tracker.log_execution(&execution).unwrap();
        }
    }

    // Run pipeline
    let solidification_config = SolidificationConfig {
        min_success_count: 5,
        min_success_rate: 0.8,
        min_age_days: 0,
        max_idle_days: 100,
    };

    let pipeline = SolidificationPipeline::new(tracker)
        .with_config(solidification_config)
        .with_min_confidence(0.5);

    let result = pipeline.run().await.unwrap();
    assert_eq!(result.suggestions.len(), 3, "Should detect 3 patterns");

    // Batch auto-load
    let batch_result = auto_loader.load_batch(&result.suggestions).await.unwrap();

    assert_eq!(batch_result.total, 3);
    assert_eq!(batch_result.loaded, 3);
    assert_eq!(batch_result.failed, 0);
    assert!(batch_result.all_succeeded());
    assert_eq!(batch_result.success_rate(), 1.0);

    // Verify all tools are loaded (use actual generated names from suggestions)
    for suggestion in &result.suggestions {
        let skill_name = suggestion.suggested_name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");

        assert!(
            tool_server.has_tool(&skill_name).await,
            "Tool '{}' should be loaded",
            skill_name
        );
    }
}

#[tokio::test]
async fn test_auto_load_with_existing_tool() {
    let temp_dir = TempDir::new().unwrap();
    let tracker = Arc::new(EvolutionTracker::in_memory().unwrap());
    let tool_server = Arc::new(AlephToolServer::new());

    let config = MarkdownSkillGeneratorConfig {
        output_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let auto_loader = Arc::new(EvolutionAutoLoader::with_config(tool_server.clone(), config));

    // Log executions
    for i in 0..7 {
        let execution = SkillExecution {
            id: format!("exec-{}", i),
            skill_id: "test-skill".to_string(),
            session_id: format!("session-{}", i),
            invoked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            duration_ms: 100,
            status: ExecutionStatus::Success,
            satisfaction: Some(0.9),
            context: "test".to_string(),
            input_summary: "test".to_string(),
            output_length: 50,
        };
        tracker.log_execution(&execution).unwrap();
    }

    // Generate suggestion
    let solidification_config = SolidificationConfig {
        min_success_count: 5,
        min_success_rate: 0.7,
        min_age_days: 0,
        max_idle_days: 100,
    };

    let pipeline = SolidificationPipeline::new(tracker)
        .with_config(solidification_config)
        .with_min_confidence(0.5);

    let result = pipeline.run().await.unwrap();
    assert!(!result.suggestions.is_empty());

    let suggestion = &result.suggestions[0];

    // Load first time
    let count1 = auto_loader.load_from_suggestion(suggestion).await.unwrap();
    assert_eq!(count1, 1);

    // Load again (should replace existing tool using replace_tool API)
    let count2 = auto_loader.load_from_suggestion(suggestion).await.unwrap();
    assert_eq!(count2, 1);

    // Calculate expected skill name
    let skill_name = suggestion.suggested_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    // Verify tool still exists (was replaced, not duplicated)
    assert!(
        tool_server.has_tool(&skill_name).await,
        "Tool '{}' should exist after reload",
        skill_name
    );
}
