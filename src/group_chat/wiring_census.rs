//! Census tests for the group-chat runtime wiring.
//!
//! Both wirings were once missing while everything around them looked
//! healthy: the `group_chat_sessions` / `group_chat_turns` tables were never
//! written (the tables, the queries and the `with_database` builders all
//! existed), and a persona's `provider` override was ignored because the
//! executor had no provider registry to look in — so every persona silently
//! ran on the default provider. Nothing failed loudly; the feature simply
//! did not do what its configuration promised.
//!
//! The census keeps the wiring from dying quietly again. It is a DETECTOR,
//! not a judge: it reports the wiring it did not find, and a human decides
//! what that means.

/// The production source of the server's startup wiring.
const STARTUP_WIRING: &str = include_str!("../bin/aleph-server/commands/start/mod.rs");

/// The production lines of the startup wiring.
fn startup_wiring_lines() -> Vec<String> {
    let source = crate::utils::source_scan::production_code_lines(STARTUP_WIRING);
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The group-chat orchestrator must be wired to the state database at
/// startup. Without it the `group_chat_sessions` table is never written and
/// the session history survives only in memory.
#[test]
fn the_group_chat_orchestrator_is_wired_to_the_state_database() {
    let lines = startup_wiring_lines();
    assert!(
        lines.iter().any(|line| line.contains("orchestrator.with_database(")),
        "the group-chat orchestrator is not wired to the state database"
    );
}

/// The group-chat executor must be wired to the state database too — it owns
/// the turn rows (`group_chat_turns`), which the orchestrator's session row
/// does not replace.
#[test]
fn the_group_chat_executor_is_wired_to_the_state_database() {
    let lines = startup_wiring_lines();
    assert!(
        lines.iter().any(|line| line.contains("executor.with_database(")),
        "the group-chat executor is not wired to the state database"
    );
}

/// The group-chat executor must be wired to the multi-provider registry, so
/// that a persona's `provider` override resolves to a real provider instead
/// of falling through to the default one.
#[test]
fn the_group_chat_executor_is_wired_to_the_provider_registry() {
    let lines = startup_wiring_lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("executor.with_provider_registry(")),
        "the group-chat executor is not wired to the provider registry"
    );
}
