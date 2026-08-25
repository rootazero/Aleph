//! The task-locals a spawned unit of work must carry, and the one way to
//! carry them.
//!
//! Lives in `src/scope/` rather than next to any one of its users because it
//! now has three: sub-agents (`agents::subagent_tool`), team group-chat
//! fan-out and the team fan-out's per-member spawn
//! (`teams::broadcast`). The rule the repo already follows — the single source
//! belongs on the side that is depended upon — puts it here: `agents` and
//! `teams` both depend on `scope`, and neither depends on the other.

/// The task-locals a spawned unit of work must carry across a
/// `tokio::spawn`, and the one way to carry them.
///
/// `tokio::task_local!` does not cross a spawn boundary — a child task reads
/// `None`, never the parent's value. The three here decide, between them, which
/// memory partition the work reads and writes, which project its file tools
/// see, and which agent identity its notes are filed under. Losing them is
/// silent: the work runs, answers, and files everything into the unscoped base
/// namespace.
///
/// This exists as one type because the shape had already been written once,
/// correctly, in `subagent_tool::spawn_background` — and the batch legs of
/// `subagent_loop`, added later in the same crate, re-seeded **none of the
/// three**. Two copies of a three-line ritual is how the second variant loses
/// it; `gateway/CLAUDE.md` 地雷 C even names "another subagent variant" as the
/// thing to watch for.
///
/// The team fan-out was the third and fourth variant, and it lost them the same
/// way: `teams.chat.send` spawns from inside a gateway dispatch where the scope
/// IS live, and `dispatch_user` then spawns again per member. Every member run
/// therefore executed with `current_scope() == None`, so its session rows were
/// written NULL/NULL (adopted by the operator) and every memory write it made
/// landed in the BASE partition that `session_read_ids` unions into every other
/// principal's turn.
/// The room author was the fifth, and it fails in a third direction again:
/// not quiet, not open, but **confidently wrong**. `ambient_actor()` — the
/// resolver every "who is asking" predicate uses — falls back to the scope's
/// `owner_user_id` when no author is in scope, and a project room's owner is
/// its CREATOR, the same person for every member. So a child spawned from
/// Bob's turn in Alice's room does not lose its actor: it acquires Alice's.
/// `memory_search` then passes `partition_visible_to("main__u-alice", …)` and
/// reads her personal corpus, `session_list` enumerates her personal sessions,
/// and the signed ledger files Bob's child's actions under her name.
///
/// `scope/mod.rs`'s own
/// `dropping_the_author_at_a_spawn_silently_reports_the_room_owner` already
/// asserted this shape, commented "not a missing label, it is a confidently
/// wrong one" — it just had no carrier to fix it in.
///
/// The sixth — [`EXEC_WORKSPACE`](crate::sandbox::context::EXEC_WORKSPACE) —
/// is not attribution at all, which is why the type's name understates its
/// contract (read the first line: *the task-locals a spawned unit of work must
/// carry*). It is the directory this run is AUTHORISED to execute in, and it
/// fails in a fourth direction again: not quiet, not open, not wrong, but
/// **dead**. `WorkspaceSandbox::jail_root_for` falls back to
/// `workspaces/<hash(session)>` when nobody published one, and a detached
/// child's session is a fresh nonce — so the jail is an empty directory
/// created on first use. Every shell command the child runs lands in a tree
/// that contains nothing, and every absolute path it addresses into the real
/// project is refused "cwd outside workspace root". A foreground sub-agent,
/// which never crosses a spawn, has always had the parent's root; the
/// background and batch legs of the same tool did not, so the same delegation
/// worked or did not depending on whether the caller passed `background`.
///
/// Carrying it does **not** weaken the "a nested run cannot inherit an outer
/// run's workspace" rule that makes `EXEC_WORKSPACE` an `Option` published
/// explicitly: a nested *run* re-enters `run_agent_loop`, which scopes its own
/// value over this one. What is carried here is one run's own tree of tasks.
/// And it stays gateway-owned by construction — the carrier reads the
/// task-local, never a model-writable field.
#[derive(Clone, Default)]
pub struct CarriedAttribution {
    scope: Option<super::ScopeAttribution>,
    project_root: Option<std::path::PathBuf>,
    agent_id: Option<String>,
    caller_role: Option<String>,
    room_author: Option<String>,
    exec_workspace: Option<std::path::PathBuf>,
}

impl CarriedAttribution {
    /// Read the six task-locals. **Must be called BEFORE `tokio::spawn`** —
    /// inside the spawned future they are already gone.
    ///
    /// `caller_role` joined the other three because it fails in the same way and
    /// in the same place, but in the opposite direction: the three above are
    /// fail-QUIET (work lands in the wrong partition), while an absent role is
    /// fail-OPEN. `turn_context::role_is_operator(None)` is `true` by design
    /// ("absent role = local/internal = trusted"), so a member-initiated run
    /// that loses it skips BOTH the exec-tier ceiling
    /// (`ExecTier::most_restrictive(tier, global)`) and the operator-tool gate.
    /// Every reader of this task-local lives in a gateway handler, so
    /// re-establishing it inside spawned work changes nothing except the
    /// metadata a run request stamps from it.
    #[must_use]
    pub fn capture() -> Self {
        Self {
            scope: super::current_scope(),
            project_root: crate::projects::current_project_root(),
            agent_id: crate::agents::current_agent_id(),
            caller_role: crate::gateway::caller_identity::current_caller_role(),
            room_author: super::current_room_author(),
            exec_workspace: crate::sandbox::context::current_exec_workspace(),
        }
    }

    /// The carried scope, for a caller that has to stamp it onto a
    /// `RunRequest`'s metadata rather than (or as well as) re-establish it.
    ///
    /// `run_loop` rebuilds the run's scope from `request.metadata` and nothing
    /// else, so a spawn that only re-establishes the task-local still writes a
    /// NULL/NULL session row. Both halves are needed and they are not the same
    /// half.
    #[must_use]
    pub const fn scope(&self) -> Option<&super::ScopeAttribution> {
        self.scope.as_ref()
    }

    /// Override the carried room author with a speaker the caller knows
    /// explicitly.
    ///
    /// `capture()` reads `room_author` from [`super::current_room_author`],
    /// which is only ever seeded by [`super::with_room_author`] — and nothing
    /// on `teams.chat.send`'s path scopes that around this handler, because
    /// it learns who is speaking (the gateway caller's identity) BEFORE any
    /// run or task-local nest exists. Without this override the carried
    /// `room_author` stays `None`, and `ambient_room_author()` inside the
    /// spawned fan-out then falls back to the scope's `owner_user_id` — a
    /// project room's CREATOR, identically for every member. A message from
    /// Bob in Alice's room would silently resolve its actor to Alice:
    /// confidently wrong, not merely missing (see the type-level doc above).
    ///
    /// `with_speaker` only overwrites when `author.is_some()`, so a call with
    /// no known speaker keeps whatever `capture()` already read — this never
    /// widens what was captured, and never blanks a value `capture()` found.
    #[must_use]
    pub fn with_speaker(mut self, author: Option<String>) -> Self {
        if author.is_some() {
            self.room_author = author;
        }
        self
    }

    /// Re-establish all six around `fut`, inside the spawned task.
    ///
    /// Boxed: `AgentRuntime::run`'s state machine is already large, and nesting
    /// three task-local combinators around it inline overflowed the
    /// debug-build test-thread stack. The `Box::pin` stays exactly where it was
    /// when that was discovered — it is load-bearing, not tidiness.
    pub async fn reestablish<F, T>(self, fut: F) -> T
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        crate::gateway::caller_identity::CALLER_ROLE
            .scope(
                self.caller_role,
                crate::agents::with_agent_id(
                    self.agent_id,
                    crate::projects::with_project_root(
                        self.project_root,
                        super::with_scope(
                            self.scope,
                            super::with_room_author(
                                self.room_author,
                                crate::sandbox::context::with_exec_workspace(
                                    self.exec_workspace,
                                    Box::pin(fut),
                                ),
                            ),
                        ),
                    ),
                ),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{with_scope, ScopeAttribution};

    /// The property the whole type exists for: a bare `tokio::spawn` loses the
    /// scope, and `capture` + `reestablish` restores exactly it.
    #[tokio::test]
    async fn a_bare_spawn_loses_the_scope_and_the_carrier_restores_it() {
        let attr = ScopeAttribution::personal("u-alice");

        let (lost, carried) = with_scope(Some(attr.clone()), async {
            let lost = tokio::spawn(async { crate::scope::current_scope() })
                .await
                .unwrap();
            let carrier = CarriedAttribution::capture();
            let carried =
                tokio::spawn(carrier.reestablish(async { crate::scope::current_scope() }))
                    .await
                    .unwrap();
            (lost, carried)
        })
        .await;

        assert!(
            lost.is_none(),
            "a bare spawn must lose it — if this ever passes, the reason this type exists is gone"
        );
        assert_eq!(
            carried.map(|a| a.owner_user_id),
            Some("u-alice".to_string())
        );
    }

    /// The room author must survive the spawn, and this is the one carried
    /// value whose loss is not silent-and-empty but silent-and-WRONG: with the
    /// scope carried and the author dropped, `ambient_room_author()` falls back
    /// to the scope's `owner_user_id`, which in a project room is the CREATOR.
    /// A child spawned from Bob's turn in Alice's room does not become
    /// anonymous — it becomes Alice, and every "who is asking" predicate
    /// downstream agrees.
    ///
    /// `scope/mod.rs::dropping_the_author_at_a_spawn_silently_reports_the_room_owner`
    /// is the negative half of this pair: it asserts the wrong answer that
    /// carrying the scope alone produces.
    #[tokio::test]
    async fn the_carrier_keeps_the_room_author_and_not_just_the_room() {
        let room = ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: crate::scope::ScopeId::Project("p-room".into()),
        };

        let carried = with_scope(
            Some(room),
            crate::scope::with_room_author(Some("u-bob".to_string()), async {
                let carrier = CarriedAttribution::capture();
                tokio::spawn(carrier.reestablish(async { crate::scope::ambient_room_author() }))
                    .await
                    .unwrap()
            }),
        )
        .await;

        assert_eq!(
            carried,
            Some("u-bob".to_string()),
            "the speaker must cross the spawn; falling back to u-alice is the \
             room's creator, which is a confident answer about the wrong person"
        );
    }

    /// `teams.chat.send`'s actual shape: nothing seeds `with_room_author`
    /// around this handler (it resolves the speaker itself, before any run
    /// exists), so `capture()` alone carries `room_author: None` — and inside
    /// the spawn, `ambient_room_author()` would fall back to the room's
    /// CREATOR (`u-alice`) for a message Bob just sent. `with_speaker` is the
    /// fix: the handler passes the speaker it already resolved, and that
    /// speaker — not the room owner — is what the fan-out sees.
    ///
    /// This is Ruling P1's project-scope case: the one `ambient_owner()`
    /// alone gets confidently wrong (see `visibility::ambient_actor`'s doc).
    #[tokio::test]
    async fn with_speaker_overrides_the_room_owner_fallback_when_nothing_seeded_an_author() {
        let room = ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: crate::scope::ScopeId::Project("p-room".into()),
        };

        // No `with_room_author` around `capture()` — the handler-level shape.
        let carried = with_scope(Some(room), async {
            assert_eq!(
                crate::scope::current_room_author(),
                None,
                "sanity: nothing seeded an author before capture in this shape"
            );
            let carrier = CarriedAttribution::capture().with_speaker(Some("u-bob".to_string()));
            tokio::spawn(carrier.reestablish(async { crate::scope::ambient_room_author() }))
                .await
                .unwrap()
        })
        .await;

        assert_eq!(
            carried,
            Some("u-bob".to_string()),
            "with_speaker must make the actual speaker (u-bob) cross the spawn; \
             without it this resolves to u-alice, the room's creator — a \
             confident answer about the wrong person, not a missing one"
        );
    }

    /// `with_speaker(None)` (no resolved speaker — an unattributed/internal
    /// caller) must be a no-op: it must not blank a `room_author` that
    /// `capture()` already found from a live `with_room_author` scope. Only
    /// `Some(author)` may override.
    #[tokio::test]
    async fn with_speaker_none_never_blanks_a_captured_author() {
        let room = ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: crate::scope::ScopeId::Project("p-room".into()),
        };

        let carried = with_scope(
            Some(room),
            crate::scope::with_room_author(Some("u-bob".to_string()), async {
                let carrier = CarriedAttribution::capture().with_speaker(None);
                tokio::spawn(carrier.reestablish(async { crate::scope::ambient_room_author() }))
                    .await
                    .unwrap()
            }),
        )
        .await;

        assert_eq!(
            carried,
            Some("u-bob".to_string()),
            "with_speaker(None) must never widen/blank what capture() already carried"
        );
    }

    /// The jail root must cross the spawn, and this is the one carried value
    /// whose loss is neither quiet nor wrong but **dead**: with it absent,
    /// `WorkspaceSandbox::jail_root_for` anchors the child on
    /// `workspaces/<hash(its own fresh session nonce)>` — an empty directory
    /// it creates on first use — so every command the child runs executes in a
    /// tree containing nothing, and every absolute path into the real project
    /// is refused "cwd outside workspace root".
    ///
    /// The negative half is asserted in the same test on purpose: a bare spawn
    /// must still lose it. If that half ever passes, this carrier leg is dead
    /// weight and should go.
    #[tokio::test]
    async fn the_carrier_keeps_the_runs_authorised_exec_root() {
        use crate::sandbox::context::{current_exec_workspace, with_exec_workspace};

        let root = std::path::PathBuf::from("/tmp/aleph-carried-jail-root");

        let (lost, carried) = with_exec_workspace(Some(root.clone()), async {
            let lost = tokio::spawn(async { current_exec_workspace() })
                .await
                .unwrap();
            let carrier = CarriedAttribution::capture();
            let carried = tokio::spawn(carrier.reestablish(async { current_exec_workspace() }))
                .await
                .unwrap();
            (lost, carried)
        })
        .await;

        assert!(
            lost.is_none(),
            "a bare spawn must lose it — if this ever passes, the reason this leg exists is gone"
        );
        assert_eq!(
            carried,
            Some(root),
            "a detached child must execute in the root its run was authorised for,              not in a fresh empty hash directory"
        );
    }

    /// Capturing outside any scope is the unrestricted case and must stay a
    /// no-op, so wrapping a cron / test / A2A spawn changes nothing.
    #[tokio::test]
    async fn capturing_nothing_reestablishes_nothing() {
        let carrier = CarriedAttribution::capture();
        assert!(carrier.scope().is_none());
        let inner = tokio::spawn(carrier.reestablish(async {
            (
                crate::scope::current_scope().is_some(),
                crate::sandbox::context::current_exec_workspace().is_some(),
            )
        }))
        .await
        .unwrap();
        assert_eq!(
            inner,
            (false, false),
            "publishing `None` for the exec root must stay indistinguishable from              never having scoped it — that absence is what keeps every caller              outside a run (cron, cluster file commands, tests) on the historical              per-session workspace"
        );
    }
}
