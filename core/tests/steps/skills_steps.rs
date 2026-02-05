//! Step definitions for Markdown Skills features

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cucumber::{given, then, when};
use serde_json::json;
use tempfile::TempDir;
use tokio::time::sleep;

use crate::world::{AlephWorld, SkillsContext};
use alephcore::gateway::handlers::HandlerRegistry;
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::skill_evolution::types::{SkillMetrics, SolidificationSuggestion};
use alephcore::tools::markdown_skill::{
    load_skills_from_dir, MarkdownSkillGenerator, MarkdownSkillGeneratorConfig, ReloadCallback,
    SkillLoader, SkillWatcher, SkillWatcherConfig,
};
use alephcore::tools::AlephToolServer;

const TEST_SKILL_MD: &str = r#"---
name: test-skill
description: "A test skill"
metadata:
  requires:
    bins:
      - "echo"
---

# Test Skill

This is a test skill for hot reload testing.

## Examples

```bash
echo "hello"
```
"#;

// =============================================================================
// Given Steps - RPC Handlers
// =============================================================================

#[given("a handler registry")]
async fn given_handler_registry(w: &mut AlephWorld) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    ctx.registry = Some(HandlerRegistry::new());
}

#[given("a temp directory with a valid skill")]
async fn given_temp_dir_with_valid_skill(w: &mut AlephWorld) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    let temp_dir = TempDir::new().unwrap();
    let skill_path = temp_dir.path().join("test-skill");
    std::fs::create_dir(&skill_path).unwrap();

    let skill_md = skill_path.join("SKILL.md");
    std::fs::write(
        &skill_md,
        r#"---
name: test-skill
description: Test skill for integration testing
metadata:
  requires:
    bins: ["echo"]
  aleph:
    security:
      sandbox: host
      confirmation: never
---

# Test Skill

Test skill content for integration testing.
"#,
    )
    .unwrap();

    ctx.skill_dir = Some(skill_path);
    ctx.temp_dir = Some(temp_dir);
}

#[given("a temp directory with a reload-test skill")]
async fn given_temp_dir_with_reload_skill(w: &mut AlephWorld) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    let temp_dir = TempDir::new().unwrap();
    let skill_path = temp_dir.path().join("reload-test-skill");
    std::fs::create_dir(&skill_path).unwrap();

    let skill_md = skill_path.join("SKILL.md");
    std::fs::write(
        &skill_md,
        r#"---
name: reload-test
description: Original description
metadata:
  requires:
    bins: ["echo"]
  aleph:
    security:
      sandbox: host
      confirmation: never
---

# Reload Test

Original content.
"#,
    )
    .unwrap();

    ctx.skill_dir = Some(skill_path);
    ctx.temp_dir = Some(temp_dir);
}

// =============================================================================
// Given Steps - Skill Loading
// =============================================================================

#[given("the echo-basic fixture skill")]
async fn given_echo_basic_fixture(w: &mut AlephWorld) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/markdown_skills/echo-basic");
    ctx.skill_dir = Some(fixtures_dir);
}

#[given("the gh-pr-docker fixture skill")]
async fn given_gh_pr_docker_fixture(w: &mut AlephWorld) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/markdown_skills/gh-pr-docker");
    ctx.skill_dir = Some(fixtures_dir);
}

#[given("the markdown skills fixtures directory")]
async fn given_fixtures_directory(w: &mut AlephWorld) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    let fixtures_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/markdown_skills");
    ctx.skill_dir = Some(fixtures_dir);
}

// =============================================================================
// Given Steps - Skill Generator
// =============================================================================

#[given("a skill generator with temp output directory")]
async fn given_skill_generator(w: &mut AlephWorld) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    let temp_dir = TempDir::new().unwrap();
    let config = MarkdownSkillGeneratorConfig {
        output_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    ctx.generator = Some(MarkdownSkillGenerator::with_config(config));
    ctx.temp_dir = Some(temp_dir);
}

#[given(expr = "a suggestion with name {string} and description {string} and confidence {float}")]
async fn given_suggestion(w: &mut AlephWorld, name: String, description: String, confidence: f32) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    ctx.suggestion = Some(ctx.create_test_suggestion("test-pattern", &name, &description, confidence));
}

#[given(expr = "the suggestion has pattern_id {string}")]
async fn given_suggestion_pattern_id(w: &mut AlephWorld, pattern_id: String) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    if let Some(ref mut suggestion) = ctx.suggestion {
        suggestion.pattern_id = pattern_id;
    }
}

#[given(expr = "the suggestion has sample contexts {string}")]
async fn given_suggestion_sample_contexts(w: &mut AlephWorld, contexts_str: String) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    if let Some(ref mut suggestion) = ctx.suggestion {
        suggestion.sample_contexts = contexts_str.split('|').map(String::from).collect();
    }
}

#[given(expr = "the suggestion has instructions preview {string}")]
async fn given_suggestion_instructions(w: &mut AlephWorld, instructions: String) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    if let Some(ref mut suggestion) = ctx.suggestion {
        suggestion.instructions_preview = instructions;
    }
}

// =============================================================================
// Given Steps - Hot Reload
// =============================================================================

#[given("an empty temp directory for skills")]
async fn given_empty_skills_dir(w: &mut AlephWorld) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    let temp_dir = TempDir::new().unwrap();
    ctx.skill_dir = Some(temp_dir.path().to_path_buf());
    ctx.temp_dir = Some(temp_dir);
}

#[given("a skill directory with an existing skill")]
async fn given_skill_dir_with_skill(w: &mut AlephWorld) {
    let ctx = w.skills.get_or_insert_with(SkillsContext::default);
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(&skills_dir).unwrap();

    // Create initial skill
    let skill_dir = skills_dir.join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), TEST_SKILL_MD).unwrap();

    ctx.skill_dir = Some(skills_dir);
    ctx.temp_dir = Some(temp_dir);
}

#[given(expr = "a watcher config with debounce {int}ms")]
async fn given_watcher_config(w: &mut AlephWorld, debounce_ms: u64) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    ctx.watcher_config = Some(SkillWatcherConfig {
        debounce_duration: Duration::from_millis(debounce_ms),
        emit_initial_events: false,
    });
}

// =============================================================================
// When Steps - RPC Handlers
// =============================================================================

#[when("I check registered markdown_skills handlers")]
async fn when_check_registered_handlers(w: &mut AlephWorld) {
    // No action needed, assertions will check the registry
}

#[when("I send markdown_skills.load with the skill path")]
async fn when_send_load_request(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let skill_path = ctx.skill_dir.as_ref().expect("Skill dir not set");

    let request = JsonRpcRequest::new(
        "markdown_skills.load",
        Some(json!({
            "path": skill_path.to_string_lossy().to_string()
        })),
        Some(json!(1)),
    );

    ctx.rpc_response = Some(registry.handle(&request).await);
}

#[when("I send markdown_skills.load with invalid path")]
async fn when_send_load_invalid_path(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");

    let request = JsonRpcRequest::new(
        "markdown_skills.load",
        Some(json!({
            "path": "/nonexistent/path"
        })),
        Some(json!(2)),
    );

    ctx.rpc_response = Some(registry.handle(&request).await);
}

#[when("I send markdown_skills.list")]
async fn when_send_list_request(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");

    let request = JsonRpcRequest::new("markdown_skills.list", None, Some(json!(3)));

    ctx.rpc_response = Some(registry.handle(&request).await);
}

#[when(expr = "I send markdown_skills.reload for skill {string}")]
async fn when_send_reload_request(w: &mut AlephWorld, skill_name: String) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");

    let request = JsonRpcRequest::new(
        "markdown_skills.reload",
        Some(json!({
            "name": skill_name
        })),
        Some(json!(4)),
    );

    ctx.rpc_response = Some(registry.handle(&request).await);
}

#[when(expr = "I send markdown_skills.unload for skill {string}")]
async fn when_send_unload_request(w: &mut AlephWorld, skill_name: String) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");

    let request = JsonRpcRequest::new(
        "markdown_skills.unload",
        Some(json!({
            "name": skill_name
        })),
        Some(json!(5)),
    );

    ctx.rpc_response = Some(registry.handle(&request).await);
}

#[when("I send markdown_skills.load without params")]
async fn when_send_load_no_params(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");

    let request = JsonRpcRequest::new("markdown_skills.load", None, Some(json!(6)));

    ctx.rpc_response = Some(registry.handle(&request).await);
}

#[when("I send markdown_skills.reload without params")]
async fn when_send_reload_no_params(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");

    let request = JsonRpcRequest::new("markdown_skills.reload", None, Some(json!(7)));

    ctx.rpc_response = Some(registry.handle(&request).await);
}

#[when("I send markdown_skills.unload without params")]
async fn when_send_unload_no_params(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");

    let request = JsonRpcRequest::new("markdown_skills.unload", None, Some(json!(8)));

    ctx.rpc_response = Some(registry.handle(&request).await);
}

#[when(expr = "I update the skill description to {string}")]
async fn when_update_skill_description(w: &mut AlephWorld, new_description: String) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skill_path = ctx.skill_dir.as_ref().expect("Skill dir not set");
    let skill_md = skill_path.join("SKILL.md");

    std::fs::write(
        &skill_md,
        format!(
            r#"---
name: reload-test
description: {}
metadata:
  requires:
    bins: ["echo"]
  aleph:
    security:
      sandbox: host
      confirmation: never
---

# Reload Test

Updated content.
"#,
            new_description
        ),
    )
    .unwrap();
}

// =============================================================================
// When Steps - Skill Loading
// =============================================================================

#[when("I load skills from the directory")]
async fn when_load_skills_from_dir(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skill_dir = ctx.skill_dir.as_ref().expect("Skill dir not set").clone();
    ctx.loaded_tools = load_skills_from_dir(skill_dir).await;
}

#[when("I load skills with error handling")]
async fn when_load_skills_with_errors(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skill_dir = ctx.skill_dir.as_ref().expect("Skill dir not set").clone();
    let loader = SkillLoader::new(skill_dir);
    let (tools, errors) = loader.load_all().await;
    ctx.loaded_tools = tools;
    ctx.load_errors = errors;
}

#[when("I create a tool server with the skill")]
async fn when_create_tool_server(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skill_dir = ctx.skill_dir.as_ref().expect("Skill dir not set").clone();
    ctx.tool_server = Some(AlephToolServer::new_with_skills(vec![skill_dir]).await);
}

#[when("I execute the echo skill with Hello World")]
async fn when_execute_skill(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let args = serde_json::json!({
        "args": ["Hello", "World"]
    });
    if let Some(tool) = ctx.loaded_tools.first() {
        let result = tool.call(args).await;
        if let Ok(output) = result {
            // Store result in a way we can check later
            ctx.generated_content = Some(output.stdout.clone());
        }
    }
}

// =============================================================================
// When Steps - Skill Generator
// =============================================================================

#[when("I generate the skill")]
async fn when_generate_skill(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let generator = ctx.generator.as_ref().expect("Generator not initialized");
    let suggestion = ctx.suggestion.as_ref().expect("Suggestion not set");

    let result = generator.generate(suggestion);
    if let Ok(path) = result {
        ctx.generated_skill_path = Some(path.clone());
        ctx.generated_content = Some(std::fs::read_to_string(&path).unwrap());
    }
}

#[when("I load the generated skill")]
async fn when_load_generated_skill(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skill_path = ctx
        .generated_skill_path
        .as_ref()
        .expect("Generated skill path not set");
    let skill_dir = skill_path.parent().unwrap().to_path_buf();
    ctx.loaded_tools = load_skills_from_dir(skill_dir).await;
}

// =============================================================================
// When Steps - Hot Reload
// =============================================================================

#[when("I start a skill watcher")]
async fn when_start_watcher(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skills_dir = ctx.skill_dir.as_ref().expect("Skill dir not set").clone();
    let config = ctx.watcher_config.clone().unwrap_or_default();
    let callback = ctx.create_reload_callback();

    let watcher = SkillWatcher::new(&skills_dir, callback.clone(), config).unwrap();

    let task = tokio::spawn(async move { watcher.run(skills_dir, callback).await });

    ctx.watcher_task = Some(task);

    // Wait for watcher to start
    sleep(Duration::from_millis(200)).await;
}

#[when("I start a counting skill watcher")]
async fn when_start_counting_watcher(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skills_dir = ctx.skill_dir.as_ref().expect("Skill dir not set").clone();
    let config = ctx.watcher_config.clone().unwrap_or_default();
    let callback = ctx.create_counting_callback();

    let watcher = SkillWatcher::new(&skills_dir, callback.clone(), config).unwrap();

    let task = tokio::spawn(async move { watcher.run(skills_dir, callback).await });

    ctx.watcher_task = Some(task);

    // Wait for watcher to start
    sleep(Duration::from_millis(200)).await;
}

#[when("I create a new skill file")]
async fn when_create_skill_file(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skills_dir = ctx.skill_dir.as_ref().expect("Skill dir not set");

    let skill_dir = skills_dir.join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), TEST_SKILL_MD).unwrap();

    // Wait for file event to be processed
    sleep(Duration::from_millis(300)).await;
}

#[when("I modify the existing skill file")]
async fn when_modify_skill_file(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skills_dir = ctx.skill_dir.as_ref().expect("Skill dir not set");

    let skill_md = skills_dir.join("test-skill/SKILL.md");
    let modified_skill = TEST_SKILL_MD.replace("A test skill", "A modified test skill");
    std::fs::write(&skill_md, modified_skill).unwrap();

    // Wait for file event to be processed
    sleep(Duration::from_millis(300)).await;
}

#[when("I create non-skill files")]
async fn when_create_non_skill_files(w: &mut AlephWorld) {
    let ctx = w.skills.as_mut().expect("Skills context not initialized");
    let skills_dir = ctx.skill_dir.as_ref().expect("Skill dir not set");

    std::fs::write(skills_dir.join("README.md"), "# README").unwrap();
    std::fs::write(skills_dir.join("config.json"), "{}").unwrap();

    // Wait to ensure no reloads are triggered
    sleep(Duration::from_millis(300)).await;
}

// =============================================================================
// Then Steps - RPC Handlers
// =============================================================================

#[then("the markdown_skills.load handler should be registered")]
async fn then_load_handler_registered(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(registry.has_method("markdown_skills.load"));
}

#[then("the markdown_skills.reload handler should be registered")]
async fn then_reload_handler_registered(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(registry.has_method("markdown_skills.reload"));
}

#[then("the markdown_skills.list handler should be registered")]
async fn then_list_handler_registered(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(registry.has_method("markdown_skills.list"));
}

#[then("the markdown_skills.unload handler should be registered")]
async fn then_unload_handler_registered(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(registry.has_method("markdown_skills.unload"));
}

#[then("the skills RPC response should be successful")]
async fn then_response_successful(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    assert!(
        response.is_success(),
        "Response should be success: {:?}",
        response
    );
}

#[then("the skills RPC response should be an error")]
async fn then_response_error(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    assert!(response.is_error(), "Response should be error");
}

#[then(expr = "the response result count should be {int}")]
async fn then_response_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    assert_eq!(result["count"], expected);
}

#[then(expr = "the response skills array should have {int} items")]
async fn then_response_skills_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    let skills = result["skills"].as_array().expect("Skills not an array");
    assert_eq!(skills.len(), expected);
}

#[then(expr = "the first skill name should be {string}")]
async fn then_first_skill_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    let skills = result["skills"].as_array().expect("Skills not an array");
    assert_eq!(skills[0]["name"], expected);
}

#[then(expr = "the first skill description should be {string}")]
async fn then_first_skill_description(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    let skills = result["skills"].as_array().expect("Skills not an array");
    assert_eq!(skills[0]["description"], expected);
}

#[then(expr = "the first skill sandbox_mode should be {string}")]
async fn then_first_skill_sandbox(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    let skills = result["skills"].as_array().expect("Skills not an array");
    assert_eq!(skills[0]["sandbox_mode"], expected);
}

#[then(expr = "the skills RPC error message should contain {string}")]
async fn then_error_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let error = response.error.as_ref().expect("Error not set");
    assert!(
        error.message.contains(&expected),
        "Error message '{}' should contain '{}'",
        error.message,
        expected
    );
}

#[then("the response result should have skills array")]
async fn then_response_has_skills_array(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    assert!(result["skills"].is_array());
}

#[then("the response result should have count number")]
async fn then_response_has_count(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    assert!(result["count"].is_number());
}

#[then("the response result should have removed boolean")]
async fn then_response_has_removed(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    assert!(result["removed"].is_boolean());
}

#[then("the reload was_replaced should be true")]
async fn then_reload_was_replaced(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    assert_eq!(result["was_replaced"], true);
}

#[then(expr = "the reloaded skill description should be {string}")]
async fn then_reloaded_skill_description(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let response = ctx.rpc_response.as_ref().expect("Response not set");
    let result = response.result.as_ref().expect("Result not set");
    assert_eq!(result["skill"]["description"], expected);
}

// =============================================================================
// Then Steps - Skill Loading
// =============================================================================

#[then(expr = "the loaded tools count should be {int}")]
async fn then_loaded_tools_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    assert_eq!(ctx.loaded_tools.len(), expected);
}

#[then(expr = "the first loaded tool name should be {string}")]
async fn then_first_tool_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    assert_eq!(ctx.loaded_tools[0].spec.name, expected);
}

#[then(expr = "the first loaded tool description should be {string}")]
async fn then_first_tool_description(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    assert_eq!(ctx.loaded_tools[0].spec.description, expected);
}

#[then(expr = "the first loaded tool should require bin {string}")]
async fn then_first_tool_requires_bin(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    assert!(ctx.loaded_tools[0]
        .spec
        .metadata
        .requires
        .bins
        .contains(&expected));
}

#[then("the first loaded tool should have aleph extensions")]
async fn then_first_tool_has_aleph(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    assert!(ctx.loaded_tools[0].spec.metadata.aleph.is_some());
}

#[then("the first loaded tool sandbox should be docker")]
async fn then_first_tool_sandbox_docker(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let aleph_meta = ctx.loaded_tools[0].spec.metadata.aleph.as_ref().unwrap();
    assert!(matches!(
        aleph_meta.security.sandbox,
        alephcore::tools::markdown_skill::SandboxMode::Docker
    ));
}

#[then(expr = "the docker image should be {string}")]
async fn then_docker_image(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let aleph_meta = ctx.loaded_tools[0].spec.metadata.aleph.as_ref().unwrap();
    let docker = aleph_meta.docker.as_ref().unwrap();
    assert_eq!(docker.image, expected);
}

#[then(expr = "the docker env_vars should include {string}")]
async fn then_docker_env_vars(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let aleph_meta = ctx.loaded_tools[0].spec.metadata.aleph.as_ref().unwrap();
    let docker = aleph_meta.docker.as_ref().unwrap();
    assert!(docker.env_vars.contains(&expected));
}

#[then(expr = "the input_hints should have key {string}")]
async fn then_input_hints_has_key(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let aleph_meta = ctx.loaded_tools[0].spec.metadata.aleph.as_ref().unwrap();
    assert!(aleph_meta.input_hints.contains_key(&expected));
}

#[then(expr = "the load errors count should be {int}")]
async fn then_load_errors_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    assert_eq!(ctx.load_errors.len(), expected);
}

#[then(expr = "the first error path should contain {string}")]
async fn then_first_error_path_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    assert!(ctx.load_errors[0].0.to_string_lossy().contains(&expected));
}

#[then("the tool definition should have llm_context")]
async fn then_definition_has_llm_context(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let definition = ctx.loaded_tools[0].definition();
    assert!(definition.llm_context.is_some());
}

#[then(expr = "the skill llm_context should contain {string}")]
async fn then_llm_context_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let definition = ctx.loaded_tools[0].definition();
    let context = definition.llm_context.as_ref().unwrap();
    assert!(context.contains(&expected));
}

#[then(expr = "the tool server should have tool {string}")]
async fn then_server_has_tool(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let server = ctx.tool_server.as_ref().expect("Tool server not set");
    let has = server.has_tool(&expected).await;
    let names = server.list_names().await;
    assert!(
        has,
        "{} tool not found. Available tools: {:?}",
        expected, names
    );
}

#[then(expr = "the server tool definition name should be {string}")]
async fn then_server_tool_def_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let server = ctx.tool_server.as_ref().expect("Tool server not set");
    let def = server.get_definition(&expected).await.unwrap();
    assert_eq!(def.name, expected);
}

#[then("the execution result should contain Hello")]
async fn then_execution_contains_hello(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let content = ctx.generated_content.as_ref().expect("Content not set");
    assert!(content.contains("Hello") || content.contains("World"));
}

#[then(expr = "the schema properties should have key {string}")]
async fn then_schema_has_property(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let definition = ctx.loaded_tools[0].definition();
    let properties = definition.parameters.get("properties").unwrap().as_object().unwrap();
    assert!(properties.contains_key(&expected));
}

#[then(expr = "the schema required should include {string}")]
async fn then_schema_required_includes(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let definition = ctx.loaded_tools[0].definition();
    let required = definition.parameters.get("required").unwrap().as_array().unwrap();
    assert!(required.iter().any(|v| v.as_str().unwrap() == expected));
}

#[then(expr = "the schema required should not include {string}")]
async fn then_schema_required_not_includes(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let definition = ctx.loaded_tools[0].definition();
    let required = definition.parameters.get("required").unwrap().as_array().unwrap();
    assert!(!required.iter().any(|v| v.as_str().unwrap() == expected));
}

// =============================================================================
// Then Steps - Skill Generator
// =============================================================================

#[then("the generated skill path should exist")]
async fn then_generated_path_exists(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let path = ctx
        .generated_skill_path
        .as_ref()
        .expect("Path not set");
    assert!(path.exists());
}

#[then("the generated skill path should end with SKILL.md")]
async fn then_generated_path_ends_skill_md(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let path = ctx
        .generated_skill_path
        .as_ref()
        .expect("Path not set");
    assert!(path.ends_with("SKILL.md"));
}

#[then(expr = "the generated content should contain {string}")]
async fn then_generated_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let content = ctx.generated_content.as_ref().expect("Content not set");
    assert!(content.contains(&expected), "Content should contain '{}': {}", expected, content);
}

#[then(expr = "the generated skill directory name should be {string}")]
async fn then_generated_dir_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let path = ctx
        .generated_skill_path
        .as_ref()
        .expect("Path not set");
    let dir_name = path
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(dir_name, expected, "Failed for path: {:?}", path);
}

#[then(expr = "the loaded generated tool name should be {string}")]
async fn then_loaded_gen_tool_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    assert_eq!(ctx.loaded_tools[0].spec.name, expected);
}

#[then("the loaded generated tool should have evolution metadata")]
async fn then_loaded_gen_has_evolution(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let aleph_meta = ctx.loaded_tools[0].spec.metadata.aleph.as_ref().unwrap();
    assert!(aleph_meta.evolution.is_some());
}

#[then("the evolution source should be auto-generated")]
async fn then_evolution_source_auto(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let aleph_meta = ctx.loaded_tools[0].spec.metadata.aleph.as_ref().unwrap();
    let evolution = aleph_meta.evolution.as_ref().unwrap();
    assert_eq!(evolution.source, "auto-generated");
}

#[then(expr = "the evolution confidence score should be approximately {float}")]
async fn then_evolution_confidence(w: &mut AlephWorld, expected: f64) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let aleph_meta = ctx.loaded_tools[0].spec.metadata.aleph.as_ref().unwrap();
    let evolution = aleph_meta.evolution.as_ref().unwrap();
    assert!(
        (evolution.confidence_score - expected).abs() < 0.001,
        "Expected {}, got {}",
        expected,
        evolution.confidence_score
    );
}

#[then(expr = "the evolution created_from_trace should be {string}")]
async fn then_evolution_trace(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let aleph_meta = ctx.loaded_tools[0].spec.metadata.aleph.as_ref().unwrap();
    let evolution = aleph_meta.evolution.as_ref().unwrap();
    assert_eq!(evolution.created_from_trace.as_ref().unwrap(), &expected);
}

// =============================================================================
// Then Steps - Hot Reload
// =============================================================================

#[then("the reload count should be greater than 0")]
async fn then_reload_count_gt_0(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let count = *ctx.reload_count.lock().unwrap();
    assert!(count > 0, "Expected reload count > 0, got {}", count);
}

#[then("the reloaded tools should not be empty")]
async fn then_reloaded_not_empty(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let tools = ctx.reloaded_tools.lock().unwrap();
    assert!(!tools.is_empty(), "Expected reloaded tools to not be empty");
}

#[then(expr = "the first reloaded tool name should be {string}")]
async fn then_first_reloaded_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let tools = ctx.reloaded_tools.lock().unwrap();
    assert_eq!(tools[0].spec.name, expected);
}

#[then("the reload count should be 0")]
async fn then_reload_count_0(w: &mut AlephWorld) {
    let ctx = w.skills.as_ref().expect("Skills context not initialized");
    let count = *ctx.reload_count.lock().unwrap();
    assert_eq!(count, 0, "Non-skill files should not trigger reloads");
}

#[then("the default watcher config debounce should be 500ms")]
async fn then_default_debounce(w: &mut AlephWorld) {
    let config = SkillWatcherConfig::default();
    assert_eq!(config.debounce_duration, Duration::from_millis(500));
}

#[then("the default watcher config emit_initial_events should be false")]
async fn then_default_emit_initial(w: &mut AlephWorld) {
    let config = SkillWatcherConfig::default();
    assert!(!config.emit_initial_events);
}
