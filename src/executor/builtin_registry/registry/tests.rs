//! Unit tests for the registry free functions (`parse_caller_agent_id`).
#![allow(unused_imports)]

use super::*;
use super::free_fns::*;

use super::parse_caller_agent_id;

// Regression for BUG-1: a naive `.split(':').next()` returned the literal
// namespace prefix `"agent"`, silently misrouting all per-agent state
// (RememberTool wrote to `agents/agent/MEMORY.md` while readers used
// `agents/<id>/MEMORY.md`). These cases lock the canonical key parser as
// the single source of truth.

#[test]
fn extracts_agent_id_from_main_key() {
    assert_eq!(parse_caller_agent_id("agent:main:main", "fallback"), "main");
    assert_eq!(parse_caller_agent_id("agent:work:main", "fallback"), "work");
}

#[test]
fn extracts_agent_id_from_dm_key() {
    assert_eq!(
        parse_caller_agent_id("agent:assistant:dm:user1", "fallback"),
        "assistant"
    );
    assert_eq!(
        parse_caller_agent_id("agent:assistant:telegram:dm:user1", "fallback"),
        "assistant"
    );
    assert_eq!(
        parse_caller_agent_id("agent:assistant:peer:user1", "fallback"),
        "assistant"
    );
}

#[test]
fn extracts_agent_id_from_group_and_task_keys() {
    assert_eq!(
        parse_caller_agent_id("agent:codev:discord:group:guild456", "fallback"),
        "codev"
    );
    assert_eq!(
        parse_caller_agent_id("agent:scheduler:cron:daily-summary", "fallback"),
        "scheduler"
    );
}

#[test]
fn returns_fallback_for_garbage_or_empty() {
    assert_eq!(parse_caller_agent_id("", "fallback"), "fallback");
    assert_eq!(parse_caller_agent_id("not-a-session-key", "main"), "main");
    // missing trailing component -> parse fails -> fallback.
    assert_eq!(parse_caller_agent_id("agent:", "main"), "main");
}

#[test]
fn never_returns_literal_agent_namespace_prefix() {
    // The historical bug returned the literal "agent" string from
    // `.split(':').next()`. Every realistic key form must yield the
    // actual agent_id, not the namespace prefix.
    for key in [
        "agent:main:main",
        "agent:work:main",
        "agent:assistant:dm:user1",
        "agent:scheduler:webhook:hook-1",
        "agent:rust-bot:slack:channel:c123",
    ] {
        let id = parse_caller_agent_id(key, "fallback");
        assert_ne!(
            id, "agent",
            "regression: key {key} parsed to namespace prefix instead of agent_id"
        );
    }
}
