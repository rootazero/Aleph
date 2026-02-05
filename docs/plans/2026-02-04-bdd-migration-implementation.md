# BDD Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrate all 72 Rust test files from traditional unit tests to BDD using cucumber-rs framework.

**Architecture:** Cucumber-rs with composite World design, async steps, single test runner with path filtering. Tests organized to mirror source structure.

**Tech Stack:** cucumber 0.21, tokio (async runtime), tempfile (test isolation)

---

## Task 0: Infrastructure Setup

**Files:**
- Modify: `core/Cargo.toml`
- Create: `core/tests/cucumber.rs`
- Create: `core/tests/world/mod.rs`
- Create: `core/tests/steps/mod.rs`
- Create: `core/tests/features/.gitkeep`

### Step 1: Add cucumber dependency to Cargo.toml

Add to `[dev-dependencies]` section in `core/Cargo.toml`:

```toml
cucumber = { version = "0.21", features = ["macros"] }
```

Add test target configuration at the end of the file:

```toml
[[test]]
name = "cucumber"
harness = false
```

### Step 2: Create directory structure

```bash
mkdir -p core/tests/world
mkdir -p core/tests/steps
mkdir -p core/tests/features/config
mkdir -p core/tests/features/scripting
```

### Step 3: Create World base structure

Create `core/tests/world/mod.rs`:

```rust
//! BDD Test World - Shared state between cucumber steps

use cucumber::World;
use tempfile::TempDir;

/// Main World struct for all BDD tests
/// Each module context is lazily initialized via Option<T>
#[derive(Debug, Default, World)]
pub struct AlephWorld {
    // ═══ Common State ═══
    /// Temporary directory for test isolation
    pub temp_dir: Option<TempDir>,
    /// Last operation result (for Then assertions)
    pub last_result: Option<Result<(), String>>,
    /// Last error message captured
    pub last_error: Option<String>,

    // ═══ Module Contexts (added as batches are implemented) ═══
    // pub config: Option<ConfigContext>,
    // pub scripting: Option<ScriptingContext>,
    // ... more contexts added per batch
}
```

### Step 4: Create steps base structure

Create `core/tests/steps/mod.rs`:

```rust
//! BDD Step Definitions
//!
//! Organized by module with shared common steps.

mod common;

pub use common::*;

// Module-specific steps added per batch:
// mod config_steps;
// mod scripting_steps;
// pub use config_steps::*;
// pub use scripting_steps::*;
```

Create `core/tests/steps/common.rs`:

```rust
//! Common step definitions shared across all features

use cucumber::{given, then};
use crate::world::AlephWorld;
use tempfile::tempdir;

#[given("a temporary directory")]
async fn given_temp_dir(w: &mut AlephWorld) {
    w.temp_dir = Some(tempdir().expect("Failed to create temp dir"));
}

#[then("the operation should succeed")]
async fn then_should_succeed(w: &mut AlephWorld) {
    match &w.last_result {
        Some(Ok(())) => {}
        Some(Err(e)) => panic!("Expected success, got error: {}", e),
        None => panic!("No operation result recorded"),
    }
}

#[then("the operation should fail")]
async fn then_should_fail(w: &mut AlephWorld) {
    match &w.last_result {
        Some(Err(_)) => {}
        Some(Ok(())) => panic!("Expected failure, but operation succeeded"),
        None => panic!("No operation result recorded"),
    }
}

#[then(expr = "the error message should contain {string}")]
async fn then_error_contains(w: &mut AlephWorld, expected: String) {
    let err = w.last_error.as_ref().expect("No error recorded");
    assert!(
        err.contains(&expected),
        "Error '{}' does not contain '{}'",
        err,
        expected
    );
}
```

### Step 5: Create cucumber test runner

Create `core/tests/cucumber.rs`:

```rust
//! Cucumber BDD Test Runner
//!
//! Run all tests: cargo test --test cucumber
//! Run specific feature: cargo test --test cucumber -- tests/features/config/
//! Run with tag: cargo test --test cucumber -- --tags @wip

mod world;
mod steps;

use cucumber::World;
use world::AlephWorld;

#[tokio::main]
async fn main() {
    AlephWorld::cucumber()
        .max_concurrent_scenarios(4)
        .run("tests/features")
        .await;
}
```

### Step 6: Create placeholder feature file

Create `core/tests/features/.gitkeep`:

```
# BDD Feature files organized by module
```

### Step 7: Verify infrastructure compiles

```bash
cd core && cargo test --test cucumber --no-run
```

Expected: Compilation succeeds (no features yet, so no tests run)

### Step 8: Commit infrastructure

```bash
git add core/Cargo.toml core/tests/cucumber.rs core/tests/world/ core/tests/steps/ core/tests/features/
git commit -m "feat(tests): add cucumber BDD infrastructure"
```

---

## Task 1: Batch 1a - Scripting Engine Tests

**Files:**
- Create: `core/tests/features/scripting/engine.feature`
- Create: `core/tests/steps/scripting_steps.rs`
- Modify: `core/tests/steps/mod.rs`
- Modify: `core/tests/world/mod.rs`
- Delete: `core/tests/scripting_engine_test.rs` (after verification)

### Step 1: Add ScriptingContext to World

Update `core/tests/world/mod.rs`:

```rust
//! BDD Test World - Shared state between cucumber steps

use cucumber::World;
use tempfile::TempDir;
use rhai::{Engine, AST};

/// Scripting engine test context
#[derive(Debug, Default)]
pub struct ScriptingContext {
    pub engine: Option<Engine>,
    pub compile_result: Option<Result<AST, String>>,
    pub eval_result: Option<Result<i64, String>>,
}

#[derive(Debug, Default, World)]
pub struct AlephWorld {
    // ═══ Common State ═══
    pub temp_dir: Option<TempDir>,
    pub last_result: Option<Result<(), String>>,
    pub last_error: Option<String>,

    // ═══ Module Contexts ═══
    pub scripting: Option<ScriptingContext>,
}
```

### Step 2: Create scripting feature file

Create `core/tests/features/scripting/engine.feature`:

```gherkin
Feature: Sandboxed Scripting Engine
  As a system administrator
  I want a secure scripting environment
  So that user scripts cannot harm the host system

  Background:
    Given a sandboxed scripting engine

  Scenario: Reject eval operation
    When I try to compile a script containing "eval"
    Then the compilation should fail

  Scenario: Reject infinite loops
    When I try to compile a script containing "while true { }"
    Then the compilation should fail

  Scenario: Accept simple arithmetic
    When I compile the script "1 + 1"
    Then the compilation should succeed

  Scenario: Accept filter/map chains
    When I compile the script "[1, 2, 3].filter(|x| x > 1)"
    Then the compilation should succeed

  Scenario: Enforce operation limits
    When I evaluate the script "(1..10000).map(|x| x * x).sum()"
    Then the evaluation should fail
```

### Step 3: Create scripting steps

Create `core/tests/steps/scripting_steps.rs`:

```rust
//! Step definitions for scripting engine features

use cucumber::{given, when, then};
use crate::world::{AlephWorld, ScriptingContext};
use alephcore::daemon::dispatcher::scripting::create_sandboxed_engine;

#[given("a sandboxed scripting engine")]
async fn given_sandboxed_engine(w: &mut AlephWorld) {
    let engine = create_sandboxed_engine();
    w.scripting = Some(ScriptingContext {
        engine: Some(engine),
        compile_result: None,
        eval_result: None,
    });
}

#[when(expr = "I try to compile a script containing {string}")]
async fn when_compile_containing(w: &mut AlephWorld, content: String) {
    let ctx = w.scripting.as_mut().expect("Scripting context not initialized");
    let engine = ctx.engine.as_ref().expect("Engine not initialized");

    let script = if content == "eval" {
        "eval(\"malicious\")"
    } else {
        &content
    };

    ctx.compile_result = Some(
        engine
            .compile(script)
            .map_err(|e| e.to_string())
    );
}

#[when(expr = "I compile the script {string}")]
async fn when_compile_script(w: &mut AlephWorld, script: String) {
    let ctx = w.scripting.as_mut().expect("Scripting context not initialized");
    let engine = ctx.engine.as_ref().expect("Engine not initialized");

    ctx.compile_result = Some(
        engine
            .compile(&script)
            .map_err(|e| e.to_string())
    );
}

#[when(expr = "I evaluate the script {string}")]
async fn when_eval_script(w: &mut AlephWorld, script: String) {
    let ctx = w.scripting.as_mut().expect("Scripting context not initialized");
    let engine = ctx.engine.as_ref().expect("Engine not initialized");

    let result: Result<i64, _> = engine.eval(&script);
    ctx.eval_result = Some(result.map_err(|e| e.to_string()));
}

#[then("the compilation should fail")]
async fn then_compile_fails(w: &mut AlephWorld) {
    let ctx = w.scripting.as_ref().expect("Scripting context not initialized");
    let result = ctx.compile_result.as_ref().expect("No compilation attempted");
    assert!(result.is_err(), "Expected compilation to fail, but it succeeded");
}

#[then("the compilation should succeed")]
async fn then_compile_succeeds(w: &mut AlephWorld) {
    let ctx = w.scripting.as_ref().expect("Scripting context not initialized");
    let result = ctx.compile_result.as_ref().expect("No compilation attempted");
    assert!(result.is_ok(), "Expected compilation to succeed, got: {:?}", result);
}

#[then("the evaluation should fail")]
async fn then_eval_fails(w: &mut AlephWorld) {
    let ctx = w.scripting.as_ref().expect("Scripting context not initialized");
    let result = ctx.eval_result.as_ref().expect("No evaluation attempted");
    assert!(result.is_err(), "Expected evaluation to fail due to limits");
}
```

### Step 4: Register scripting steps

Update `core/tests/steps/mod.rs`:

```rust
//! BDD Step Definitions

mod common;
mod scripting_steps;

pub use common::*;
pub use scripting_steps::*;
```

### Step 5: Run the BDD tests

```bash
cd core && cargo test --test cucumber -- tests/features/scripting/
```

Expected: All 5 scenarios pass

### Step 6: Delete old test file

```bash
rm core/tests/scripting_engine_test.rs
```

### Step 7: Verify full test suite still passes

```bash
cd core && cargo test
```

### Step 8: Commit Batch 1a

```bash
git add -A
git commit -m "feat(tests): migrate scripting engine tests to BDD"
```

---

## Task 2: Batch 1b - Config Basic Tests

**Files:**
- Create: `core/tests/features/config/basic.feature`
- Create: `core/tests/steps/config_steps.rs`
- Create: `core/tests/world/config_ctx.rs`
- Modify: `core/tests/steps/mod.rs`
- Modify: `core/tests/world/mod.rs`

### Step 1: Create ConfigContext

Create `core/tests/world/config_ctx.rs`:

```rust
//! Configuration test context

use alephcore::config::{Config, MemoryConfig, BehaviorConfig, ShortcutsConfig};

#[derive(Debug, Default)]
pub struct ConfigContext {
    pub config: Option<Config>,
    pub memory_config: Option<MemoryConfig>,
    pub behavior_config: Option<BehaviorConfig>,
    pub shortcuts_config: Option<ShortcutsConfig>,
    pub validation_result: Option<Result<(), String>>,
    pub parse_result: Option<Result<Config, String>>,
}
```

### Step 2: Update World to include ConfigContext

Update `core/tests/world/mod.rs`:

```rust
//! BDD Test World - Shared state between cucumber steps

use cucumber::World;
use tempfile::TempDir;
use rhai::{Engine, AST};

mod config_ctx;

pub use config_ctx::ConfigContext;

/// Scripting engine test context
#[derive(Debug, Default)]
pub struct ScriptingContext {
    pub engine: Option<Engine>,
    pub compile_result: Option<Result<AST, String>>,
    pub eval_result: Option<Result<i64, String>>,
}

#[derive(Debug, Default, World)]
pub struct AlephWorld {
    // ═══ Common State ═══
    pub temp_dir: Option<TempDir>,
    pub last_result: Option<Result<(), String>>,
    pub last_error: Option<String>,

    // ═══ Module Contexts ═══
    pub scripting: Option<ScriptingContext>,
    pub config: Option<ConfigContext>,
}
```

### Step 3: Create config basic feature file

Create `core/tests/features/config/basic.feature`:

```gherkin
Feature: Basic Configuration
  As a user
  I want sensible default configurations
  So that the system works out of the box

  Scenario: Default config has expected hotkey
    Given a default Config
    Then the default_hotkey should be "Grave"
    And memory should be enabled

  Scenario: New config matches default
    Given a new Config
    Then the default_hotkey should be "Grave"

  Scenario: Memory config has expected defaults
    Given a default MemoryConfig
    Then memory should be enabled
    And the embedding_model should be "bge-small-zh-v1.5"
    And max_context_items should be 5
    And retention_days should be 90
    And vector_db should be "sqlite-vec"
    And similarity_threshold should be 0.7
    And dreaming should be enabled
    And dreaming window_start should be "02:00"
    And dreaming window_end should be "05:00"

  Scenario: Default excluded apps include security apps
    Given a default MemoryConfig
    Then excluded_apps should contain "com.apple.keychainaccess"
    And excluded_apps should contain "com.agilebits.onepassword7"

  Scenario: Shortcuts config has expected defaults
    Given a default ShortcutsConfig
    Then the summon shortcut should be "Command+Grave"
    And the cancel shortcut should be "Escape"

  Scenario: Behavior config has expected defaults
    Given a default BehaviorConfig
    Then the output_mode should be "typewriter"
    And typing_speed should be 50

  Scenario: Minimal config with provider passes validation
    Given a config parsed from:
      """
      [providers.openai]
      api_key = "sk-test"
      model = "gpt-4o"

      [general]
      default_provider = "openai"
      """
    Then the config should be valid
    And the default_hotkey should be "Grave"
    And smart_flow should be enabled
    And memory should be enabled
```

### Step 4: Create config steps

Create `core/tests/steps/config_steps.rs`:

```rust
//! Step definitions for configuration features

use cucumber::{given, then};
use crate::world::{AlephWorld, ConfigContext};
use alephcore::config::{Config, MemoryConfig, BehaviorConfig, ShortcutsConfig};

// ═══ Given Steps ═══

#[given("a default Config")]
async fn given_default_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.config = Some(Config::default());
}

#[given("a new Config")]
async fn given_new_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.config = Some(Config::new());
}

#[given("a default MemoryConfig")]
async fn given_default_memory_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.memory_config = Some(MemoryConfig::default());
}

#[given("a default ShortcutsConfig")]
async fn given_default_shortcuts_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.shortcuts_config = Some(ShortcutsConfig::default());
}

#[given("a default BehaviorConfig")]
async fn given_default_behavior_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.behavior_config = Some(BehaviorConfig::default());
}

#[given(expr = "a config parsed from:")]
async fn given_config_from_toml(w: &mut AlephWorld, toml_str: String) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.parse_result = Some(
        toml::from_str::<Config>(&toml_str).map_err(|e| e.to_string())
    );
    if let Some(Ok(ref config)) = ctx.parse_result {
        ctx.config = Some(config.clone());
    }
}

// ═══ Then Steps - Config ═══

#[then(expr = "the default_hotkey should be {string}")]
async fn then_default_hotkey(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert_eq!(config.default_hotkey, expected);
}

#[then("memory should be enabled")]
async fn then_memory_enabled(w: &mut AlephWorld) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    if let Some(ref config) = ctx.config {
        assert!(config.memory.enabled);
    } else if let Some(ref mem_config) = ctx.memory_config {
        assert!(mem_config.enabled);
    } else {
        panic!("No config or memory_config initialized");
    }
}

#[then("the config should be valid")]
async fn then_config_valid(w: &mut AlephWorld) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    let result = config.validate();
    assert!(result.is_ok(), "Config validation failed: {:?}", result);
}

#[then("smart_flow should be enabled")]
async fn then_smart_flow_enabled(w: &mut AlephWorld) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(config.smart_flow.enabled);
}

// ═══ Then Steps - MemoryConfig ═══

#[then(expr = "the embedding_model should be {string}")]
async fn then_embedding_model(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.embedding_model, expected);
}

#[then(expr = "max_context_items should be {int}")]
async fn then_max_context_items(w: &mut AlephWorld, expected: i32) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.max_context_items, expected as usize);
}

#[then(expr = "retention_days should be {int}")]
async fn then_retention_days(w: &mut AlephWorld, expected: i32) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.retention_days, expected as u32);
}

#[then(expr = "vector_db should be {string}")]
async fn then_vector_db(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.vector_db, expected);
}

#[then(expr = "similarity_threshold should be {float}")]
async fn then_similarity_threshold(w: &mut AlephWorld, expected: f32) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert!((mem.similarity_threshold - expected as f64).abs() < 0.001);
}

#[then("dreaming should be enabled")]
async fn then_dreaming_enabled(w: &mut AlephWorld) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert!(mem.dreaming.enabled);
}

#[then(expr = "dreaming window_start should be {string}")]
async fn then_dreaming_start(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.dreaming.window_start_local, expected);
}

#[then(expr = "dreaming window_end should be {string}")]
async fn then_dreaming_end(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.dreaming.window_end_local, expected);
}

#[then(expr = "excluded_apps should contain {string}")]
async fn then_excluded_apps_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert!(
        mem.excluded_apps.contains(&expected),
        "excluded_apps {:?} does not contain '{}'",
        mem.excluded_apps,
        expected
    );
}

// ═══ Then Steps - ShortcutsConfig ═══

#[then(expr = "the summon shortcut should be {string}")]
async fn then_summon_shortcut(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let shortcuts = ctx.shortcuts_config.as_ref().expect("ShortcutsConfig not initialized");
    assert_eq!(shortcuts.summon, expected);
}

#[then(expr = "the cancel shortcut should be {string}")]
async fn then_cancel_shortcut(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let shortcuts = ctx.shortcuts_config.as_ref().expect("ShortcutsConfig not initialized");
    assert_eq!(shortcuts.cancel, Some(expected));
}

// ═══ Then Steps - BehaviorConfig ═══

#[then(expr = "the output_mode should be {string}")]
async fn then_output_mode(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let behavior = ctx.behavior_config.as_ref().expect("BehaviorConfig not initialized");
    assert_eq!(behavior.output_mode, expected);
}

#[then(expr = "typing_speed should be {int}")]
async fn then_typing_speed(w: &mut AlephWorld, expected: i32) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let behavior = ctx.behavior_config.as_ref().expect("BehaviorConfig not initialized");
    assert_eq!(behavior.typing_speed, expected as u32);
}
```

### Step 5: Register config steps

Update `core/tests/steps/mod.rs`:

```rust
//! BDD Step Definitions

mod common;
mod scripting_steps;
mod config_steps;

pub use common::*;
pub use scripting_steps::*;
pub use config_steps::*;
```

### Step 6: Run the BDD tests

```bash
cd core && cargo test --test cucumber -- tests/features/config/basic.feature
```

Expected: All 7 scenarios pass

### Step 7: Commit Batch 1b

```bash
git add -A
git commit -m "feat(tests): migrate config basic tests to BDD"
```

---

## Task 3: Batch 1c - Config Validation Tests

**Files:**
- Create: `core/tests/features/config/validation.feature`
- Modify: `core/tests/steps/config_steps.rs`

### Step 1: Create validation feature file

Create `core/tests/features/config/validation.feature`:

```gherkin
Feature: Configuration Validation
  As a system administrator
  I want configuration validation
  So that invalid configs are rejected before runtime

  # ═══ Valid Configurations ═══

  Scenario: Valid config with provider passes validation
    Given a config with a valid openai provider
    Then the config should be valid

  # ═══ Provider Validation ═══

  Scenario: Missing default provider fails validation
    Given a config with default_provider "nonexistent"
    Then the config should be invalid

  Scenario: Provider without API key fails validation
    Given a config with openai provider without api_key
    Then the config should be invalid

  Scenario: Invalid temperature fails validation
    Given a config with openai provider with temperature 3.0
    Then the config should be invalid

  Scenario: Zero timeout fails validation
    Given a config with openai provider with timeout 0
    Then the config should be invalid
    And the error should contain "timeout must be greater than 0"

  Scenario: Ollama provider without API key passes validation
    Given a config with ollama provider without api_key
    Then the config should be valid

  # ═══ Routing Rules Validation ═══

  Scenario: Invalid regex in rule fails validation
    Given a config with a valid openai provider
    And a routing rule with regex "[invalid("
    Then the config should be invalid

  Scenario: Rule referencing unknown provider fails validation
    Given a config with a routing rule referencing "nonexistent" provider
    Then the config should be invalid

  Scenario Outline: Valid regex patterns pass validation
    Given a config with a valid openai provider
    And a routing rule with regex "<pattern>"
    Then the config should be valid

    Examples:
      | pattern              |
      | .*                   |
      | ^/code               |
      | \\d+                 |
      | hello\|world         |
      | [a-zA-Z]+            |
      | ^test$               |

  Scenario Outline: Invalid regex patterns fail validation
    Given a config with a valid openai provider
    And a routing rule with regex "<pattern>"
    Then the config should be invalid

    Examples:
      | pattern      |
      | [invalid(    |
      | (unclosed    |
      | **           |
      | [z-a]        |

  # ═══ Memory Config Validation ═══

  Scenario: Zero max_context_items fails validation
    Given a config with memory max_context_items 0
    Then the config should be invalid
    And the error should contain "max_context_items must be greater than 0"

  Scenario: Invalid similarity threshold fails validation
    Given a config with memory similarity_threshold 1.5
    Then the config should be invalid
    And the error should contain "similarity_threshold must be between 0.0 and 1.0"

  Scenario: Invalid dreaming window fails validation
    Given a config with dreaming window_start "25:00"
    Then the config should be invalid
    And the error should contain "window_start_local must be HH:MM"

  Scenario: Invalid graph decay fails validation
    Given a config with graph_decay node_decay_per_day 1.5
    Then the config should be invalid
    And the error should contain "graph_decay.node_decay_per_day"
```

### Step 2: Add validation steps to config_steps.rs

Append to `core/tests/steps/config_steps.rs`:

```rust
// ═══ Given Steps - Validation Scenarios ═══

#[given("a config with a valid openai provider")]
async fn given_config_with_valid_provider(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let provider = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider);
    config.general.default_provider = Some("openai".to_string());
    ctx.config = Some(config);
}

#[given(expr = "a config with default_provider {string}")]
async fn given_config_with_default_provider(w: &mut AlephWorld, provider_name: String) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.general.default_provider = Some(provider_name);
    ctx.config = Some(config);
}

#[given("a config with openai provider without api_key")]
async fn given_config_without_api_key(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.api_key = None;
    config.providers.insert("openai".to_string(), provider);
    ctx.config = Some(config);
}

#[given(expr = "a config with openai provider with temperature {float}")]
async fn given_config_with_temperature(w: &mut AlephWorld, temp: f32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.temperature = Some(temp as f64);
    config.providers.insert("openai".to_string(), provider);
    ctx.config = Some(config);
}

#[given(expr = "a config with openai provider with timeout {int}")]
async fn given_config_with_timeout(w: &mut AlephWorld, timeout: i32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.timeout_seconds = timeout as u64;
    config.providers.insert("openai".to_string(), provider);
    ctx.config = Some(config);
}

#[given("a config with ollama provider without api_key")]
async fn given_ollama_without_api_key(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("llama3.2");
    provider.api_key = None;
    provider.protocol = Some("ollama".to_string());
    config.providers.insert("ollama".to_string(), provider);
    ctx.config = Some(config);
}

#[given(expr = "a routing rule with regex {string}")]
async fn given_routing_rule_regex(w: &mut AlephWorld, pattern: String) {
    let ctx = w.config.as_mut().expect("Config context not initialized");
    let config = ctx.config.as_mut().expect("Config not initialized");
    let mut rule = RoutingRuleConfig::command(&pattern, "openai", None);
    rule.regex = pattern;
    config.rules.push(rule);
}

#[given(expr = "a config with a routing rule referencing {string} provider")]
async fn given_rule_unknown_provider(w: &mut AlephWorld, provider_name: String) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.rules.push(RoutingRuleConfig::command(".*", &provider_name, None));
    ctx.config = Some(config);
}

#[given(expr = "a config with memory max_context_items {int}")]
async fn given_memory_max_context(w: &mut AlephWorld, value: i32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.memory.max_context_items = value as usize;
    ctx.config = Some(config);
}

#[given(expr = "a config with memory similarity_threshold {float}")]
async fn given_memory_similarity(w: &mut AlephWorld, value: f32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.memory.similarity_threshold = value as f64;
    ctx.config = Some(config);
}

#[given(expr = "a config with dreaming window_start {string}")]
async fn given_dreaming_start(w: &mut AlephWorld, value: String) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.memory.dreaming.window_start_local = value;
    ctx.config = Some(config);
}

#[given(expr = "a config with graph_decay node_decay_per_day {float}")]
async fn given_graph_decay(w: &mut AlephWorld, value: f32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.memory.graph_decay.node_decay_per_day = value as f64;
    ctx.config = Some(config);
}

// ═══ Then Steps - Validation ═══

#[then("the config should be invalid")]
async fn then_config_invalid(w: &mut AlephWorld) {
    let ctx = w.config.as_mut().expect("Config context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    let result = config.validate();
    ctx.validation_result = Some(result.clone().map_err(|e| e.to_string()));
    assert!(result.is_err(), "Expected config to be invalid, but it was valid");
}

#[then(expr = "the error should contain {string}")]
async fn then_error_should_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let result = ctx.validation_result.as_ref().expect("No validation performed");
    match result {
        Err(e) => assert!(
            e.contains(&expected),
            "Error '{}' does not contain '{}'",
            e,
            expected
        ),
        Ok(()) => panic!("Expected error but validation passed"),
    }
}
```

### Step 3: Add imports to config_steps.rs

Ensure these imports are at the top of `core/tests/steps/config_steps.rs`:

```rust
use alephcore::config::{Config, MemoryConfig, BehaviorConfig, ShortcutsConfig, ProviderConfig, RoutingRuleConfig};
```

### Step 4: Run the validation tests

```bash
cd core && cargo test --test cucumber -- tests/features/config/validation.feature
```

Expected: All scenarios pass

### Step 5: Delete old config test files

After verifying all tests pass:

```bash
rm core/src/config/tests/basic.rs
rm core/src/config/tests/validation.rs
```

Update `core/src/config/tests/mod.rs` to remove references to deleted modules.

### Step 6: Verify full test suite

```bash
cd core && cargo test
```

### Step 7: Commit Batch 1c

```bash
git add -A
git commit -m "feat(tests): migrate config validation tests to BDD"
```

---

## Subsequent Batches

The remaining batches follow the same pattern:

### Batch 2: Daemon + Perception (Tasks 4-5)
- Create `features/daemon/*.feature`
- Create `features/perception/*.feature`
- Create `DaemonContext`, `PerceptionContext`
- Create step definitions
- Delete old test files

### Batch 3: Agent Loop + POE + Thinker (Tasks 6-8)
- Create `features/agent_loop/*.feature`
- Create `features/poe/*.feature`
- Create `features/thinker/*.feature`
- Create corresponding contexts and steps

### Batch 4: Memory + Dispatcher (Tasks 9-10)
- Create `features/memory/*.feature`
- Create `features/dispatcher/*.feature`

### Batch 5: Gateway + Tools + Extension (Tasks 11-13)
- Create `features/gateway/*.feature`
- Create `features/tools/*.feature`
- Create `features/extension/*.feature`

### Batch 6: Top-level Integration Tests (Tasks 14-17)
- Create `features/integration/*.feature`
- Migrate 24 integration test files

---

## Summary

| Batch | Tasks | Estimated Scenarios |
|-------|-------|---------------------|
| 0 | Infrastructure | - |
| 1a | Scripting | 5 |
| 1b | Config Basic | 7 |
| 1c | Config Validation | ~20 |
| 2 | Daemon + Perception | ~45 |
| 3 | Agent + POE + Thinker | ~65 |
| 4 | Memory + Dispatcher | ~25 |
| 5 | Gateway + Tools + Extension | ~45 |
| 6 | Integration | ~60 |

**Total:** ~270 scenarios across ~40 feature files

---

## Best Practices & Lessons Learned

### 1. Permit Management in Concurrency Tests

**Context**: When testing low-level concurrency primitives (semaphores, mutexes), test steps may need to hold resources across step boundaries.

**Problem**: Using `std::mem::forget()` without cleanup causes memory leaks.

**Solution**:
```rust
// In "acquire" step:
if let Some(permit) = semaphore.try_acquire_permit() {
    std::mem::forget(permit);  // Intentional for testing - document clearly
}

// In "release" step:
semaphore.add_permits(1);  // Explicit cleanup
```

**Guidelines**:
- Document intentional `std::mem::forget` usage with clear comments
- Always provide explicit cleanup in corresponding steps
- Consider if higher-level API testing can avoid this pattern
- Avoid unsafe code (`transmute`) for lifetime extension

**Example**: `core/tests/steps/scheduler_steps.rs` (Phase 2, Task 5)

### 2. Time-Based Testing

**Context**: Testing anti-starvation, timeouts, rate limiting, or other time-sensitive behavior.

**Problem**: Placeholder implementations that don't actually wait lead to false positives.

**Bad**:
```rust
#[when("I wait for timeout")]
async fn when_wait_for_timeout(_w: &mut AlephWorld) {
    // TODO: Implement actual waiting
}
```

**Good**:
```rust
#[when("I wait for timeout")]
async fn when_wait_for_timeout(w: &mut AlephWorld) {
    let timeout_ms = w.config.timeout_threshold_ms;
    tokio::time::sleep(Duration::from_millis(timeout_ms + 100)).await;
}
```

**Guidelines**:
- Use `tokio::time::sleep` for real delays in async tests
- Consider `tokio::time::pause()` + `advance()` for faster tests
- Read thresholds from configuration, don't hardcode
- Add small buffer (e.g., +100ms) to ensure threshold is crossed

**Example**: `core/tests/steps/scheduler_steps.rs::when_wait_for_anti_starvation_conditions`

### 3. Validation in When Steps

**Context**: When steps that trigger state changes (spawn, create, register).

**Problem**: Blindly executing operations without validation can miss constraint violations.

**Bad**:
```rust
#[when(expr = "I spawn child {string} from parent {string}")]
async fn when_spawn_child(w: &mut AlephWorld, child: String, parent: String) {
    w.registry.spawn(&parent, &child).await;  // No validation
}
```

**Good**:
```rust
#[when(expr = "I spawn child {string} from parent {string}")]
async fn when_spawn_child(w: &mut AlephWorld, child: String, parent: String) {
    let result = w.registry.check_can_spawn(&parent).await;

    if result.is_ok() {
        w.registry.spawn(&parent, &child).await;
        w.last_result = Some(Ok(()));
    } else {
        w.last_result = Some(result.map_err(|e| e.to_string()));
    }
}
```

**Guidelines**:
- Validate preconditions before executing state changes
- Store validation results for Then assertions
- Distinguish between "operation attempted" vs "operation succeeded"
- Use separate steps for "should succeed" vs "should fail" scenarios

**Example**: `core/tests/steps/scheduler_steps.rs::when_spawn_child_from_parent`

### 4. Test Scenario Design

**Context**: Identifying duplicate vs complementary test scenarios.

**Problem**: Removing "duplicate" scenarios that actually test different aspects.

**Analysis Checklist**:
- Do scenarios test the same inputs? (If no → likely complementary)
- Do scenarios assert different outcomes? (If yes → likely complementary)
- Do scenarios cover different edge cases? (If yes → likely complementary)
- Do scenarios test different code paths? (If yes → likely complementary)

**Example**: Phase 2 scheduler tests had two "priority" scenarios:
- Scenario A: Tests basic priority with specific run IDs
- Scenario B: Tests priority across all 4 lanes (Main, Nested, Subagent, Cron)

**Verdict**: Complementary, not duplicate. Both provide value.

**Guidelines**:
- Analyze coverage goals before removing tests
- Document scenario intent in feature file comments
- Use tags to group related scenarios
- Prefer more coverage over less when in doubt

### 5. Context Field Management

**Context**: Managing state in World context structs.

**Problem**: Accumulating unused fields as tests evolve.

**Guidelines**:
- Remove fields that are no longer used
- Use `Option<T>` for lazily-initialized state
- Group related fields into sub-contexts
- Document field purpose in comments
- Run `cargo fix` to catch unused field warnings

**Example**: Removed `held_permits_count` from `SchedulerContext` after redesigning permit management.

### 6. Async Step Definitions

**Context**: All step definitions in cucumber-rs must be async.

**Guidelines**:
- Always use `async fn` for step definitions
- Use `tokio::time::sleep` instead of `std::thread::sleep`
- Await all async operations
- Use `tokio::spawn` for background tasks
- Clean up background tasks in cleanup steps

### 7. Error Message Quality

**Context**: Assertion failures in Then steps.

**Bad**:
```rust
assert!(result.is_ok());
```

**Good**:
```rust
assert!(
    result.is_ok(),
    "Expected operation to succeed, but got error: {:?}",
    result.unwrap_err()
);
```

**Guidelines**:
- Always provide context in assertion messages
- Include actual vs expected values
- Use `{:?}` for debug formatting
- Make failures actionable

### 8. Code Quality Review Process

**Context**: Ensuring BDD test quality before merging.

**Process**:
1. **Implementation**: Write feature + steps + context
2. **Spec Review**: Verify scenarios match requirements
3. **Code Quality Review**: Check for common issues:
   - Memory leaks (forgotten resources)
   - Placeholder implementations
   - Missing validations
   - Duplicate scenarios
   - Unused context fields
4. **Fix Issues**: Address all critical and important findings
5. **Re-test**: Verify all tests pass after fixes
6. **Document**: Update plan with lessons learned

**Example**: Phase 2 Task 5 underwent this process, resulting in 4 critical fixes.

---

## References

- [Cucumber Best Practices](https://cucumber.io/docs/bdd/better-gherkin/)
- [cucumber-rs Documentation](https://docs.rs/cucumber/)
- [Tokio Testing Guide](https://tokio.rs/tokio/topics/testing)
- Phase 2 Implementation: `docs/plans/2026-02-05-multi-agent-2.0-phase2-impl.md`
