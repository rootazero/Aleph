//! Integration tests for Markdown Skill System

use aethecore::tools::markdown_skill::{load_skills_from_dir, SkillLoader};
use aethecore::tools::AetherToolServer;
use std::path::PathBuf;

#[tokio::test]
async fn test_load_openclaw_compatible_skill() {
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/markdown_skills/echo-basic");

    let tools = load_skills_from_dir(fixtures_dir).await;

    assert_eq!(tools.len(), 1);
    let tool = &tools[0];
    assert_eq!(tool.spec.name, "echo-basic");
    assert_eq!(tool.spec.description, "Basic echo command (OpenClaw compatible)");
    assert_eq!(tool.spec.metadata.requires.bins, vec!["echo"]);
}

#[tokio::test]
async fn test_load_aether_enhanced_skill() {
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/markdown_skills/gh-pr-docker");

    let tools = load_skills_from_dir(fixtures_dir).await;

    assert_eq!(tools.len(), 1);
    let tool = &tools[0];
    assert_eq!(tool.spec.name, "gh-pr-docker");

    // Check Aether extensions
    let aether = tool.spec.metadata.aether.as_ref().unwrap();
    assert!(matches!(
        aether.security.sandbox,
        aethecore::tools::markdown_skill::SandboxMode::Docker
    ));

    // Check Docker config
    let docker = aether.docker.as_ref().unwrap();
    assert_eq!(docker.image, "ghcr.io/cli/cli:latest");
    assert!(docker.env_vars.contains(&"GITHUB_TOKEN".to_string()));

    // Check input hints
    assert!(aether.input_hints.contains_key("action"));
    assert!(aether.input_hints.contains_key("repo"));
}

#[tokio::test]
async fn test_partial_failure_tolerance() {
    let fixtures_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/markdown_skills");

    let loader = SkillLoader::new(fixtures_dir);
    let (tools, errors) = loader.load_all().await;

    // Should load 2 valid skills
    assert_eq!(tools.len(), 2);

    // Should have 1 error (invalid-yaml)
    assert_eq!(errors.len(), 1);
    assert!(errors[0].0.to_string_lossy().contains("invalid-yaml"));
}

#[tokio::test]
async fn test_tool_definition_includes_llm_context() {
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/markdown_skills/echo-basic");

    let tools = load_skills_from_dir(fixtures_dir).await;
    let tool = &tools[0];

    let definition = tool.definition();

    // Check that llm_context is populated
    assert!(definition.llm_context.is_some());
    let context = definition.llm_context.unwrap();

    // Should contain examples section
    assert!(context.contains("echo"));
}

#[tokio::test]
async fn test_tool_server_integration() {
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/markdown_skills/echo-basic");

    let server = AetherToolServer::new_with_skills(vec![fixtures_dir]).await;

    // Debug: list all tools
    let names = server.list_names().await;
    println!("Loaded tools: {:?}", names);

    // Check that the skill was loaded
    assert!(
        server.has_tool("echo-basic").await,
        "echo-basic tool not found. Available tools: {:?}",
        names
    );

    // Get definition
    let def = server.get_definition("echo-basic").await.unwrap();
    assert_eq!(def.name, "echo-basic");
}

#[tokio::test]
async fn test_echo_execution() {
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/markdown_skills/echo-basic");

    let tools = load_skills_from_dir(fixtures_dir).await;
    let tool = &tools[0];

    // Test actual execution with args array mode
    let args = serde_json::json!({
        "args": ["Hello", "World"]
    });

    let result = tool.call(args).await.unwrap();

    // Echo command should produce output
    assert!(result.stdout.contains("Hello") || result.stdout.contains("World"));
    assert_eq!(result.exit_code, 0);
}

#[tokio::test]
async fn test_schema_generation_with_hints() {
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/markdown_skills/gh-pr-docker");

    let tools = load_skills_from_dir(fixtures_dir).await;
    let tool = &tools[0];

    let definition = tool.definition();
    let schema = definition.parameters;

    // Check that schema was generated from input_hints
    let properties = schema.get("properties").unwrap().as_object().unwrap();
    assert!(properties.contains_key("action"));
    assert!(properties.contains_key("repo"));
    assert!(properties.contains_key("number"));

    // Check required fields
    let required = schema.get("required").unwrap().as_array().unwrap();
    assert!(required
        .iter()
        .any(|v| v.as_str().unwrap() == "action"));
    assert!(required.iter().any(|v| v.as_str().unwrap() == "repo"));
    // "number" should NOT be required (it's optional)
    assert!(!required
        .iter()
        .any(|v| v.as_str().unwrap() == "number"));
}
