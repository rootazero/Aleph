//! Integration tests for Markdown Skill hot reload

use aethecore::tools::markdown_skill::{
    SkillWatcher, SkillWatcherConfig, ReloadCallback, MarkdownCliTool,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::sleep;

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

#[tokio::test]
async fn test_watcher_detects_skill_creation() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(&skills_dir).unwrap();

    // Create reload callback that tracks reloaded tools
    let reloaded_tools = Arc::new(Mutex::new(Vec::new()));
    let reloaded_tools_clone = reloaded_tools.clone();

    let callback: ReloadCallback = Arc::new(move |tools| {
        let mut reloaded = reloaded_tools_clone.lock().unwrap();
        reloaded.extend(tools);
        Ok(())
    });

    // Create watcher with short debounce for testing
    let config = SkillWatcherConfig {
        debounce_duration: Duration::from_millis(100),
        emit_initial_events: false,
    };

    let watcher = SkillWatcher::new(&skills_dir, callback.clone(), config).unwrap();

    // Spawn watcher in background
    let skills_dir_clone = skills_dir.clone();
    let watcher_task = tokio::spawn(async move {
        watcher.run(skills_dir_clone, callback).await
    });

    // Wait for watcher to start
    sleep(Duration::from_millis(200)).await;

    // Create a new skill
    let skill_dir = skills_dir.join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), TEST_SKILL_MD).unwrap();

    // Wait for file event to be processed
    sleep(Duration::from_millis(300)).await;

    // Verify that the callback was invoked
    let reloaded = reloaded_tools.lock().unwrap();
    assert!(
        !reloaded.is_empty(),
        "Expected skills to be reloaded after file creation"
    );
    assert_eq!(reloaded[0].spec.name, "test-skill");

    // Cleanup
    watcher_task.abort();
}

#[tokio::test]
async fn test_watcher_detects_skill_modification() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(&skills_dir).unwrap();

    // Create initial skill
    let skill_dir = skills_dir.join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), TEST_SKILL_MD).unwrap();

    // Track reload count
    let reload_count = Arc::new(Mutex::new(0));
    let reload_count_clone = reload_count.clone();

    let callback: ReloadCallback = Arc::new(move |tools| {
        *reload_count_clone.lock().unwrap() += tools.len();
        Ok(())
    });

    // Create watcher
    let config = SkillWatcherConfig {
        debounce_duration: Duration::from_millis(100),
        emit_initial_events: false,
    };

    let watcher = SkillWatcher::new(&skills_dir, callback.clone(), config).unwrap();

    // Spawn watcher
    let skills_dir_clone = skills_dir.clone();
    let watcher_task = tokio::spawn(async move {
        watcher.run(skills_dir_clone, callback).await
    });

    // Wait for watcher to start
    sleep(Duration::from_millis(200)).await;

    // Modify the skill
    let modified_skill = TEST_SKILL_MD.replace("A test skill", "A modified test skill");
    std::fs::write(skill_dir.join("SKILL.md"), modified_skill).unwrap();

    // Wait for reload
    sleep(Duration::from_millis(300)).await;

    // Verify reload was triggered
    let count = *reload_count.lock().unwrap();
    assert!(count > 0, "Expected skills to be reloaded after modification");

    // Cleanup
    watcher_task.abort();
}

#[test]
fn test_watcher_config_defaults() {
    let config = SkillWatcherConfig::default();
    assert_eq!(config.debounce_duration, Duration::from_millis(500));
    assert!(!config.emit_initial_events);
}

#[tokio::test]
async fn test_watcher_ignores_non_skill_files() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(&skills_dir).unwrap();

    let reload_count = Arc::new(Mutex::new(0));
    let reload_count_clone = reload_count.clone();

    let callback: ReloadCallback = Arc::new(move |tools| {
        *reload_count_clone.lock().unwrap() += tools.len();
        Ok(())
    });

    let config = SkillWatcherConfig {
        debounce_duration: Duration::from_millis(100),
        emit_initial_events: false,
    };

    let watcher = SkillWatcher::new(&skills_dir, callback.clone(), config).unwrap();

    let skills_dir_clone = skills_dir.clone();
    let watcher_task = tokio::spawn(async move {
        watcher.run(skills_dir_clone, callback).await
    });

    sleep(Duration::from_millis(200)).await;

    // Create non-skill files
    std::fs::write(skills_dir.join("README.md"), "# README").unwrap();
    std::fs::write(skills_dir.join("config.json"), "{}").unwrap();

    // Wait to ensure no reloads are triggered
    sleep(Duration::from_millis(300)).await;

    let count = *reload_count.lock().unwrap();
    assert_eq!(count, 0, "Non-skill files should not trigger reloads");

    // Cleanup
    watcher_task.abort();
}
