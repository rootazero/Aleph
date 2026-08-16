use std::collections::HashMap;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{AgentId, FlowId};
use crate::orchestrator::resolver::{
    depth_guard, resolve_flow_id, RoutingOverrides, MAX_FLOW_DEPTH,
};

fn default_table() -> HashMap<AgentId, FlowId> {
    let mut m = HashMap::new();
    m.insert("main".into(), "default-agent".into());
    m.insert("researcher".into(), "researcher".into());
    m
}

fn no_overrides() -> RoutingOverrides {
    RoutingOverrides::default()
}

#[test]
fn depth_guard_allows_below_max() {
    assert!(depth_guard(0).is_ok());
    assert!(depth_guard(MAX_FLOW_DEPTH - 1).is_ok());
    assert!(depth_guard(MAX_FLOW_DEPTH).is_ok());
}

#[test]
fn depth_guard_rejects_above_max() {
    let err = depth_guard(MAX_FLOW_DEPTH + 1).unwrap_err();
    assert!(matches!(err, FlowError::RecursionLimit { max } if max == MAX_FLOW_DEPTH));
}

#[test]
fn default_table_resolves_known_agent() {
    let got = resolve_flow_id("main", None, &no_overrides(), &default_table()).unwrap();
    assert_eq!(got, "default-agent");
}

#[test]
fn unknown_agent_returns_error() {
    let err = resolve_flow_id("unknown", None, &no_overrides(), &default_table()).unwrap_err();
    assert!(matches!(err, FlowError::UnknownAgent(ref id) if id == "unknown"));
}

#[test]
fn exact_channel_override_wins() {
    let mut ov = no_overrides();
    ov.exact
        .insert(("main".into(), "telegram".into()), "main-lite".into());
    ov.wildcard.insert("main".into(), "default-agent".into());
    let got = resolve_flow_id("main", Some("telegram"), &ov, &default_table()).unwrap();
    assert_eq!(got, "main-lite");
}

#[test]
fn wildcard_override_used_for_non_matching_channel() {
    let mut ov = no_overrides();
    ov.exact
        .insert(("main".into(), "telegram".into()), "main-lite".into());
    ov.wildcard.insert("main".into(), "main-overridden".into());
    let got = resolve_flow_id("main", Some("slack"), &ov, &default_table()).unwrap();
    assert_eq!(got, "main-overridden");
}

#[test]
fn default_table_is_last_resort() {
    let ov = no_overrides();
    let got = resolve_flow_id("main", Some("slack"), &ov, &default_table()).unwrap();
    assert_eq!(got, "default-agent");
}

#[test]
fn no_channel_uses_wildcard_then_default() {
    let mut ov = no_overrides();
    ov.wildcard.insert("main".into(), "main-wild".into());
    let got = resolve_flow_id("main", None, &ov, &default_table()).unwrap();
    assert_eq!(got, "main-wild");
}

use crate::orchestrator::flow_spec::SessionStrategy;
use crate::orchestrator::resolver::{resolve_session, SessionResolveInput};

fn fixed_key() -> String {
    "fresh-abc".into()
}

#[test]
fn reuse_strategy_uses_hint() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Reuse,
        session_hint: Some("existing-123".into()),
        parent_session: None,
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.session_key, "existing-123");
    assert_eq!(r.parent_session_key, None);
    assert!(!r.is_new);
}

#[test]
fn reuse_strategy_without_hint_errors() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Reuse,
        session_hint: None,
        parent_session: None,
        fresh_key_fn: fixed_key,
    };
    let err = resolve_session(input).unwrap_err();
    assert!(matches!(err, FlowError::InvalidConfig(_)));
}

#[test]
fn fresh_strategy_mints_new_key() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Fresh,
        session_hint: Some("ignored".into()),
        parent_session: None,
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.session_key, "fresh-abc");
    assert!(r.is_new);
    assert!(r.parent_session_key.is_none());
}

#[test]
fn child_strategy_uses_parent_from_request() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Child {
            parent_session_key: None,
        },
        session_hint: None,
        parent_session: Some("parent-xyz".into()),
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.session_key, "fresh-abc");
    assert_eq!(r.parent_session_key.as_deref(), Some("parent-xyz"));
    assert!(r.is_new);
}

#[test]
fn child_strategy_request_beats_spec_override() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Child {
            parent_session_key: Some("from-spec".into()),
        },
        session_hint: None,
        parent_session: Some("from-request".into()),
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.parent_session_key.as_deref(), Some("from-request"));
}

#[test]
fn child_strategy_falls_back_to_spec_when_request_empty() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Child {
            parent_session_key: Some("from-spec".into()),
        },
        session_hint: None,
        parent_session: None,
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.parent_session_key.as_deref(), Some("from-spec"));
}

#[test]
fn child_strategy_without_parent_falls_back_to_fresh() {
    // Regression for the preset-flow dispatch gap: explore/coder/researcher &
    // co. declare `SessionStrategy::Child` with no static parent, and the
    // gateway run loop passes `parent_session: None` — hard-failing on
    // InvalidConfig made every dispatch to those flows fail. The graceful
    // degrade is a fresh session (parent link absent).
    let input = SessionResolveInput {
        strategy: SessionStrategy::Child {
            parent_session_key: None,
        },
        session_hint: None,
        parent_session: None,
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.session_key, "fresh-abc");
    assert_eq!(r.parent_session_key, None);
    assert!(r.is_new);
}
