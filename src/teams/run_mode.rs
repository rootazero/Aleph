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
use crate::gateway::execution_engine::{BusyInputMode, BUSY_INPUT_MODE_KEY};
use crate::routing::session_key::SessionKey;

/// The mode every team-originated run executes in. `Work` is the identity
/// partition: it defers nothing and subtracts nothing from the core set, so a
/// team run's surface is exactly the registry minus whatever the member itself
/// declared.
pub const TEAM_RUN_MODE: SessionMode = SessionMode::Work;

/// The busy-input policy every team-originated run declares.
///
/// A team run is a WHOLE DISPATCHED INTENT, not a remark. Left unset the gate
/// falls back to `Steer`, and a same-session collision (a member @'ed twice in
/// one round, a task re-dispatched onto a session whose predecessor is still
/// draining) folds the new run's prompt into the running sibling as a bare
/// `UserMessage` and returns `Ok(())` — the dispatcher records a completed run
/// that executed nothing of what it asked, because everything a `RunRequest`
/// carries beyond text (sandbox / workspace / iteration cap / timeout) is
/// resolved AFTER the admission gate. `Queue` leaves the running sibling alone
/// and surfaces `AgentBusy`, which both producers' callers already understand.
pub const TEAM_RUN_BUSY_INPUT_MODE: BusyInputMode = BusyInputMode::Queue;

/// `task_type` of a group-chat member run's session key
/// (`teams::broadcast`, keyed by team id).
pub const TEAM_CHAT_TASK_TYPE: &str = "team_chat";

/// `task_type` of a task-dispatch member run's session key
/// (`teams::dispatcher`, keyed by task id).
pub const TEAM_TASK_TASK_TYPE: &str = "team";

/// True when `key` addresses a team-originated run — exactly the sessions
/// whose mode [`stamp`] re-pins on every turn.
///
/// Lives here rather than on `SessionKey` because the fact it encodes is this
/// module's: "which sessions are team runs" and "what mode team runs are
/// pinned to" are the same fact, and a consumer that reads one without the
/// other gets a half-truth. Both task types are used at their construction
/// sites via the constants above so the predicate cannot drift from them.
///
/// Only direct `Task` keys match. A subagent spawned inside a member run
/// carries its own key and its own explicitly-passed mode; it is not a team
/// run and [`stamp`] never touches it.
#[must_use]
pub fn is_team_session(key: &SessionKey) -> bool {
    matches!(
        key,
        SessionKey::Task { task_type, .. }
            if task_type == TEAM_CHAT_TASK_TYPE || task_type == TEAM_TASK_TASK_TYPE
    )
}

/// Stamp [`TEAM_RUN_MODE`] and [`TEAM_RUN_BUSY_INPUT_MODE`] onto a team run's
/// request metadata.
///
/// `ExecutionEngine::resolve_turn_mode` treats a request-carried mode as
/// highest precedence and persists it onto the session; from the second turn on
/// `stored == requested` and the write short-circuits. The busy-input mode is
/// read per-request by the admission gate (`BusyInputMode::from_metadata`) and
/// never persisted.
///
/// Called by both team run producers and nowhere else:
/// `teams::broadcast::member_run_metadata` (group chat fan-out) and
/// `teams::dispatcher::runner::task_run_metadata` (task dispatch / workflow
/// steps). Both facts are stamped HERE rather than at the two call sites: a
/// second copy at each producer is a second source, and the one that drifts is
/// always the one nobody re-reads.
pub fn stamp(metadata: &mut HashMap<String, String>) {
    metadata.insert(MODE_SESSION_KEY.to_string(), TEAM_RUN_MODE.id().to_string());
    metadata.insert(
        BUSY_INPUT_MODE_KEY.to_string(),
        TEAM_RUN_BUSY_INPUT_MODE.as_wire().to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::{DmScope, SessionKey};
    use crate::teams::member_provision::{LEADER_ESSENTIAL_TOOLS, WORKER_ESSENTIAL_TOOLS};

    /// The predicate must recognise keys built exactly the way the three
    /// production sites build them — same constructor, same constants.
    #[test]
    fn is_team_session_matches_both_team_run_producers() {
        assert!(is_team_session(&SessionKey::task(
            "alice",
            TEAM_CHAT_TASK_TYPE,
            "squad"
        )));
        assert!(is_team_session(&SessionKey::task(
            "alice",
            TEAM_TASK_TASK_TYPE,
            "task-1"
        )));
    }

    /// `SessionKey::task` normalizes `task_type`; if that ever mangled the two
    /// constants the predicate would still match the *constructed* key above
    /// while missing every key parsed back from storage. Pin the round trip.
    #[test]
    fn team_task_types_survive_key_serialization() {
        for task_type in [TEAM_CHAT_TASK_TYPE, TEAM_TASK_TASK_TYPE] {
            let key = SessionKey::task("alice", task_type, "id-1");
            let parsed = SessionKey::parse(&key.to_key_string()).expect("must parse");
            assert!(
                is_team_session(&parsed),
                "`{task_type}` did not survive the key round trip: {}",
                key.to_key_string()
            );
        }
    }

    #[test]
    fn is_team_session_rejects_non_team_sessions() {
        assert!(!is_team_session(&SessionKey::main("alice")));
        assert!(!is_team_session(&SessionKey::dm(
            "alice",
            "telegram",
            "u1",
            DmScope::PerPeer
        )));
        assert!(!is_team_session(&SessionKey::task(
            "alice", "cron", "daily"
        )));
    }

    #[test]
    fn stamp_writes_the_pinned_mode() {
        let mut metadata = HashMap::new();
        stamp(&mut metadata);
        assert_eq!(
            metadata.get(MODE_SESSION_KEY).map(String::as_str),
            Some("work")
        );
    }

    /// The wire value the gate parses must be `queue` — not merely "some
    /// busy-input key is present". A typo here reads back as the `Steer`
    /// fallback (`from_wire` maps every unknown string to it), which is
    /// exactly the silent inline-fold this stamp exists to prevent.
    #[test]
    fn stamp_writes_the_queue_busy_input_mode() {
        let mut metadata = HashMap::new();
        stamp(&mut metadata);
        assert_eq!(
            metadata.get(BUSY_INPUT_MODE_KEY).map(String::as_str),
            Some("queue")
        );
        assert_eq!(
            BusyInputMode::from_metadata(&metadata),
            BusyInputMode::Queue,
            "the gate must parse a team run's stamp back as Queue"
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
