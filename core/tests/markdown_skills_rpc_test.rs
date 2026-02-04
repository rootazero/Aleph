//! Integration tests for Markdown Skills RPC handlers

use aethecore::gateway::handlers::HandlerRegistry;
use aethecore::gateway::protocol::JsonRpcRequest;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

#[tokio::test]
async fn test_markdown_skills_handlers_registered() {
    let registry = HandlerRegistry::new();

    assert!(registry.has_method("markdown_skills.load"));
    assert!(registry.has_method("markdown_skills.reload"));
    assert!(registry.has_method("markdown_skills.list"));
    assert!(registry.has_method("markdown_skills.unload"));
}

#[tokio::test]
async fn test_load_skill_success() {
    let registry = HandlerRegistry::new();

    // Create temporary directory with a valid SKILL.md
    let temp_dir = TempDir::new().unwrap();
    let skill_path = temp_dir.path().join("test-skill");
    fs::create_dir(&skill_path).unwrap();

    let skill_md = skill_path.join("SKILL.md");
    // NOTE: Parser expects "\n---\n" pattern, so ensure proper spacing
    // NOTE: ConfirmationMode values are: always, write, never (not "none")
    fs::write(
        &skill_md,
        "---
name: test-skill
description: Test skill for integration testing
metadata:
  requires:
    bins: [\"echo\"]
  aether:
    security:
      sandbox: host
      confirmation: never
---

# Test Skill

Test skill content for integration testing.
",
    )
    .unwrap();

    // Create load request
    let request = JsonRpcRequest::new(
        "markdown_skills.load",
        Some(json!({
            "path": skill_path.to_string_lossy().to_string()
        })),
        Some(json!(1)),
    );

    // Handle request
    let response = registry.handle(&request).await;

    // Verify success
    assert!(response.is_success(), "Response should be success: {:?}", response);
    let result = response.result.unwrap();
    assert_eq!(result["count"], 1);

    let skills = result["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["name"], "test-skill");
    assert_eq!(skills[0]["description"], "Test skill for integration testing");
    assert_eq!(skills[0]["sandbox_mode"], "host");
}

#[tokio::test]
async fn test_load_skill_invalid_path() {
    let registry = HandlerRegistry::new();

    // Create request with non-existent path
    let request = JsonRpcRequest::new(
        "markdown_skills.load",
        Some(json!({
            "path": "/nonexistent/path"
        })),
        Some(json!(2)),
    );

    // Handle request
    let response = registry.handle(&request).await;

    // Should return error (no skills found)
    assert!(response.is_error(), "Should return error for invalid path");
}

#[tokio::test]
async fn test_list_skills() {
    let registry = HandlerRegistry::new();

    // Create request
    let request = JsonRpcRequest::new(
        "markdown_skills.list",
        None,
        Some(json!(3)),
    );

    // Handle request (should succeed even if empty)
    let response = registry.handle(&request).await;

    assert!(response.is_success());
    let result = response.result.unwrap();
    assert!(result["skills"].is_array());
    assert!(result["count"].is_number());
}

#[tokio::test]
async fn test_reload_skill_not_found() {
    let registry = HandlerRegistry::new();

    // Create reload request for non-existent skill
    let request = JsonRpcRequest::new(
        "markdown_skills.reload",
        Some(json!({
            "name": "nonexistent-skill"
        })),
        Some(json!(4)),
    );

    // Handle request
    let response = registry.handle(&request).await;

    // Should return error
    assert!(response.is_error());
    let error = response.error.unwrap();
    assert!(error.message.contains("not found"));
}

#[tokio::test]
async fn test_unload_skill() {
    let registry = HandlerRegistry::new();

    // Create unload request
    let request = JsonRpcRequest::new(
        "markdown_skills.unload",
        Some(json!({
            "name": "test-skill"
        })),
        Some(json!(5)),
    );

    // Handle request (should succeed even if skill doesn't exist)
    let response = registry.handle(&request).await;

    assert!(response.is_success());
    let result = response.result.unwrap();
    assert!(result["removed"].is_boolean());
}

#[tokio::test]
async fn test_load_and_reload_flow() {
    let registry = HandlerRegistry::new();

    // Create temporary directory with a valid SKILL.md
    let temp_dir = TempDir::new().unwrap();
    let skill_path = temp_dir.path().join("reload-test-skill");
    fs::create_dir(&skill_path).unwrap();

    let skill_md = skill_path.join("SKILL.md");
    fs::write(
        &skill_md,
        r#"---
name: reload-test
description: Original description
metadata:
  requires:
    bins: ["echo"]
  aether:
    security:
      sandbox: host
      confirmation: never
---

# Reload Test

Original content.
"#,
    )
    .unwrap();

    // Load skill
    let load_request = JsonRpcRequest::new(
        "markdown_skills.load",
        Some(json!({
            "path": skill_path.to_string_lossy().to_string()
        })),
        Some(json!(6)),
    );

    let load_response = registry.handle(&load_request).await;
    assert!(load_response.is_success());

    // Update skill file
    fs::write(
        &skill_md,
        r#"---
name: reload-test
description: Updated description
metadata:
  requires:
    bins: ["echo"]
  aether:
    security:
      sandbox: host
      confirmation: never
---

# Reload Test

Updated content.
"#,
    )
    .unwrap();

    // Reload skill
    let reload_request = JsonRpcRequest::new(
        "markdown_skills.reload",
        Some(json!({
            "name": "reload-test"
        })),
        Some(json!(7)),
    );

    let reload_response = registry.handle(&reload_request).await;
    assert!(reload_response.is_success());

    let result = reload_response.result.unwrap();
    assert_eq!(result["was_replaced"], true);

    let skill = &result["skill"];
    assert_eq!(skill["name"], "reload-test");
    assert_eq!(skill["description"], "Updated description");
}

#[tokio::test]
async fn test_missing_params() {
    let registry = HandlerRegistry::new();

    // Test load without params
    let request = JsonRpcRequest::new(
        "markdown_skills.load",
        None,
        Some(json!(8)),
    );

    let response = registry.handle(&request).await;
    assert!(response.is_error());
    assert!(response.error.unwrap().message.contains("Missing params"));

    // Test reload without params
    let request = JsonRpcRequest::new(
        "markdown_skills.reload",
        None,
        Some(json!(9)),
    );

    let response = registry.handle(&request).await;
    assert!(response.is_error());

    // Test unload without params
    let request = JsonRpcRequest::new(
        "markdown_skills.unload",
        None,
        Some(json!(10)),
    );

    let response = registry.handle(&request).await;
    assert!(response.is_error());
}
