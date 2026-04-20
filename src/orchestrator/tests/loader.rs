use crate::orchestrator::loader::{load_presets, load_user_flows_from_str};

#[test]
fn preset_catalog_contains_seven_flows() {
    let set = load_presets().expect("parse presets");
    let ids: Vec<&String> = set.keys().collect();
    assert_eq!(set.len(), 7, "expected 7 preset flows, got {ids:?}");
    for expected in &[
        "default-agent",
        "explore",
        "coder",
        "researcher",
        "default",
        "plan",
        "verify",
    ] {
        assert!(set.contains_key(*expected), "missing preset {expected}");
    }
}

#[test]
fn preset_default_agent_targets_main() {
    let set = load_presets().expect("parse presets");
    let default_agent = set.get("default-agent").unwrap();
    assert_eq!(default_agent.agent, "main");
}

#[test]
fn user_flow_file_with_single_flow_parses() {
    let toml_src = r#"
[[flow]]
id = "user/my-flow"
description = "My custom flow"
agent = "main"
sandbox_kind = "none"
brain = { kind = "preferred", provider = "minimax" }
session_strategy = { kind = "fresh" }
"#;
    let set = load_user_flows_from_str(toml_src).expect("parse");
    assert_eq!(set.len(), 1);
    assert!(set.contains_key("user/my-flow"));
}

#[test]
fn duplicate_flow_id_within_single_file_is_rejected() {
    let toml_src = r#"
[[flow]]
id = "dup"
description = "first"
agent = "main"
sandbox_kind = "none"
brain = { kind = "default" }
session_strategy = { kind = "fresh" }

[[flow]]
id = "dup"
description = "second"
agent = "main"
sandbox_kind = "none"
brain = { kind = "default" }
session_strategy = { kind = "fresh" }
"#;
    let err = load_user_flows_from_str(toml_src).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("duplicate"),
        "got {err}"
    );
}
