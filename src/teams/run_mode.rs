//! The usage mode every team-originated agent run executes in.
//!
//! A member run is not a user session. It has no composer, no mode pill, and
//! nobody watching it choose a register — the Panel hides `ModePicker` in team
//! chat for exactly that reason. Left unset, `resolve_turn_mode` falls through
//! to the global `[policies] mode`, which means an operator who switches their
//! own sessions to chat silently defers the `task` and `team` families out of
//! every team member's tool list — the same verbs `teams::leader_prompt` names
//! as the leader's four numbered duties.
//!
//! So team runs declare their mode instead of inheriting one. The narrowing
//! knob for a team is the per-member `tools` declaration
//! (`teams::member_provision`), not a session mode.

use std::collections::HashMap;

use crate::config::types::policies::{SessionMode, MODE_SESSION_KEY};

/// The mode every team-originated run executes in. `Work` is the identity
/// partition: it defers nothing and subtracts nothing from the core set, so a
/// team run's surface is exactly the registry minus whatever the member itself
/// declared.
pub const TEAM_RUN_MODE: SessionMode = SessionMode::Work;

/// Stamp [`TEAM_RUN_MODE`] onto a team run's request metadata.
///
/// `ExecutionEngine::resolve_turn_mode` treats a request-carried mode as
/// highest precedence and persists it onto the session; from the second turn on
/// `stored == requested` and the write short-circuits.
///
/// Called by both team run producers and nowhere else:
/// `teams::broadcast::member_run_metadata` (group chat fan-out) and
/// `teams::dispatcher::runner::task_run_metadata` (task dispatch / workflow
/// steps).
pub fn stamp(metadata: &mut HashMap<String, String>) {
    metadata.insert(MODE_SESSION_KEY.to_string(), TEAM_RUN_MODE.id().to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::member_provision::{LEADER_ESSENTIAL_TOOLS, WORKER_ESSENTIAL_TOOLS};

    #[test]
    fn stamp_writes_the_pinned_mode() {
        let mut metadata = HashMap::new();
        stamp(&mut metadata);
        assert_eq!(
            metadata.get(MODE_SESSION_KEY).map(String::as_str),
            Some("work")
        );
    }

    /// The invariant this module exists for. A team member's launch prompt
    /// contracts it to call these verbs; the mode its run executes in must not
    /// defer them out of the initial tool list. The names come from the same
    /// two constants `member_provision::validate_toolset` checks a declared
    /// `tools` list against, so the declaration-side contract and the
    /// presentation-side partition cannot drift.
    #[test]
    fn pinned_mode_never_defers_a_team_protocol_essential() {
        for name in WORKER_ESSENTIAL_TOOLS
            .iter()
            .chain(LEADER_ESSENTIAL_TOOLS.iter())
        {
            assert!(
                !TEAM_RUN_MODE.defers_tool(name),
                "`{name}` is contracted by the team prompts but deferred by mode `{}`",
                TEAM_RUN_MODE.id()
            );
        }
    }

    /// Chat would strand the protocol — this asserts the guard above actually
    /// has teeth rather than passing vacuously because nothing is ever
    /// deferred.
    #[test]
    fn the_guard_would_catch_a_chat_pin() {
        assert!(
            SessionMode::Chat.defers_tool("task_create"),
            "chat must still defer the task family, else the guard is vacuous"
        );
    }
}
