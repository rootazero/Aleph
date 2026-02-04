//! Integration tests for Markdown Skill Generator

use aethecore::skill_evolution::types::{SkillMetrics, SolidificationSuggestion};
use aethecore::tools::markdown_skill::{
    load_skills_from_dir, MarkdownSkillGenerator, MarkdownSkillGeneratorConfig,
};
use tempfile::TempDir;

#[test]
fn test_generate_skill_from_suggestion() {
    // Create a temporary directory for output
    let temp_dir = TempDir::new().unwrap();
    let output_dir = temp_dir.path().to_path_buf();

    // Create a generator with custom config
    let config = MarkdownSkillGeneratorConfig {
        output_dir: output_dir.clone(),
        ..Default::default()
    };
    let generator = MarkdownSkillGenerator::with_config(config);

    // Create a mock suggestion
    let mut metrics = SkillMetrics::new("test-pattern");
    metrics.total_executions = 5;
    metrics.successful_executions = 5;

    let suggestion = SolidificationSuggestion {
        pattern_id: "test-pattern-123".to_string(),
        suggested_name: "Git Quick Commit".to_string(),
        suggested_description: "Quickly commit changes with a message".to_string(),
        confidence: 0.92,
        metrics,
        sample_contexts: vec![
            "git add . && git commit -m 'fix bug'".to_string(),
            "git add README.md && git commit -m 'update docs'".to_string(),
        ],
        instructions_preview: "Use git to add and commit changes with a message".to_string(),
    };

    // Generate the skill
    let result = generator.generate(&suggestion);
    assert!(result.is_ok());

    let skill_path = result.unwrap();
    assert!(skill_path.exists());
    assert!(skill_path.ends_with("SKILL.md"));

    // Read and verify the content
    let content = std::fs::read_to_string(&skill_path).unwrap();

    // Check frontmatter
    assert!(content.contains("---"));
    assert!(content.contains("name: git-quick-commit"));
    assert!(content.contains("description:"));

    // Check metadata
    assert!(content.contains("metadata:"));
    assert!(content.contains("requires:"));
    assert!(content.contains("bins:"));
    assert!(content.contains("- \"git\"")); // Should detect git from instructions

    // Check Aether extensions
    assert!(content.contains("aether:"));
    assert!(content.contains("security:"));
    assert!(content.contains("evolution:"));
    assert!(content.contains("source: \"auto-generated\""));
    assert!(content.contains("confidence_score: 0.92"));
    assert!(content.contains("created_from_trace: \"test-pattern-123\""));

    // Check markdown content
    assert!(content.contains("# Git Quick Commit"));
    assert!(content.contains("## Description"));
    assert!(content.contains("## Examples"));
    assert!(content.contains("fix bug"));
    assert!(content.contains("## Metrics"));
}

#[tokio::test]
async fn test_generated_skill_can_be_loaded() {
    // Create a temporary directory
    let temp_dir = TempDir::new().unwrap();
    let output_dir = temp_dir.path().to_path_buf();

    // Generate a skill
    let config = MarkdownSkillGeneratorConfig {
        output_dir: output_dir.clone(),
        ..Default::default()
    };
    let generator = MarkdownSkillGenerator::with_config(config);

    let mut metrics = SkillMetrics::new("test-pattern");
    metrics.total_executions = 3;
    metrics.successful_executions = 3;

    let suggestion = SolidificationSuggestion {
        pattern_id: "echo-test".to_string(),
        suggested_name: "Echo Tool".to_string(),
        suggested_description: "Echo a message".to_string(),
        confidence: 0.85,
        metrics,
        sample_contexts: vec!["echo 'hello'".to_string()],
        instructions_preview: "Use echo command to print a message".to_string(),
    };

    let skill_path = generator.generate(&suggestion).unwrap();
    let skill_dir = skill_path.parent().unwrap();

    // Load the generated skill
    let tools = load_skills_from_dir(skill_dir.to_path_buf()).await;

    assert_eq!(tools.len(), 1);
    let tool = &tools[0];

    // Verify tool properties
    assert_eq!(tool.spec.name, "echo-tool");
    assert_eq!(
        tool.spec.description,
        "Echo a message"
    );

    // Verify evolution metadata
    let aether = tool.spec.metadata.aether.as_ref().unwrap();
    let evolution = aether.evolution.as_ref().unwrap();
    assert_eq!(evolution.source, "auto-generated");
    // Use approximate equality for float comparison
    assert!((evolution.confidence_score - 0.85).abs() < 0.001);
    assert_eq!(evolution.created_from_trace.as_ref().unwrap(), "echo-test");

    // Verify tool definition
    let definition = tool.definition();
    assert_eq!(definition.name, "echo-tool");
    assert!(definition.llm_context.is_some()); // Should have markdown content
}

#[test]
fn test_skill_name_conversion() {
    let temp_dir = TempDir::new().unwrap();
    let config = MarkdownSkillGeneratorConfig {
        output_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let generator = MarkdownSkillGenerator::with_config(config);

    let mut metrics = SkillMetrics::new("test");
    metrics.total_executions = 1;
    metrics.successful_executions = 1;

    // Test various name formats
    let test_cases = vec![
        ("Quick Fix", "quick-fix"),
        ("Docker Build & Push", "docker-build-push"),
        ("search_files", "search-files"),
        ("Git Commit --amend", "git-commit-amend"),
    ];

    for (input, expected) in test_cases {
        let suggestion = SolidificationSuggestion {
            pattern_id: "test".to_string(),
            suggested_name: input.to_string(),
            suggested_description: "Test".to_string(),
            confidence: 0.8,
            metrics: metrics.clone(),
            sample_contexts: vec![],
            instructions_preview: "test".to_string(),
        };

        let result = generator.generate(&suggestion).unwrap();
        let skill_name = result
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();

        assert_eq!(skill_name, expected, "Failed for input: {}", input);

        // Clean up
        std::fs::remove_dir_all(result.parent().unwrap()).ok();
    }
}
