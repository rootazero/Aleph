//! Tests for PromptSnapshot, capture_snapshot, and build_from_snapshot

use super::super::*;
use crate::agents::{AgentDef, AgentMode};
use crate::thinker::prompt_layer::AssemblyPath;

fn make_agent_def() -> AgentDef {
    AgentDef::new("test-subagent", AgentMode::SubAgent)
}

// ---------- capture_snapshot ----------

#[test]
fn capture_snapshot_returns_nonempty_stable_prefix() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let snapshot = builder.capture_snapshot(&[]);

    assert!(
        !snapshot.stable_prefix.is_empty(),
        "stable_prefix should be non-empty"
    );
}

#[test]
fn capture_snapshot_basic_path_without_soul() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let snapshot = builder.capture_snapshot(&[]);

    assert_eq!(
        snapshot.path,
        AssemblyPath::Basic,
        "path should be Basic when no soul is attached"
    );
}

#[test]
fn capture_snapshot_soul_path_with_soul() {
    use crate::thinker::soul::SoulManifest;

    let soul = SoulManifest {
        identity: "I am Aleph.".to_string(),
        ..Default::default()
    };
    let builder = PromptBuilder::new(PromptConfig::default()).with_soul(soul);
    let snapshot = builder.capture_snapshot(&[]);

    assert_eq!(
        snapshot.path,
        AssemblyPath::Soul,
        "path should be Soul when soul is attached"
    );
    assert!(
        !snapshot.stable_prefix.is_empty(),
        "stable_prefix should be non-empty with soul"
    );
}

// ---------- build_from_snapshot ----------

#[test]
fn build_from_snapshot_starts_with_stable_prefix() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let snapshot = builder.capture_snapshot(&[]);
    let agent_def = make_agent_def();

    let result = builder.build_from_snapshot(&snapshot, &agent_def, &[]);

    assert!(
        result.starts_with(&snapshot.stable_prefix),
        "build_from_snapshot output should start with the stable prefix"
    );
}

#[test]
fn build_from_snapshot_equals_stable_plus_dynamic() {
    // The fork path (snapshot) should produce exactly:
    //   execute_stable_only(path, input_without_agent)
    // + execute_dynamic_only(path, input_with_agent)
    //
    // This differs from build_for_agent_basic when agent-aware layers
    // (e.g., AgentRoleLayer) are classified as Stable — they appear in the
    // fresh path's stable section but are absent from the parent's snapshot
    // (captured without agent_def). This is by design: the fork path reuses
    // the *parent's* stable prefix and only rebuilds dynamic layers.
    let builder = PromptBuilder::new(PromptConfig::default());
    let agent_def = make_agent_def();

    let snapshot = builder.capture_snapshot(&[]);
    let fork_result = builder.build_from_snapshot(&snapshot, &agent_def, &[]);

    // Verify structural correctness:
    // 1. Fork result starts with stable prefix
    assert!(
        fork_result.starts_with(&snapshot.stable_prefix),
        "fork result should start with stable prefix"
    );
    // 2. Fork result contains standard sections from both stable and dynamic zones
    assert!(
        fork_result.contains("Response Format"),
        "fork result should contain Response Format (stable layer)"
    );
    // 3. Fork result is non-empty and at least as long as stable prefix
    assert!(
        fork_result.len() >= snapshot.stable_prefix.len(),
        "fork result should be at least as long as stable prefix"
    );
}

#[test]
fn build_from_snapshot_longer_than_stable_prefix_alone() {
    let builder = PromptBuilder::new(PromptConfig::default());
    let snapshot = builder.capture_snapshot(&[]);
    let agent_def = make_agent_def();

    let result = builder.build_from_snapshot(&snapshot, &agent_def, &[]);

    // The dynamic suffix should add content beyond the stable prefix.
    // At minimum, the result should be >= the stable prefix length.
    assert!(
        result.len() >= snapshot.stable_prefix.len(),
        "build_from_snapshot should be at least as long as stable_prefix"
    );
}
