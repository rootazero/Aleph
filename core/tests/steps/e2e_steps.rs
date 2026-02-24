//! Step definitions for E2E features (Evolution, YAML Policies)

use crate::world::{AlephWorld, BatchLoadResult, E2eContext};
use alephcore::daemon::dispatcher::policy::{ActionType, PolicyEngine};
use alephcore::daemon::events::{DerivedEvent, PressureLevel, PressureType};
use alephcore::daemon::worldmodel::state::{
    ActivityType, EnhancedContext, MemoryPressure, SystemLoad,
};
use alephcore::skill_evolution::types::{ExecutionStatus, SkillExecution, SolidificationConfig};
use alephcore::skill_evolution::{EvolutionTracker, SolidificationPipeline};
use alephcore::tools::markdown_skill::{EvolutionAutoLoader, MarkdownSkillGeneratorConfig};
use alephcore::tools::AlephToolServer;
use chrono::Utc;
use cucumber::{given, then, when};
use std::sync::Arc;
use tempfile::TempDir;

// ═══ Evolution Setup Steps ═══

#[given("an in-memory evolution tracker")]
async fn given_evolution_tracker(w: &mut AlephWorld) {
    let ctx = w.e2e.get_or_insert_with(E2eContext::default);
    ctx.tracker = Some(Arc::new(EvolutionTracker::in_memory().unwrap()));
}

#[given("an evolution auto-loader with temp output directory")]
async fn given_auto_loader(w: &mut AlephWorld) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");
    let temp_dir = TempDir::new().unwrap();

    let config = MarkdownSkillGeneratorConfig {
        output_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };

    let tool_server = ctx
        .tool_server
        .clone()
        .expect("Tool server not initialized");
    ctx.auto_loader = Some(Arc::new(EvolutionAutoLoader::with_config(
        tool_server,
        config,
    )));
    ctx.temp_dir = Some(temp_dir);
}

// Note: "an empty tool server" is defined in security_steps.rs, reuse pattern:
#[given("an empty tool server for evolution")]
async fn given_empty_tool_server_e2e(w: &mut AlephWorld) {
    let ctx = w.e2e.get_or_insert_with(E2eContext::default);
    ctx.tool_server = Some(Arc::new(AlephToolServer::new()));
}

// ═══ Evolution Execution Steps ═══

#[when(expr = "I log {int} successful executions for skill {string} across {int} sessions")]
async fn when_log_executions(w: &mut AlephWorld, count: i32, skill_id: String, sessions: i32) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let tracker = ctx.tracker.as_ref().expect("Tracker not initialized");

    for i in 0..count {
        let execution = SkillExecution {
            id: format!("exec-{}", i),
            skill_id: skill_id.clone(),
            session_id: format!("session-{}", i % sessions),
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
}

#[when(expr = "I log {int} successful executions for {int} different patterns")]
async fn when_log_multiple_patterns(w: &mut AlephWorld, exec_per_pattern: i32, pattern_count: i32) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let tracker = ctx.tracker.as_ref().expect("Tracker not initialized");

    for pattern_num in 0..pattern_count {
        for i in 0..exec_per_pattern {
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
}

#[when("I run the solidification pipeline with low thresholds")]
async fn when_run_solidification(w: &mut AlephWorld) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");
    let tracker = ctx.tracker.clone().expect("Tracker not initialized");

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
    ctx.solidification_result = Some(result);
}

#[when("I auto-load the first suggestion")]
async fn when_auto_load_first(w: &mut AlephWorld) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");
    let auto_loader = ctx
        .auto_loader
        .clone()
        .expect("Auto-loader not initialized");
    let result = ctx
        .solidification_result
        .as_ref()
        .expect("No solidification result");
    let suggestion = &result.suggestions[0];

    // Store for later verification
    ctx.current_suggestion = Some(suggestion.clone());

    // Calculate the expected skill name
    let skill_name = suggestion
        .suggested_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    ctx.generated_skill_name = Some(skill_name);

    let loaded_count = auto_loader.load_from_suggestion(suggestion).await.unwrap();
    ctx.loaded_count = loaded_count;
}

#[when("I auto-load the same suggestion again")]
async fn when_auto_load_again(w: &mut AlephWorld) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");
    let auto_loader = ctx
        .auto_loader
        .clone()
        .expect("Auto-loader not initialized");
    let suggestion = ctx
        .current_suggestion
        .as_ref()
        .expect("No current suggestion");

    let loaded_count = auto_loader.load_from_suggestion(suggestion).await.unwrap();
    ctx.loaded_count = loaded_count;
}

#[when("I batch auto-load all suggestions")]
async fn when_batch_auto_load(w: &mut AlephWorld) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");
    let auto_loader = ctx
        .auto_loader
        .clone()
        .expect("Auto-loader not initialized");
    let result = ctx
        .solidification_result
        .as_ref()
        .expect("No solidification result");

    let batch_result = auto_loader.load_batch(&result.suggestions).await.unwrap();
    ctx.batch_result = Some(BatchLoadResult {
        total: batch_result.total,
        loaded: batch_result.loaded,
        failed: batch_result.failed,
    });
}

// ═══ Evolution Assertion Steps ═══

#[then("suggestions should be generated")]
async fn then_suggestions_generated(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let result = ctx
        .solidification_result
        .as_ref()
        .expect("No solidification result");
    assert!(
        !result.suggestions.is_empty(),
        "Pipeline should generate suggestions"
    );
}

#[then(expr = "the candidate count should be {int}")]
async fn then_candidate_count(w: &mut AlephWorld, expected: i32) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let result = ctx
        .solidification_result
        .as_ref()
        .expect("No solidification result");
    assert_eq!(
        result.candidates_detected, expected as usize,
        "Candidate count mismatch"
    );
}

#[then(expr = "exactly {int} tool should be loaded")]
async fn then_tool_loaded_count(w: &mut AlephWorld, expected: i32) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    assert_eq!(ctx.loaded_count, expected as usize, "Loaded count mismatch");
}

#[then("the tool should be registered in the tool server")]
async fn then_tool_registered(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let tool_server = ctx
        .tool_server
        .as_ref()
        .expect("Tool server not initialized");
    let skill_name = ctx
        .generated_skill_name
        .as_ref()
        .expect("No generated skill name");

    assert!(
        tool_server.has_tool(skill_name).await,
        "Tool '{}' should be registered in ToolServer",
        skill_name
    );
}

#[then("the tool should have a description")]
async fn then_tool_has_description(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let tool_server = ctx
        .tool_server
        .as_ref()
        .expect("Tool server not initialized");
    let skill_name = ctx
        .generated_skill_name
        .as_ref()
        .expect("No generated skill name");

    let definition = tool_server.get_definition(skill_name).await.unwrap();
    assert!(
        !definition.description.is_empty(),
        "Tool should have description"
    );
}

#[then("the tool should have a parameters schema")]
async fn then_tool_has_params(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let tool_server = ctx
        .tool_server
        .as_ref()
        .expect("Tool server not initialized");
    let skill_name = ctx
        .generated_skill_name
        .as_ref()
        .expect("No generated skill name");

    let definition = tool_server.get_definition(skill_name).await.unwrap();
    assert!(
        definition.parameters.is_object(),
        "Tool should have parameters schema"
    );
}

#[then("the tool should have LLM context")]
async fn then_tool_has_llm_context(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let tool_server = ctx
        .tool_server
        .as_ref()
        .expect("Tool server not initialized");
    let skill_name = ctx
        .generated_skill_name
        .as_ref()
        .expect("No generated skill name");

    let definition = tool_server.get_definition(skill_name).await.unwrap();
    assert!(
        definition.llm_context.is_some(),
        "Tool should have LLM context"
    );
}

#[then(expr = "the generated skills count should be {int}")]
async fn then_generated_skills_count(w: &mut AlephWorld, expected: i32) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let auto_loader = ctx
        .auto_loader
        .as_ref()
        .expect("Auto-loader not initialized");
    let generated = auto_loader.get_generated_skills();
    assert_eq!(
        generated.len(),
        expected as usize,
        "Generated skills count mismatch"
    );
}

#[then(expr = "{int} suggestions should be generated")]
async fn then_n_suggestions(w: &mut AlephWorld, expected: i32) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let result = ctx
        .solidification_result
        .as_ref()
        .expect("No solidification result");
    assert_eq!(
        result.suggestions.len(),
        expected as usize,
        "Suggestion count mismatch"
    );
}

#[then(expr = "the batch result should show {int} total and {int} loaded")]
async fn then_batch_result(w: &mut AlephWorld, total: i32, loaded: i32) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let batch_result = ctx.batch_result.as_ref().expect("No batch result");
    assert_eq!(batch_result.total, total as usize, "Total mismatch");
    assert_eq!(batch_result.loaded, loaded as usize, "Loaded mismatch");
}

#[then("the batch success rate should be 100%")]
async fn then_batch_success_100(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let batch_result = ctx.batch_result.as_ref().expect("No batch result");
    assert!(batch_result.all_succeeded(), "All should succeed");
    assert!(
        (batch_result.success_rate() - 1.0).abs() < 0.001,
        "Success rate should be 100%"
    );
}

#[then("all generated tools should be registered")]
async fn then_all_tools_registered(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let tool_server = ctx
        .tool_server
        .as_ref()
        .expect("Tool server not initialized");
    let result = ctx
        .solidification_result
        .as_ref()
        .expect("No solidification result");

    for suggestion in &result.suggestions {
        let skill_name = suggestion
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
            tool_server.has_tool(&skill_name).await,
            "Tool '{}' should be loaded",
            skill_name
        );
    }
}

#[then("the tool should still exist after reload")]
async fn then_tool_exists_after_reload(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let tool_server = ctx
        .tool_server
        .as_ref()
        .expect("Tool server not initialized");
    let skill_name = ctx
        .generated_skill_name
        .as_ref()
        .expect("No generated skill name");

    assert!(
        tool_server.has_tool(skill_name).await,
        "Tool '{}' should exist after reload",
        skill_name
    );
}

// ═══ Policy Setup Steps ═══

#[given("an MVP policy engine")]
async fn given_mvp_policy_engine(w: &mut AlephWorld) {
    let ctx = w.e2e.get_or_insert_with(E2eContext::default);
    ctx.policy_engine = Some(PolicyEngine::new_mvp());
}

#[then(expr = "the engine should have {int} policies")]
async fn then_engine_policy_count(w: &mut AlephWorld, expected: i32) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let engine = ctx
        .policy_engine
        .as_ref()
        .expect("Policy engine not initialized");
    assert_eq!(
        engine.policy_count(),
        expected as usize,
        "Policy count mismatch"
    );
}

#[given("a default enhanced context")]
async fn given_default_context(w: &mut AlephWorld) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");
    ctx.enhanced_context = Some(EnhancedContext::default());
}

#[given(expr = "an enhanced context with battery level {int}")]
async fn given_context_battery(w: &mut AlephWorld, battery: i32) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");
    let context = EnhancedContext {
        system_constraint: SystemLoad {
            cpu_usage: 0.0,
            memory_pressure: MemoryPressure::Normal,
            battery_level: Some(battery as u8),
        },
        ..EnhancedContext::default()
    };
    ctx.enhanced_context = Some(context);
}

#[given(expr = "an activity changed event from {string} to {string} with {int} participants")]
async fn given_activity_event(w: &mut AlephWorld, old: String, new: String, participants: i32) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");

    let old_activity = match old.as_str() {
        "Idle" => ActivityType::Idle,
        "Meeting" => ActivityType::Meeting { participants: 0 },
        _ => ActivityType::Unknown,
    };

    let new_activity = match new.as_str() {
        "Idle" => ActivityType::Idle,
        "Meeting" => ActivityType::Meeting {
            participants: participants as usize,
        },
        _ => ActivityType::Unknown,
    };

    ctx.derived_event = Some(DerivedEvent::ActivityChanged {
        timestamp: Utc::now(),
        old_activity,
        new_activity,
        confidence: 0.9,
    });
}

#[given(expr = "a resource pressure changed event for battery from {string} to {string}")]
async fn given_pressure_event(w: &mut AlephWorld, old: String, new: String) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");

    let old_level = match old.as_str() {
        "Normal" => PressureLevel::Normal,
        "Critical" => PressureLevel::Critical,
        _ => PressureLevel::Normal,
    };

    let new_level = match new.as_str() {
        "Normal" => PressureLevel::Normal,
        "Critical" => PressureLevel::Critical,
        _ => PressureLevel::Normal,
    };

    ctx.derived_event = Some(DerivedEvent::ResourcePressureChanged {
        timestamp: Utc::now(),
        pressure_type: PressureType::Battery,
        old_level,
        new_level,
    });
}

#[when("I evaluate all policies")]
async fn when_evaluate_policies(w: &mut AlephWorld) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");
    let engine = ctx
        .policy_engine
        .as_ref()
        .expect("Policy engine not initialized");
    let context = ctx.enhanced_context.as_ref().expect("Context not set");
    let event = ctx.derived_event.as_ref().expect("Event not set");

    let actions = engine.evaluate_all(context, event);
    ctx.triggered_actions = actions;
}

#[then("actions should be triggered")]
async fn then_actions_triggered(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    assert!(
        !ctx.triggered_actions.is_empty(),
        "Actions should be triggered"
    );
}

#[then("one action should be MuteSystemAudio")]
async fn then_mute_action(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    assert!(
        ctx.triggered_actions
            .iter()
            .any(|a| matches!(a.action_type, ActionType::MuteSystemAudio)),
        "Should have MuteSystemAudio action"
    );
}

#[then("one action should be NotifyUser")]
async fn then_notify_action(w: &mut AlephWorld) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    assert!(
        ctx.triggered_actions
            .iter()
            .any(|a| matches!(a.action_type, ActionType::NotifyUser { .. })),
        "Should have NotifyUser action"
    );
}

// ═══ YAML File Steps ═══

#[given("the example policies YAML file path")]
async fn given_yaml_path(w: &mut AlephWorld) {
    let ctx = w.e2e.get_or_insert_with(E2eContext::default);
    let yaml_path = std::env::current_dir()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/policies.yaml");
    ctx.yaml_path = Some(yaml_path);
}

#[then("the file should exist")]
async fn then_file_exists(w: &mut AlephWorld) {
    let ctx = w.e2e.as_mut().expect("E2E context not initialized");
    let yaml_path = ctx.yaml_path.as_ref().expect("YAML path not set");
    assert!(
        yaml_path.exists(),
        "Example YAML policy file should exist at {:?}",
        yaml_path
    );

    // Read content for subsequent assertions
    let content = std::fs::read_to_string(yaml_path).expect("Should be able to read policies.yaml");
    ctx.yaml_content = Some(content);
}

#[then(expr = "the file content should contain {string}")]
async fn then_content_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.e2e.as_ref().expect("E2E context not initialized");
    let content = ctx.yaml_content.as_ref().expect("YAML content not loaded");
    assert!(
        content.contains(&expected),
        "YAML content should contain '{}'",
        expected
    );
}
