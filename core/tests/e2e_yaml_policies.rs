//! End-to-End Test for YAML Policies
//!
//! NOTE: This is a simplified E2E test for Phase 5.1 MVP.
//! Full YAML loading and Rhai integration will be added in Phase 5.2.
//! For now, we test the hardcoded MVP policies.

use alephcore::daemon::dispatcher::policy::PolicyEngine;
use alephcore::daemon::events::{DerivedEvent, PressureLevel, PressureType};
use alephcore::daemon::worldmodel::state::{ActivityType, EnhancedContext, MemoryPressure, SystemLoad};
use chrono::Utc;

#[test]
fn test_mvp_policies_meeting_auto_mute() {
    // Create PolicyEngine with 5 MVP policies
    let engine = PolicyEngine::new_mvp();

    // Should have 5 MVP policies
    assert_eq!(engine.policy_count(), 5);

    // Test evaluation - ActivityChanged to Meeting
    let context = EnhancedContext::default();
    let event = DerivedEvent::ActivityChanged {
        timestamp: Utc::now(),
        old_activity: ActivityType::Idle,
        new_activity: ActivityType::Meeting { participants: 5 },
        confidence: 0.9,
    };

    let actions = engine.evaluate_all(&context, &event);

    // Should trigger "Meeting Auto-Mute" rule
    assert!(
        !actions.is_empty(),
        "MeetingMutePolicy should trigger for Meeting activity"
    );

    // Check that one action is MuteSystemAudio
    assert!(
        actions.iter().any(|a| {
            matches!(
                a.action_type,
                alephcore::daemon::dispatcher::policy::ActionType::MuteSystemAudio
            )
        }),
        "Should have MuteSystemAudio action"
    );
}

#[test]
fn test_mvp_policies_low_battery_alert() {
    let engine = PolicyEngine::new_mvp();

    let mut context = EnhancedContext::default();
    context.system_constraint = SystemLoad {
        cpu_usage: 0.0,
        memory_pressure: MemoryPressure::Normal,
        battery_level: Some(15), // Low battery (below 20%)
    };

    let event = DerivedEvent::ResourcePressureChanged {
        timestamp: Utc::now(),
        pressure_type: PressureType::Battery,
        old_level: PressureLevel::Normal,
        new_level: PressureLevel::Critical,
    };

    let actions = engine.evaluate_all(&context, &event);

    // Should trigger "Low Battery" policy
    assert!(
        !actions.is_empty(),
        "LowBatteryPolicy should trigger when battery is low"
    );

    // Check for notification
    assert!(
        actions.iter().any(|a| {
            matches!(
                a.action_type,
                alephcore::daemon::dispatcher::policy::ActionType::NotifyUser { .. }
            )
        }),
        "Should have NotifyUser action for low battery"
    );
}

#[test]
fn test_example_yaml_policy_file_exists() {
    // Verify the example YAML file exists
    let yaml_path = std::env::current_dir()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/policies.yaml");

    assert!(
        yaml_path.exists(),
        "Example YAML policy file should exist at {:?}",
        yaml_path
    );

    // Verify it's readable
    let content = std::fs::read_to_string(&yaml_path)
        .expect("Should be able to read policies.yaml");

    assert!(
        content.contains("Low Battery Alert"),
        "Should contain example policies"
    );
    assert!(
        content.contains("Meeting Auto-Mute"),
        "Should contain example policies"
    );
}
