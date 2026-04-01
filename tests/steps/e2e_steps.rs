//! Step definitions for E2E features (Policies)
//! NOTE: Evolution steps removed — skill_evolution module deleted

use crate::world::{AlephWorld, E2eContext};
use alephcore::daemon::dispatcher::policy::{ActionType, PolicyEngine};
use alephcore::daemon::events::{DerivedEvent, PressureLevel, PressureType};
use alephcore::daemon::worldmodel::state::{
    ActivityType, EnhancedContext, MemoryPressure, SystemLoad,
};
use chrono::Utc;
use cucumber::{given, then, when};

// TODO: removed — skill_evolution module deleted:
// All Evolution Setup, Execution, and Assertion steps have been removed.

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
