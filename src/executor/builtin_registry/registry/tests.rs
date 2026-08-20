//! Unit tests for the registry free functions (`parse_caller_agent_id`).
#![allow(unused_imports)]

use super::free_fns::*;
use super::*;

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

/// The registry is constructed once, at boot, where no turn context exists —
/// so the agent id it bakes into `flag_user_correction` is always the base
/// agent. Corrections are namespaced per agent and `FeedbackDistill` reads one
/// agent's corpus at a time, so until the identity was resolved at dispatch
/// every correction a non-base agent recorded landed where that agent's
/// distillation never looks.
#[tokio::test]
async fn flag_user_correction_files_under_the_turns_agent_not_the_boot_agent() {
    use crate::executor::builtin_registry::{BuiltinToolConfig, BuiltinToolRegistry};
    use crate::executor::tool_registry::ToolRegistry;
    use crate::memory::store::raw_memory::RawMemoryStore;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::memory::store::MemoryBackend;
    use crate::routing::SessionKey;
    use crate::sync_primitives::Arc;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let db: MemoryBackend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
    let registry = BuiltinToolRegistry::with_config(BuiltinToolConfig {
        memory_db: Some(Arc::clone(&db)),
        ..Default::default()
    })
    .await
    .unwrap();

    let ctx = TurnContext {
        session_key: SessionKey::main("researcher"),
        run_id: String::new(),
        channel_id: String::new(),
        conversation_id: String::new(),
        caller_role: None,
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
    };
    let out = TURN_CONTEXT
        .scope(ctx, async {
            registry
                .execute_tool(
                    "flag_user_correction",
                    serde_json::json!({
                        "content": "Stop padding replies with restatements",
                        "severity": "med",
                    }),
                )
                .await
        })
        .await
        .expect("flag_user_correction dispatch");
    assert_eq!(out["success"], serde_json::json!(true), "{out}");

    let researcher = db
        .get_raw_by_path_prefix_since("aleph://correction/", "researcher", 0, 10)
        .await
        .unwrap();
    assert_eq!(
        researcher.len(),
        1,
        "the correction must land in the corpus of the agent that ran the turn"
    );
    let base = db
        .get_raw_by_path_prefix_since("aleph://correction/", "main", 0, 10)
        .await
        .unwrap();
    assert!(
        base.is_empty(),
        "nothing may land under the boot-time base agent"
    );
}
